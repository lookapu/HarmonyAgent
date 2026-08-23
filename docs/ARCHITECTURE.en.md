# DevEco Switch Architecture

> This document describes the actual implementation on the `main` branch as of 2026-08-21. Historical planning and capability inventories are covered in `CHANGELOG.md`, `TOOL_ENHANCEMENTS.md`, and `HARNESS_ENHANCEMENTS.md` respectively.

[简体中文](ARCHITECTURE.md) | English

## 1. Product Positioning

DevEco Switch is a local desktop Agent workspace for HarmonyOS/OpenHarmony developers. The user selects a project and states a goal in natural language; the Agent is responsible for reading and modifying code, building, testing, deploying, reading device logs, and verifying the results.

It adheres to three product boundaries:

1. **Task-mode first**: the conversation is the main operation thread — plans, tools, diffs, logs, approvals, and acceptance all take place in the task context.
2. **Not a replacement for professional IDEs**: it provides a file tree, read-only previews, and necessary simple editing, but does not build a multi-tab editor, a full terminal, or a complete IDE plugin ecosystem.
3. **HarmonyOS loop first**: general Agent capabilities serve HarmonyOS project understanding, hvigor/ohpm builds, hdc device control, and SDK/API compatibility analysis.

The project started as a Provider manager; today its core is the Rust-backed Agent execution kernel. Providers, proxy, cost, and health checks are its infrastructure, not the product's end goal.

## 2. Architecture Overview

```text
┌──────────────────────────────────────────────────────────────┐
│ React 19 + TypeScript + Zustand                              │
│ Projects/sessions, streaming messages, plan/approval/tool     │
│ cards, file & device panels, admin pages                      │
└────────────────────────────┬─────────────────────────────────┘
                             │ Tauri invoke / event
┌────────────────────────────▼─────────────────────────────────┐
│ Rust / Tauri 2                                               │
│                                                              │
│ commands/chat.rs                                             │
│   Context assembly → streaming model request → tool parsing  │
│   → execution loop → acceptance                              │
│                                                              │
│ Agent Execution Kernel                                      │
│   Run state machine / execution steps / scheduler queue /    │
│   DAG / recovery / governance                                │
│                                                              │
│ Tool Execution Kernel                                       │
│   201 tools / approval pipeline / dedicated threads /        │
│   leases / fencing / idempotency                             │
│                                                              │
│ Services                                                     │
│   Provider/proxy/circuit breaker/cost, HarmonyOS env,        │
│   knowledge base, MCP, LAN                                   │
└────────────────────────────┬─────────────────────────────────┘
                             │
┌────────────────────────────▼─────────────────────────────────┐
│ SQLite (77 migrations) + local files/keychain + external     │
│ toolchain                                                    │
│ HarmonyOS SDK / hvigor / ohpm / hdc / ArkTS LSP              │
└──────────────────────────────────────────────────────────────┘
```

Current codebase scale:

| Item | Actual value |
|---|---:|
| Agent-facing tools | 201 |
| `agent/` top-level modules (excluding `mod.rs`) | 36 |
| `agent/tools/` Rust files (incl. `mod.rs`) | 30 |
| `commands/` command modules (excluding `mod.rs`) | 36 |
| `services/` service modules (excluding `mod.rs`) | 54 |
| Tauri IPC registration entry points | 296 |
| Database migrations | 77 |
| React pages | 16 |

These counts evolve with the codebase; the authoritative sources are `TOOL_SPECS` for tools, the `generate_handler!` in `lib.rs` for IPC entry points, and `src-tauri/migrations/` for migrations.

## 3. Frontend

### 3.1 Pages and Routing

`src/App.tsx` uses React Router. The root route `/` is the Agent Workspace (`Home.tsx`); the other 13 pages cover LAN, Providers, runtime versions, configuration, limits, cost & reliability, proxy, MCP, Skills, knowledge base, HarmonyOS APIs, health checks, and the ohpm ecosystem.

All pages are lazy-loaded by route so that Recharts, Markdown/KaTeX, and device-diagnostics code never block first paint.

### 3.2 State Model

`projectStore.ts` composes three Zustand slices:

- `projectSlice`: projects, workspace modules, file tree, and Git branches;
- `chatSlice`: conversations, messages, streaming buckets, tool runs, approvals, plans, and pending confirmations;
- `memorySlice`: memories, feedback, stats, and message versions.

Streaming state is bucketed by `conversation_id` rather than only holding "the current conversation". After the user switches projects or sessions, background-task increments, tool events, and final states still land in their corresponding buckets.

### 3.3 IPC and Events

Frontend API modules only wrap Tauri `invoke`. Long tasks update the UI through event streams; the main events include body/reasoning increments, tool start & end, sub-agents, plan reviews, approvals, task ledgers, governance status, completion, stop, and errors.

SQLite and the Rust state machine are the source of truth for task state; frontend timers and watchdogs are for display and fallback only and cannot override backend final states.

## 4. Conversation and Agent Main Loop

Agent orchestration actually lives in `src-tauri/src/commands/chat.rs`, not the frontend.

The main flow of one `stream_chat` call:

1. Register a unique task and AbortHandle per session, rejecting concurrent writes to the same session;
2. Persist the user message, create the Run, the root DAG node, and the durable queue record;
3. Compile the goal contract and the dynamic execution budget;
4. Assemble system prompt, project rules, history summaries, memories, diagnostics, Skills, MCP, references, and task ledger;
5. Select the Provider/model and start a streaming request using the OpenAI, Anthropic, or Gemini protocol;
6. Parse native function calling or textual tool markers;
7. Execute approvals, budgets, tool calls, retries, and result persistence;
8. Inject tool results into the next round until the model wraps up or stop/budget/error triggers;
9. Accept the goal against real tool evidence; auto-remedy when evidence is missing, entering `completed` only when the contract is satisfied;
10. Converge the Durable Run final state first, then write task stats and send the completion event to the frontend.

Key protections in the main loop:

- freeze-and-replay when the stream breaks before the first byte, continuation after output interruption, and retry on empty responses;
- rolling summaries once context hits the threshold, keeping recent messages;
- loop detection for consecutive identical tools, consecutive same-name tools, and total tool count;
- re-planning after consecutive failures;
- read-only tools run at most 4-way concurrent; write tools act as a serial barrier;
- tool-round and duration budgets are computed dynamically from goal complexity and only grow while evidence keeps being produced;
- a model's "fixed/verified/done" claims cannot replace tool evidence.

### 4.1 Long-Session Context V2

`agent/context.rs` splits long sessions into four layers: hot messages, task state, project facts, and historical archive. It is a rebuildable projection and does not replace `messages`, events, the Durable Run, tool results, or the workspace's true state.

- `TaskSnapshotV2` is rebuilt each round from the latest Run, goal contract, and execution steps; older sessions read the task ledger compatibly.
- Summary records cover message rowids and event seqs; Context checkpoints retain the latest 80 versions.
- Build, Git, device results, and tool artifacts become sourced facts or references, recording source, digest, confidence, version, and scope.
- When facts change, older versions are explicitly invalidated; file modifications, branch switches, project-identity changes, and device side effects invalidate related facts and bump the epoch.
- The token window reserves model output first, then allocates to system, task, project, archive, and recent messages; the Workspace can inspect budget, summary cursors, and fact sources.
- When Context V2 read/write fails, chat continues on the compatibility path; raw messages, Runs, and events remain available for recovery.

See `CONTEXT_V2.md` for detailed data mappings and adjudication priority.

## 5. Goal Contract and Evidence Acceptance

`agent/acceptance.rs` compiles the user's goal into a `GoalContract`. Currently recognized acceptance types:

- modifications actually landed;
- independent verification after modification;
- build;
- test;
- deploy;
- Git commit;
- Git push.

Tool runs are converted into structured evidence. For write operations, verification must occur after the last modification; a plain `read_file` only counts as verification if it covers the modified target, while builds, tests, and Git diff/status can serve as global verifiers.

When acceptance fails, the runtime kernel gives the model remediation hints and continues the tool loop; if evidence is still missing after the remediation budget, the Run is marked `interrupted/continuation_required` instead of masquerading as success.

## 6. Durable Agent Runtime

### 6.1 Run and Events

`agent/runtime.rs` manages `agent_runs` and `run_events`. A Run stores the goal, status, phase, attempt count, event sequence, parent Run, recovery plan, goal contract, lease, acceptance result, and quality snapshot.

Valid states include `queued`, `running`, `waiting_approval`, `waiting_user`, `verifying`, and the terminal states `completed`, `failed`, `cancelled`, `interrupted`. Terminal states are irreversible — a late watchdog or stale worker cannot flip a completed task back to failed.

### 6.2 Execution Steps and Recovery

`coordinator.rs` persists tool calls as execution steps, recording prepared/started/finished states and idempotency keys. `recovery.rs` decides recovery actions by side-effect domain:

- pure reads can be safely retried;
- side effects such as file modifications, commands, and deployments need effect verification first;
- operations that cannot be proven to have taken effect require human confirmation;
- steps with already-trusted results are reused directly to avoid duplicate execution.

Session snapshots are the user-visible "time travel"; Run/step checkpoints are the execution kernel's crash recovery. The two serve different purposes.

## 7. Scheduling, DAG, and Multi-Worker

### 7.1 Durable Queue

`agent/scheduler.rs` manages `agent_task_queue`: priority, max attempts, backoff, concurrency keys, checkpoints, resume tokens, worker Owner, and leases are all persisted.

Workers write heartbeats every 5 seconds and reclaim expired Owners. Claiming a task generates a lease token and an incrementing epoch; subsequent checkpoint, lease-renewal, and terminal-state writes all verify the Owner, forming fencing that prevents stale results from an old process overwriting the new Owner.

### 7.2 DAG

`agent/dag.rs` represents the main Run and sub-agents as nodes; edges support dependency conditions and a `required` flag. Nodes carry independent attempt counts, failure policies, next-attempt timestamps, output summaries, and acceptance results.

Root-task acceptance merges child-node evidence; a sub-agent's natural-language conclusion cannot bypass the root contract.

### 7.3 Multi-Process Recovery

Each desktop process registers a unique `agent_workers` record. On exit the record is marked stopped; after an abnormal exit, other instances only reclaim tasks whose heartbeats expired and leases are invalid — they never steal still-healthy tasks when a second instance starts.

## 8. Tool Execution Kernel

### 8.1 Tool Registration and Protocol

`TOOL_SPECS` in `agent/tools/mod.rs` is the authoritative list of 201 external tools, including name, description, and side-effect markers. Tools support both the textual marker protocol and OpenAI-compatible native function calling; MCP and Skill tools are injected dynamically at runtime.

Tools are limited and measured across eight task domains: build/fix/explore/deploy/refactor/test/debug/other.

### 8.2 Execution Pipeline

Tool execution goes through pre/post hooks: budget, dangerous commands, path boundaries, sensitive-file invariants, permission approvals, task progress, audit, and large-output spill-to-disk. Security boundaries live in Rust and cannot be bypassed by the frontend or the model.

### 8.3 Dedicated Threads and Crash Isolation

`agent/tool_runtime.rs` creates an execution lease for each tool call and runs the actual call on a named OS thread. Thread identity is registered in `tool_execution_workers`; panics are isolated by `catch_unwind`, ending only that execution thread without taking down the desktop process.

When the caller times out or cancels and the thread has not exited, it is marked stuck; a background scan also detects lease-expired calls. `stuck_count` and `stuck_tools` are exposed on the reliability control plane.

### 8.4 Idempotency and Side-Effect Recovery

Tool calls use stable idempotency keys and lease tokens. Result commits verify the current Worker Owner; stale results are discarded by fencing. For side-effecting tools found in prepared/running state at crash time, the kernel enters verifying first, then decides reuse, retry, or human confirmation based on structured artifacts and recovery policies.

## 9. Structured Results, Governance, and Observability

`structured_result.rs` wraps traditional text tool output into a V2 envelope containing:

- artifact path and type;
- verification type and scope;
- error classification, error code, and retryability;
- compensation/recovery strategy;
- metrics such as duration and output size;
- a stable evidence digest.

`governance.rs` derives dynamic tool-rounds, max duration, remediation count, lease, and model fallback policy from goal complexity, and produces a quality score at the terminal state.

`enterprise.rs` provides local-tenant SLOs, alerts, audit, and quota accumulation; `evals.rs` runs 16 execution-kernel reliability scenarios and 10 fixed HarmonyOS task scenarios, writing per-scenario expected/actual results plus schema-v1 execution snapshots (model/tool/SDK/device/token/cost/evidence summary) into evaluation history; `versioning.rs` aggregates the current versions and compatibility commitments of the database, tool protocol, Skill/workflow specs, knowledge index, and evaluation schema. The cost page, via `commands/reliability.rs`, displays:

- Run states and acceptance rates;
- scheduler queues, recovery tasks, and DAG nodes;
- Agent Workers and Tool Workers;
- tool stuck, failure, and recovery stats;
- SLOs, alerts, audit, and recent evaluations.

## 10. HarmonyOS Capability Layer

HarmonyOS capabilities are spread across `services/harmony*.rs`, `agent/tools/build_tools.rs`, `device_tools.rs`, `debug_tools.rs`, and `project_tools.rs`:

- probes DevEco Studio, HarmonyOS SDK, command-line-tools, JDK, Node, and Git;
- merges project manifests, ArkTS/ArkUI, API imports, and build/crash logs into a HarmonyOS fingerprint with confidence levels and relative-source evidence, reused by project understanding and capability-pack selection;
- scans HAP/HAR/HSP modules, bundleName, API versions, pages, and signing configuration;
- invokes hvigor/ohpm builds and parses errors and artifacts;
- manages devices via hdc — install, launch, screenshot, logs, performance, and files;
- starts the ArkTS language server providing definition/references/symbols/hover/diagnostics;
- queries the built-in SDK APIs, official docs, version diffs, and compatibility;
- browses the ohpm landscape and provides dependency recommendations.

Detailed rules are in `HARMONY_INTEGRATION.md`; fingerprint boundaries and fixed evals are in `FIXED_EVALUATION_SUITE.md`.

## 11. Providers, Proxy, and Model Protocols

Providers and models are stored in SQLite; API keys are managed via the system keychain. A session can choose the model, protocol, sampling parameters, reasoning effort, whether to use the proxy, and whether to enable native tool calling.

The local proxy handles request forwarding, circuit breaking, automatic failover, retries, cost, and request logging. Under multiple instances, `ProxyLock` guarantees that only one instance holds the proxy port while the others share it.

The model layer supports OpenAI, Anthropic, and Gemini request/streaming-response formats; tool protocols, reasoning content, and image inputs are converted per protocol.

## 12. Data and Storage

SQLite uses WAL and foreign-key constraints; migrations run sequentially at startup. The current 77 migrations cover:

- providers, models, proxy, cost, and request logs;
- projects, conversations, messages, references, tags, feedback, and versions;
- tool runs, events, task ledgers, snapshots, and reminders;
- Skills, MCP, knowledge base, API docs, ohpm landscape;
- Durable Runs, execution steps, recovery plans;
- scheduler queues, DAGs, workers, attempt ledgers;
- SLOs, alerts, audit, quotas, and tool execution workers.
- fixed-eval results, versioned execution snapshots, environment/resource metadata, and final evidence summaries.
- long-session Context V2 state, sourced facts, artifact references, and summary checkpoints.
- persistent lifecycles for pending approvals, plan reviews, and Agent questions, plus orphan-recovery evidence.
- post-compaction reconciliation between model summaries and structured facts, conflict codes, and correction audits.

Published migrations must not be modified; database invariants reject rewriting existing `migrations/*.sql`, and new structures require new incrementing-numbered migrations.

Other local data includes logs, the symbol cache, spilled large outputs, bundled runtimes, and the seed knowledge base. Sensitive Provider keys are never stored in plain config or plaintext SQLite fields.

## 13. MCP, Skills, and LAN

- **MCP**: server CRUD, import/export, testing, long-running stdio clients, tool discovery, and call forwarding are implemented. The Agent only loads instances precisely bound to the current project and authorized; global configuration serves purely as templates, and tool/directory/network/credential whitelists are re-verified at both discovery and invocation, with connection-config changes invalidating authorization. Detailed boundaries in [MCP Project Authorization and Scope](MCP_PROJECT_AUTHORIZATION.md).
- **Skills**: local Skill management, GitHub import, enable/disable, cloning, and usage stats are supported; enabled content is injected into the Agent context per project.
- **LAN**: a built-in Hyper service with a standalone native Web UI provides token auth, project/session management, message sending, SSE updates, and read-only file access within the project. LAN never exposes arbitrary file writes, commands, or device-control interfaces.

## 14. Startup and Shutdown

The Tauri setup completes, in order:

1. registers tool guards;
2. initializes logging, SQLite, migrations, and the global DB;
3. starts Run/Tool Worker heartbeats and expired-recovery scans;
4. imports the seed API knowledge base and refreshes ohpm data on demand;
5. starts the reminder scheduler;
6. initializes the tray, shortcuts, MCP manager, and bundled Node/JDK/Git;
7. probes the HarmonyOS toolchain;
8. auto-starts the local proxy and LAN service per configuration.

On exit, current workers are stopped, MCP child processes are reclaimed, and the lock holder stops the proxy and LAN services.

## 15. Quality Gates

`.github/workflows/quality.yml` runs on macOS and Windows:

- `npm test`, ESLint, frontend production build;
- `cargo test --locked` and Clippy;
- Agent reliability gate;
- fixed-eval CI baseline gate (`ci_baseline_gate`): saves/restores cross-machine-comparable baselines, blocking significant regressions in task completion rate, eval coverage, or critical latency; the main branch saves baselines while PRs only compare;
- Execution Kernel module tests;
- multi-process Worker crash-recovery E2E;
- Tool Worker crash and side-effect recovery E2E.

The basic principle of reliability design: **model output is not fact, persisted state is not decoration, side effects must not be blindly replayed, and completion must have evidence.**

## 16. Current Structural Risks

The following are real code risks, not unimplemented features:

1. `commands/chat.rs` simultaneously handles protocol, context, the tool loop, recovery, and persistence — the file is too large and should later be split without breaking state-machine boundaries;
2. `pages/Home.tsx` remains large; although several chat components were extracted, layout and interaction state are still highly concentrated;
3. `agent/tools/mod.rs` simultaneously carries 201 schemas and total dispatch — adding a tool requires synchronized verification of registration, permissions, grouping, and structured results;
4. the capability-batch versions in README/CHANGELOG and the app manifest `2.0.0` are not the same cadence; the official versioning policy should be unified before release;
5. bundled runtime/resources are not distributed with Git, so a clean clone can only run tests and lean builds that do not depend on these resources.

Maintenance should prioritize five invariants: terminal states irreversible, Owner fencing, migrations append-only, tool evidence traceable, and frontend/backend event idempotency.
