# DevEco Switch

> **Agent Workspace for HarmonyOS Developers** — an all-in-one desktop AI coding workbench

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-blue)]()
[![Tauri](https://img.shields.io/badge/Tauri-2.x-orange)]()
[![License](https://img.shields.io/badge/license-MIT-green)]()

[简体中文](README.md) | English

A desktop AI coding IDE for HarmonyOS / OpenHarmony developers. It packs multi-provider routing, an Agent toolchain, HarmonyOS project understanding, device debugging, an API knowledge base, and a local proxy circuit breaker into one native app, so the model truly "understands" HarmonyOS projects and can get real work done.

## What It Is

Not just a simple provider switcher. **201 Agent tools** cover the full HarmonyOS development loop — from scaffolding a project to crash attribution, from code scanning to on-device deployment:

| Dimension | Capability |
|------|------|
| 🤖 **AI Agent Core** | Rust-backed multi-turn tool loop, sub-agent spawning, task planning, TodoWrite, undo stack, proactive questioning, failure reflection, and evidence-driven acceptance |
| 📱 **Deep HarmonyOS Integration** | hdc device management / wireless on-device connection / emulator start-stop / hvigor build / ohpm dependencies / faultlog crash attribution / real-time hilog streaming / multi-module workspace detection |
| 🔌 **Multi-Provider Routing** | Multiple LLM providers (Huawei, Zhipu, Qwen, etc.) + local HTTP proxy + circuit breaker + automatic failover + cost tracking + request logging |
| 📚 **API Knowledge Base** | Built-in HarmonyOS API index (vector retrieval + symbol index) + cross-version diff + compatibility scan + user notes (knowledge entries) |
| 🛡 **Security & Reliability** | Tool whitelist / limits / budget / approval pipeline + goal contract / durable queue / DAG / worker lease / crash recovery / SLO & audit |
| 📦 **Bundled Runtimes** | Portable Node + JDK + Git bundled with the installer (downloaded automatically from official sources by CI at build time) — no dev environment pre-installation required on the user's machine |
| 💬 **Session Management** | Multi-session / context compaction / LLM call replay (`llm_replay`) / event sourcing (`session_events`) / session tags / pinning / message queue / task watchdog (auto-abort on hang) / **session time travel (snapshot rollback)** / **cross-session references (@ session)** / **scheduled reminders (schedule)** |
| 🧠 **Code Understanding** | LSP semantic analysis (ArkTS language server) + tiered scanning (`check_code` / `deep_scan` / `codebase_search` / `get_symbol_details`) / symbol index / filesystem toolset |
| 🌐 **Ecosystem Capabilities** | MCP server management / Skill enable-disable & usage stats / ohpm ecosystem panel / HarmonyOS official doc search / web search & fetch / knowledge base import-export |
| 📡 **LAN Access** | Built-in LAN server — view sessions, send messages, and manage sessions from a phone/tablet browser (token auth + read-only file viewing) |

## Core Features

### 1. A Real AI Agent, Not a Chatbot

- **Parallel sub-agents**: `spawn_agents` spawns independent subtask runs (up to 50 run records), with agents collaborating over a message board (pub/sub)
- **Task planning**: `plan_task` breaks complex tasks into steps, rendered live in the frontend (todo → doing → done/failed)
- **Proactive questioning**: `ask_user` pauses the agent flow to wait for your answer (oneshot channel, auto-cancelled on stop)
- **Undo stack**: `undo_edit` snapshots the agent's `edit_file`/`write_file` operations (up to 40 entries per session, FIFO eviction)
- **Cross-turn diagnostics**: root-cause conclusions for build/deploy/crash issues are cached per project and injected into the system prompt automatically, so the model stops "stepping on the same rake"; memory injection comes with **BM25 relevance ranking + recently-updated-on-top (front_page) + negative-feedback bag-of-words correction** (downvoted content stops reappearing)
- **Failure reflection**: after tool failures, reflection fragments are automatically distilled and injected into the next system prompt, letting the agent remember its own failure patterns
- **Time travel**: a session snapshot (message anchors + ledger + summary) is saved after every tool execution round — jump back to any historical decision point and re-steer (aligned with langgraph checkpoints)
- **Scheduled reminders**: `schedule_create` (after/at/every) sets in-session reminders, delivered to the conversation and as desktop notifications when due
- **Background tasks**: `run_command --background` returns a `job_id` immediately for long-running tasks; the summary is injected into the next request on completion
- **Runtime logging**: after deployment, `hdc shell hilog -L E` is monitored automatically; anomalies are logged as diagnostics → frontend events

### 2. HarmonyOS Project Capabilities

- **Multi-module detection**: `har / hsp / haps` modules auto-scanned, workspace module tree rendered
- **On-device debugging**: `list_devices` / `connect_device` / `manage_hdc` / `start_emulator` / `device_file` / `device_shell` / `attach_debugger` / `step_debug`
- **Build & deploy**: `build_project` (hvigorw assembleHap) / `deploy` / `deploy_all` / `analyze_hap_size` / `ota_pack` (.pkg packaging)
- **Crash attribution**: `analyze_crash` scans faultlog and attributes 7 structured root-cause classes (JsError / CppCrash / startup timeout, etc.)
- **API knowledge base**: `search_sdk_api` / `read_sdk_api_module` / `search_harmony_docs` / `diff_api_versions` / `scan_api_compat`
- **ohpm ecosystem panel**: `ohpm_search` / `ohpm_recommend` + frontend ecosystem browser (category / rating / downloads / dependency tree)
- **Environment probing**: `environment_check` / `get_env_info` / `check_sdk_alignment` / `get_installed_apps`

### 3. LSP Semantic Code Understanding

No guessing via "text scanning" — it directly launches `@arkts/language-server` over stdio JSON-RPC, the same ArkTS analysis engine DevEco Studio uses:

- `lsp_definition`: jump to definition (incl. SDK `.d.ts` and cross-module)
- `lsp_references`: global references
- `lsp_symbols`: document symbols (structs / classes / state variables)
- `lsp_hover`: hover docs and API descriptions
- `lsp_diagnostics`: live type/syntax diagnostics (incremental delivery by file model)

SDK path auto-detection: `DEVECO_SDK_HOME` → DevEco Studio install path → user directory `Huawei/Sdk`.

### 4. Multi-Provider + Local Proxy

- Not bound to any LLM vendor
- Local HTTP proxy + circuit breaker + automatic failover
- Daily cost stats / request logs / token usage tracking
- Visual provider configuration editing, easy import/export
- Only one proxy instance starts across multiple app windows (ProxyLock holder mechanism)

### 5. Security First

- **Tool whitelist**: dangerous tools (deploy/build/network) are whitelisted per project
- **Approval pipeline**: pre/post hooks (`pipeline.rs`) intercept sensitive operations
- **Task guard**: `task_guard` prevents runaway agents
- **Budget control**: `budget` / `cost_guard` modules cap per-run and daily costs
- **Tool limits**: `tool_limits` caps invocations per 8 task groups (build / fix / explore / deploy / refactor / test / debug / other) — hot tools are no longer throttled globally
- **Permission management**: `permissions` module tiers tools by type

### 6. Evidence-Driven Reliable Execution

- **Goal contract**: required conditions (modify, verify, build, test, deploy, commit, push, etc.) are extracted from the user's goal; the model can only *claim* completion while the runtime kernel adjudicates against real tool evidence
- **Durable Run**: task state, phases, event cursors, execution steps, checkpoints, and acceptance results are written to SQLite — the true final state is still determinable after a WebView refresh or process crash
- **Durable scheduling & DAG**: the task queue supports priority, leases, retries, recovery tokens, and concurrency keys; main tasks and sub-agents are recorded as DAG nodes with dependencies, failure policies, and acceptance results
- **Multi-worker anti-race**: desktop processes arbitrate write authority via heartbeats, lease tokens, and fencing — stale results from expired workers can never overwrite the current owner
- **Tool execution isolation**: tool calls run on dedicated OS threads with panics isolated; side-effecting tools recover via idempotency keys, prepared/committed states, and verification policies, avoiding duplicate execution after crashes
- **Reliability control plane**: the cost page visualizes Runs, queues, workers, tool executors, hung calls, and SLO; built-in fault-scenario evals and process/thread crash E2E gates

### 7. LAN Access

Built-in HTML server (default `http://<local-IP>:12345/`), usable directly from phone/tablet/desktop browsers:

- Browse session lists, view messages, send new messages, manage sessions (create/archive/pin/delete/clear)
- **Token auth**: each device/visitor gets a 6-digit token, with notes, expiry, enable/disable, and failed-login trace
- **Read-only file viewing**: `read_project_file` only exposes text files ≤5MB inside the project; no write/delete/move operations are registered on the LAN routes
- See [docs/LAN_ACCESS.md](docs/LAN_ACCESS.md) for details

### 8. Productivity Tools

- **Command palette**: `Cmd/Ctrl+K` opens 28 high-frequency tool actions for instant triggering (debug/refactor/build/security/knowledge/data/governance/multimodal)
- **@ references**: type `@` in the input to reference project files (MRU-ordered) or **other sessions in the same project** (`conv:` prefix injects title + summary)
- **Session tags & pinning**: tag sessions and pin frequently-used project sessions
- **Timeline panel**: session event-sourcing visualization (`session_events`)
- **Notification center**: agent task completion/failure push notifications
- **Performance monitoring**: PerfMonitor tracks rendering and IPC performance in real time
- **Audit logs**: full traces of tool calls and permission approvals

## Example Workflow

**From 0 to 1: let the agent scaffold a HarmonyOS project and deploy it to a real device**

```
1. User: create a HarmonyOS Stage project in testhy
2. Agent breaks down plan: create_harmony_project → write AppScope → write entry → ohpm_install → build_project → deploy
3. Tool flow: todo_write → write_file × 7 (hvigor/oh-package/build-profile/AppScope/entry...) → ohpm_install → build_project
4. Failure self-healing: build fails → show_diagnose_card(category=type) → edit_file fix → rebuild
5. Deploy: deploy → start_ability → read_runtime_logs
6. Anomaly capture: hilog detects TypeError → auto-diagnostics → agent proactively fixes
```

## Technical Architecture

```
┌─────────────────────────────────────────────────────┐
│  React 19 + TypeScript + Tailwind 4 + Vite 8        │
│  - i18next (EN/zh/auto) + react-markdown + katex    │
│  - Zustand store: project / theme / chat / memory   │
│  - 14 pages (Home workspace + 13 admin pages)       │
└─────────────────────────────────────────────────────┘
                        │ Tauri IPC
┌─────────────────────────────────────────────────────┐
│  Rust (Tauri 2 + hyper + rusqlite + tokio)          │
│  - 298 Tauri IPC entry points · 56 service modules  │
│  - agent/ 36 top-level modules · tools/ 29 files    │
│  - SQLite + 77 migrations · full event sourcing for │
│    runs/steps/tools                                 │
│  - Bundled runtimes: Node + JDK + Git (runtime/)    │
└─────────────────────────────────────────────────────┘
```

### Module Map

```
src-tauri/src/
├── agent/                  # AI Agent core (36 top-level modules)
│   ├── runtime.rs           #   - Durable Run state machine & event cursors
│   ├── scheduler.rs         #   - Durable queue, worker leases & fencing
│   ├── coordinator.rs       #   - Execution steps & recovery checkpoints
│   ├── context.rs           #   - Long-session tiered context, fact sourcing & summary cursors
│   ├── recovery.rs          #   - Side-effect-aware recovery plans & verification requirements
│   ├── acceptance.rs        #   - Goal contract & tool-evidence acceptance
│   ├── governance.rs        #   - Dynamic budget, reliability policies & quality snapshots
│   ├── dag.rs               #   - Main/sub-agent DAG & dependency scheduling
│   ├── tool_runtime.rs      #   - Tool workers, dedicated threads, leases & idempotency
│   ├── structured_result.rs #   - Tool result V2, artifact/verification/compensation evidence
│   ├── enterprise.rs        #   - SLO, alerts, audit & quotas
│   ├── evals.rs             #   - Reliability scenario evals & fault injection
│   ├── ask.rs               #   - Proactive questioning (oneshot channel)
│   ├── jobs.rs              #   - Background tasks (kill_tree + 512KB output ring)
│   ├── subagents.rs         #   - Sub-agent spawning (latest 50)
│   ├── agent_board.rs       #   - Agent message board (A2A pub/sub)
│   ├── reflexion.rs         #   - Failure reflection (tool-level lesson injection)
│   ├── lsp_client.rs        #   - ArkTS LSP client (stdio JSON-RPC)
│   ├── todo.rs              #   - Task lists (in-memory + DB dual write)
│   ├── undo.rs              #   - Undo stack (40 entries per session)
│   ├── scanner.rs           #   - Tiered code scanning
│   ├── diagnostics.rs       #   - Cross-turn diagnostic memory
│   ├── crash.rs             #   - Crash attribution (JsError / CppCrash / 7 classes)
│   ├── runtime_log.rs       #   - Device runtime log ring buffer
│   ├── exec_ctx.rs          #   - Tool execution context (stop flags)
│   ├── session_ctx.rs       #   - Session-level runtime state (converged)
│   ├── invariants.rs        #   - Write invariants (.env / certs / migration SQL)
│   ├── session_events.rs    #   - Session event sourcing
│   └── tools/               #   - 201 Agent tools (29 files)
│       ├── mod.rs               # Tool registry (TOOL_SPECS) + protocol dispatch
│       ├── protocol.rs          # Tool-call marker parsing
│       ├── errors.rs            # Structured error envelope (7 ToolError classes)
│       ├── pipeline.rs          # pre/post hooks
│       ├── guards.rs            # Hook implementations
│       ├── fs_tools.rs          # Filesystem (115KB)
│       ├── ui_tools.rs          # UI automation
│       ├── build_tools.rs       # hvigor build
│       ├── device_tools.rs      # Device debugging
│       ├── test_tools.rs        # Testing
│       ├── explore_tools.rs     # Exploration
│       ├── project_tools.rs     # Project
│       ├── compose_tools.rs     # Composite tools
│       ├── meta_tools.rs        # Meta tools
│       ├── skill_tools.rs       # Skills
│       ├── debug_tools.rs       # Debugging
│       ├── cmd_tools.rs         # Commands
│       ├── memory_tools.rs      # Memory
│       ├── git_tools.rs         # Git
│       ├── doc_tools.rs         # Documents
│       ├── media_tools.rs       # Multimodal
│       ├── web_tools.rs         # Web
│       ├── quality_tools.rs     # Quality gate facade
│       ├── quality_metrics.rs   #   Quality metrics (7 tools)
│       ├── quality_security.rs  #   Security scanning (4 tools)
│       ├── quality_runtime.rs   #   Runtime quality (6 tools)
│       ├── quality_media.rs     #   Media quality (2 tools)
│       └── schedule_tools.rs    # Scheduled reminders (schedule_create/list/delete)
├── commands/               # 38 command modules (298 IPC registration entry points total)
├── services/               # Business services (56)
│   ├── proxy_service.rs    #   - Local proxy
│   ├── circuit_breaker.rs  #   - Circuit breaker
│   ├── model_router.rs     #   - Model routing
│   ├── embedding.rs        #   - Vector embedding (GPU-first auto fallback)
│   ├── sdk_api.rs          #   - SDK API index
│   ├── lan_server.rs       #   - LAN access service
│   ├── ohpm_landscape.rs   #   - ohpm ecosystem data
│   ├── agent_limits.rs     #   - Agent limits (by task group)
│   ├── tool_cache.rs       #   - Tool result cache
│   ├── reminders.rs        #   - Scheduled reminder dispatch (30s polling)
│   ├── harmony_*.rs        #   - HarmonyOS integration (6 files)
│   └── ...
├── db/                     # SQLite + 68 sequential migrations
├── utils/                  # Utilities (13 files, incl. task watchdog)
├── tray/                   # System tray
└── runtime/                # Bundled Node + JDK + Git (~700MB, not committed — see below)
```

> **About large files**: `src-tauri/runtime/` (portable runtimes), `src-tauri/resources/` (seed knowledge base + embedding models, ~340MB) and `portable-build/` (portable build artifacts) total ~1GB. They are build artifacts / downloaded resources and are **not distributed with the Git repository** (see `.gitignore`). Keep these directories for local builds; users cloning the repo can obtain the full runtime from the Release installer, or prepare it themselves following the download logic in [release.yml](.github/workflows/release.yml).

## The 201 Agent Tools Grouped by Domain

| Domain (TOOL_GROUP) | Representative tools |
|------|------|
| **build** | `create_harmony_project` `build_project` `ohpm_install` `ota_pack` `analyze_hap_size` |
| **fix** | `edit_file` `multi_edit` `undo_edit` `show_diagnose_card` `analyze_crash` |
| **explore** | `read_file` `list_dir` `find_files` `grep_files` `codebase_search` `get_symbol_details` |
| **deploy** | `deploy` `deploy_all` `connect_device` `list_devices` `device_file` `device_shell` |
| **refactor** | `deep_scan` `check_code` `lsp_definition` `lsp_references` `lsp_symbols` `lsp_hover` `lsp_diagnostics` |
| **test** | `run_tests` `write_unit_tests` `api_test` `api_mock` `api_health` |
| **debug** | `attach_debugger` `step_debug` `log_query` `read_logcat` `search_hilog` `memory_snapshot` `dump_battery` |
| **other** | `web_search` `web_fetch` `http_request` `save_memory` `search_knowledge` `spawn_agents` `plan_task` `ask_user` `license_check` `vuln_scan` `docx_read` `audio_transcribe` `memorize` `ui_focus` `schedule_create` `schedule_list` `schedule_delete` |

For the full list, see the `TOOL_SPECS` array in `src-tauri/src/agent/tools/mod.rs` (with bilingual descriptions and `side_effect` annotations).

## Installation

Download the installer from [Releases](https://github.com/lookapu/HarmonyAgent/releases):

- **Windows**: `.exe` (NSIS installer) or `.msi`
- **macOS**: `.dmg` or `.app.tar.gz`

### First Launch on macOS

The app is unsigned, so macOS will block it. One terminal command fixes it:

```bash
xattr -cr "/Applications/DevEco Switch.app"
```

Or: System Settings → Privacy & Security → scroll to the bottom → click "Open Anyway"

## Building from Source

```bash
# Install frontend dependencies from the lockfile
npm ci

# Development mode (hot reload)
npx tauri dev

# Production build (requires local src-tauri/runtime & src-tauri/resources — see below)
npx tauri build
```

> **Bundled runtimes note**: portable Node / JDK / Git (~700MB), plus the knowledge-base seed and embedding models (~340MB), are not distributed with the repository.
> For local builds, keep the local `src-tauri/runtime/` and `src-tauri/resources/` directories; CI downloads the runtimes automatically from official sources (see [release.yml](.github/workflows/release.yml)).
> Without these directories, the bundled environments (Node/JDK/Git), API knowledge base, and vector retrieval features are unavailable — all other features remain unaffected.

### System Requirements

- **Build machine**: Rust stable, Node 22 (matching the CI baseline), Tauri 2 system dependencies; packaging the full version also requires `src-tauri/runtime/` and `src-tauri/resources/` (~1GB)
- **Runtime machine**: Windows 10+ / macOS 11+ / Ubuntu 22.04+

## Documentation

- [Continuous evolution roadmap](docs/ROADMAP.md) — phased tasks and acceptance criteria for long sessions, the Agent toolchain, the HarmonyOS loop, and ecosystem integration
- [Official DevEco CLI MCP integration](docs/DEVECO_CLI_MCP_INTEGRATION.md) — built-in MCP templates, command parsing enhancements, and the division of labor with custom tools
- [Long-session context V2](docs/CONTEXT_V2.md) — data mapping, fact priority, budget, and compatibility strategy
- [Architecture doc v2](docs/ARCHITECTURE.md) — product positioning, module boundaries, design trade-offs
- [LAN access guide](docs/LAN_ACCESS.md) — enabling the LAN server, token management, and security boundaries
- [Tool enhancement list](docs/TOOL_ENHANCEMENTS.md) — tool capability evolution and fulfillment status
- [Harness enhancement list](docs/HARNESS_ENHANCEMENTS.md) — capability alignment records against the external reference repository
- [Changelog](CHANGELOG.md) — version changes, migration notes, and rollback guidance

## Development Guide

- Frontend entry: `src/App.tsx` + `src/pages/Home.tsx` (Agent Workspace main UI)
- Backend entry: `src-tauri/src/lib.rs` + `src-tauri/src/main.rs`
- Agent tool registration: the `TOOL_SPECS` array in `src-tauri/src/agent/tools/mod.rs`
- Database migrations: `src-tauri/migrations/` (68 at the time of writing; executed migrations must never be modified — add new ones with incremented numbers)
- Legacy debug scripts: `scripts/legacy/` (archived only, do not reference)

## Support

If HarmonyAgent helps you, donations are welcome — maintaining an open-source project is hard work:

<p align="center">
  <img src="docs/alipay-qr.jpg" alt="Alipay donation QR code" width="200" />
  <img src="docs/wechat-qr.jpg" alt="WeChat Pay donation QR code" width="200" />
</p>

<p align="center">Alipay &nbsp;·&nbsp; WeChat Pay</p>

## License

MIT

---

**To developers**: this is an extremely workflow-dense desktop application — the frontend, Rust, and the HarmonyOS toolchain each have their own deep pitfalls. We recommend running `npx tauri dev` to experience the Agent Workspace first, then diving into a specific module as needed.
