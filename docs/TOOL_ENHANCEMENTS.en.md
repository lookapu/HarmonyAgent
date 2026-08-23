# DevEco Switch Tool Capability Inventory

> Status: 2026-08-21, `main` branch. Historical requirement sources are in `tool-enhancement-backlog.txt`; this file only describes capabilities that have actually landed, their fact sources, and boundaries still explicitly deferred.

[简体中文](TOOL_ENHANCEMENTS.md) | English

## 1. Authoritative Baseline

| Item | Current value | Authoritative source |
|---|---:|---|
| External Agent tools | 201 | `agent/tools/mod.rs::TOOL_SPECS` |
| Tool implementation files | 29 | `src-tauri/src/agent/tools/*.rs` |
| Task groups | 8 | `TOOL_GROUP` / `TASK_GROUPS` |
| Permission levels | L0/L1/L2 | `services/permissions.rs` and tool hooks |
| Tool protocol | Textual markers + native function calling | `protocol.rs` / `commands/chat.rs` |

`TOOL_SPECS` is the single source of truth for tool names, descriptions, and side-effect markers; this file does not duplicate the full 201-item array, to avoid a second list drifting when tools are added.

## 2. Eight Task Domains

| Domain | Purpose | Representative tools |
|---|---|---|
| `build` | Project scaffolding, dependencies, build & artifacts | `create_harmony_project`, `ohpm_install`, `build_project`, `ota_pack`, `analyze_hap_size` |
| `fix` | Modification, undo, diagnostics, and fixing | `edit_file`, `multi_edit`, `undo_edit`, `show_diagnose_card`, `analyze_crash` |
| `explore` | File, codebase, knowledge, and API exploration | `read_file`, `list_dir`, `grep_files`, `codebase_search`, `search_sdk_api` |
| `deploy` | Device connection, install, and launch | `list_devices`, `connect_device`, `deploy`, `deploy_all`, `start_ability` |
| `refactor` | Scanning, symbols, and LSP semantic operations | `deep_scan`, `check_code`, `lsp_definition`, `lsp_references`, `lsp_rename` |
| `test` | Unit tests, smoke, API/UI/performance verification | `run_tests`, `write_unit_tests`, `smoke_test`, `api_test`, `run_ui_flow` |
| `debug` | Logs, debugger, performance & memory | `attach_debugger`, `step_debug`, `log_query`, `memory_snapshot`, `dump_battery` |
| `other` | Git, Web, MCP, Skill, memory, and governance | `git_diff`, `web_fetch`, `use_skill`, `spawn_agents`, `schedule_create`, `license_check` |

Task groups are shared by limits, cost stats, permission display, and the command palette; the frontend should not maintain a second mapping.

## 3. Implementation Modules

`src-tauri/src/agent/tools/` is split by responsibility:

- `mod.rs`: `TOOL_SPECS`, grouping, schemas, and total dispatch;
- `protocol.rs`: textual tool-marker parsing;
- `contracts.rs`: tool schema/contract helpers;
- `pipeline.rs` / `guards.rs`: pre/post hooks and approval/budget/security guards;
- `errors.rs`: structured errors and suggestions;
- `fs_tools.rs` / `explore_tools.rs`: file read/write, search, code scanning;
- `cmd_tools.rs` / `build_tools.rs` / `test_tools.rs`: commands, builds, and tests;
- `device_tools.rs` / `debug_tools.rs` / `ui_tools.rs`: device, debugging, and UI automation;
- `project_tools.rs` / `compose_tools.rs`: project analysis and composite workflows;
- `git_tools.rs` / `web_tools.rs`: Git and networking;
- `memory_tools.rs` / `skill_tools.rs` / `meta_tools.rs`: memory, Skills, Agent meta capabilities;
- `doc_tools.rs` / `media_tools.rs`: documents and multimodal;
- `quality_tools.rs`: quality-tool facade; concrete implementations split into the four files metrics/security/runtime/media;
- `schedule_tools.rs`: in-session reminders.

External modules should access quality tools through the facade or total dispatch, never coupling directly to the `quality_*` internal files.

## 4. Landed Capabilities

### 4.1 Files and Editing

- project-root path constraints and canonical path validation;
- `.gitignore`-aware directory, glob, grep, and codebase search;
- `write_file`, `edit_file`, `multi_edit`, copy/move/delete, diff preview, and dry-run;
- session-level undo snapshots;
- invariant protection for `.env*`, keys/certificates, and already-existing migration SQL;
- large-file chunked reads, long-comment folding, and oversized-output spill to disk.

### 4.2 HarmonyOS Projects and Builds

- Stage project scaffolding, HAP/HAR/HSP module detection, and workspace scanning;
- hvigor/ohpm invocation, generic project builds, build-error parsing, and dependency diagnostics;
- HAP size analysis, version size diffs, signing checks/diagnostics, and OTA `.pkg` packaging;
- HarmonyOS SDK alignment, API compatibility scans, and cross-version API diffs.

### 4.3 Devices, Debugging, and UI

- hdc service, wireless devices, emulators, app install/launch/stop/uninstall;
- shell, device files, screenshots, screen recording, UI hierarchy, gestures, and UI flows;
- hilog/runtime log/faultlog queries and crash classification;
- debugger attach, step/next/continue/where, and other debug actions;
- CPU/memory/battery/performance sampling and memory-snapshot diffs;
- network conditions, Wi-Fi, airplane mode, and permission settings.

### 4.4 Code Understanding and Knowledge

- ArkTS LSP definition/references/rename/format/code action/completion/signature/hover/diagnostics/symbols;
- symbol indexes, tiered scanning, code metrics, and change reviews;
- SDK APIs, official HarmonyOS docs, user knowledge base, and the ohpm landscape;
- hybrid retrieval with BM25 + embedding, RRF, front-page pinning, and negative-feedback correction.

### 4.5 Testing, Quality, and Security

- unit-test generation/execution, flaky detection, smoke tests, UI flows, and performance baselines;
- OpenAPI testing, mocking, and health checks;
- license checks, vulnerability scans, code obfuscation, and sandboxed execution;
- screenshot diffs, quality metrics, log aggregation, trace replay, and metric export;
- output redaction, dangerous-command blacklists, tool caching, tool health checks, and stats.

### 4.6 Agent Meta Capabilities

- `plan_task`, Todos, proactive questioning, diagnostic cards, and progress updates;
- `spawn_agents`, tool filters, max depth, personas, and the Agent message board;
- background jobs, completion-message injection, and kill-tree;
- memory save/search, Reflexion, time travel, and session references;
- MCP service discovery/invocation, Skill invocation, web search/fetch, and session reminders.

## 5. Tool Execution Security and Reliability

Tools never go straight from a model string into an implementation function; they pass through a unified execution chain:

```text
Model tool call
  → schema/parameter parsing
  → project path & invariants
  → task budget/limits/dangerous commands
  → permission level & user approval
  → execution step + tool lease + idempotency
  → dedicated OS thread execution
  → structured result/evidence/compensation info
  → final state commit after Owner fencing
  → audit, progress, cache, and large-output spill
```

Key semantics:

- consecutive L0 reads without interactive side effects can run at most 4-way concurrent; write tools are a serial barrier;
- a tool future panicking inside its thread only fails the current call;
- caller timeout/cancel with the thread still running marks it stuck;
- read-type tools can be safely retried after crashes;
- side effects like modifications, commands, and deployments verify actual effects first — never blindly replay;
- duplicate side effects are prevented by idempotency keys;
- stale results from an old Tool Worker cannot overwrite the new Owner;
- human-readable text output is retained alongside; the structured V2 envelope provides machine-readable evidence for acceptance and recovery.

Detailed execution kernel in `ARCHITECTURE.md`.

## 6. Historical Enhancement Batches

| Date | Change |
|---|---|
| 2026-08-14 | Initial version with 117 tools covering files, builds, devices, knowledge, and the basic Agent loop |
| 2026-08-16 | Expanded to 191 tools: log queries, docs/audio, debugging, memory, OTA, license, and vulnerability scans; quality modules split out |
| 2026-08-20 | Grew to 198: `memorize`, `ui_focus`, `schedule_create/list/delete`, plus time travel, hybrid retrieval, and loop detection |
| 2026-08-21 | Tool count unchanged; added evidence contracts, durable scheduling, DAG, multi-worker, Tool Execution Kernel, dedicated execution threads, and the reliability control plane |
| 2026-08-22 | Grew to 201: `workflow_template`, `team_share`, `reproduction_bundle`, landing with the workflow-governance, team-sharing, and reproduction-bundle batches |

The tool count only describes external capability numbers; the 2026-08-21 focus was making existing tools recoverable under crashes, timeouts, multiple instances, and side-effect scenarios rather than adding more tool names.

## 7. Explicitly Deferred Items

The following external-service integrations are not part of the current 201 tools and remain deferred:

- Figma import;
- Feishu task sync;
- Jira sync.

Reason: they require external accounts, tokens, permission models, and long-term API compatibility maintenance, and they do not affect the local HarmonyOS development loop. If restarted, connection credentials, permission boundaries, failure compensation, and audit requirements should be defined first rather than merely adding a network-call tool.

## 8. Checklist for Adding or Modifying Tools

1. Register name, parameters, returns, and side effects in `TOOL_SPECS`;
2. Add the schema and dispatcher, confirming visibility in both the textual protocol and native tools;
3. Register `TOOL_GROUP`, permission level, and timeout/retry/cost hints;
4. Define the effect kind, idempotent inputs, artifacts, and verification method;
5. Wire in path, invariants, approvals, and output redaction;
6. Add tests for success, parameter errors, permission denial, timeout, and panic/recovery;
7. If new database structures are needed, only add incrementing-numbered migrations — never modify existing ones;
8. Update README/CHANGELOG; do not copy a full tool array here that would drift.

## 9. Verification Baseline

Tool-related changes must at minimum pass:

```bash
npm test
npm run lint
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked
cargo test --manifest-path src-tauri/Cargo.toml --locked --test tool_worker_crash_e2e -- --test-threads=1
```

CI additionally runs Agent reliability, Execution Kernel, and multi-process Worker crash gates. Passing tests only proves that covered invariants did not regress; tools involving real SDKs, devices, signing, and network Providers still require environment-specific acceptance.
