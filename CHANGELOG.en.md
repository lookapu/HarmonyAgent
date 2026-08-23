# DevEco Switch · CHANGELOG

> A desktop AI coding IDE for HarmonyOS / OpenHarmony developers.
> This file records user-visible changes, migration notes, and rollback guidance in reverse version order.

[简体中文](CHANGELOG.md) | English

---

## v2.1.0 — Evidence-Driven Governance & Dual Execution Kernel (2026-08-22)

Positioning: upgrading "the model can call many tools" to "tasks and tools are durably scheduled, acceptable, recoverable, and observable". The reliability & governance batch adds migrations `057`—`062`, `069`—`075`; work continues on long-session Context V2, and the total migration count reaches **77**. The governance batch adds 3 new external tools (`workflow_template`, `team_share`, `reproduction_bundle`), bringing `TOOL_SPECS` to **201**.

- Long-session hardening batch (6 items): budget dynamically profiled by session phase (`balanced/explore/execute/verify`, derived at read time from `agent_runs.phase`); 85%-window compression warning notification chain confirmed (`chat-context-warning` event + frontend notification); context panel adds tiered budget bar, profile label, invalidation count, and session health display (auto-refresh after compaction); 100+ round random-event stress regression test (fact adjudication stays correct after interleaved compaction/invalidation/reconciliation); multi-token fuzzy knowledge retrieval (`search_knowledge_fuzzy`, ranked by hit count + character overlap); session health & summary degradation detection (compaction count/fact-flip rate/reconciliation correction count/budget usage, `get_session_health` + migration 074). See [Long-Session Context V2 Design](docs/CONTEXT_V2.md).
- Long-session correlation strengthening batch (5 items + 1 eval, 2026-08-22): unified phase classifier (`BudgetProfile::from_goal_or_phase`, runtime phase with goal fallback — dynamic budgets actually take effect); context panel shows invalidation-reason details ("why forgotten" entry point); health degradation verdicts derive true depth from the summary-coverage cursor chain (compaction count is only a secondary signal); compaction events written into the session event stream (`context_compress`, replayable in Timeline); core metrics table and eval snapshots add degradation metrics (compaction/flip/warning counts); the fixed eval suite adds a 100-round long-session compaction-recovery case (27 scenarios) included in the baseline gate.

- Fixed evals add schema v1 execution snapshots: record real model/prompt usage state, tool registry summaries, de-pathified SDK versions, hashed device identities, tokens, cost, total duration, and per-case/final evidence summaries; historical runs read compatibly under schema 0.
- CI adds an eval-baseline regression gate: saves/restores cross-machine-comparable baselines, blocking significant regressions in task completion rate, eval coverage, or critical latency; duplicate side-effect and recovery rates continue to be hard-gated by crash-recovery E2Es; the main branch saves baselines while PRs only compare.
- New failure-sample reflow tool: validates reproduction-bundle completeness/redaction state and distills eval scenario drafts, so real failures can become fixed-eval regression scenarios under CI coverage.
- New unified asset-version manifest: current versions, compatibility commitments, and migration notes for the database, tool protocol, Skill/workflow specs, knowledge index, and eval schema are centrally queryable; release notes and verification share the same data source.
- Automated release-note aggregation: generates migration lists, tool-protocol changes, asset versions, risks, and rollback paths from git diffs and the CHANGELOG; the release.yml release flow uses it automatically.
- New documentation-drift gate `scripts/check-docs.py`: extracts tool/migration/IPC/module counts from code truth sources and pattern-compares against the README and architecture doc; validates roadmap-to-docs links, code path references, and tests/scripts referenced by CI workflows; wired into quality.yml dual-platform gates — merges are blocked when docs and implementation diverge.
- Warning convergence & no-new-baseline gate (Q-07): ESLint 9 warnings cleared (real react-hooks fixes and reasoned exemptions, `--max-warnings 0` blocks regressions); Clippy 338 → 44 (two rounds of clippy --fix converging 221 mechanical warnings, batch sort_by_key conversions, fixing suspected real bugs like lockfile-truncation semantics and in-Drop process termination, batch doc-format normalization); the remaining 44 are all structural (31 too_many_arguments + 13 type_complexity) kept as baseline; `scripts/check-warnings.py` deduplicates unique clippy warnings by (lint, location) and is wired into quality.yml — new warnings block immediately.
- Real-device runtime diagnostics loop acceptance passed (5.5 phase acceptance): full loop completed on a Huawei CHZ-AL00 device (HarmonyOS 6.1.0.135) — signed HAP build, install, launch, hilog log baseline, runtime fault injection (JSON parsing anomaly), hilog/hisysevent anomaly location, fix, reinstall, and re-verification; acceptance records and evidence in [HarmonyOS Phase Three Acceptance Records](docs/HARMONY_STAGE5_ACCEPTANCE.md).
- 5.5 phase acceptance fully completed: multi-module project relationships (real project entry+application dual modules: entry points, 124 page routes, permissions, dependencies, and dual-module signed-artifact relationships) and multi-device isolation recovery (real device + emulator: the real device's process was unaffected throughout the emulator uninstall failure, then only the emulator was reinstalled; anti-replay content-hash gate automated tests passed).
- UI state coverage spec & gate (Q-04): all 16 pages explicitly declare `@ui-states` six-state coverage (loading/empty data/partial success/failure/recovery/no-permission); `scripts/check-ui-states.py` validates declaration-to-code-evidence consistency and is wired into CI; new/modified pages without declarations block. Spec and status audit matrix in [UI State Coverage Spec](docs/UI_STATE_COVERAGE.md).

- Established the `agent_harmony_fixed_v3` fixed eval suite: 16 reliability scenarios and 10 HarmonyOS scenarios run uniformly, covering the real project-scaffolding kernel, compile API attribution, cross-module impact, recorded real-device faultlog, mixed projects, and long-session recovery.
- New explainable HarmonyOS fingerprint report: project manifests, ArkTS/ArkUI, `@kit.*` / `@ohos.*`, and build/crash log evidence are wired into `get_project_info` and capability-pack selection, while keeping plain-TypeScript negative examples and the "must not guess exact API level from import style" boundary.

- New problem-reproduction-bundle page and `reproduction_bundle` tool: exports the problem description, project semantic environment, optional session/tool/Run evidence, and in-project text attachments to a local ZIP — never auto-uploaded or shared.
- Before generation, per-item path, size, redaction state, omission reason, and SHA-256 are shown; on confirmation, content is re-collected and bound to a preview summary — content changes require a new preview, and every Agent generation needs fresh explicit approval.
- ZIPs use project-boundary checks, non-overwritable temp files, and per-item manifest summaries, atomically committed after self-validation; generation history stores whole-file summaries so tampering, truncation, or missing entries can be detected again.
- Reproduction bundles reuse the unified field/text redaction and additionally mask the project root and user directory; binaries, out-of-bounds paths, credentials, certificates, keystores, and signing materials are rejected by default. Structured application logs are also written to disk redacted with private permissions.

- New schema-v1 team-sharing package and admin page: import/export project memory, engineering conventions, and fixed eval sets; packages bind source URI, exact revision, SemVer, and normalized summary — same-version content cannot drift.
- Pre-import per-item preview of additions, same-source updates, local conflicts, and unchanged items; conflicts only generate disabled, unconfirmed team copies — local facts are never overwritten, and each Agent apply/rollback requires explicit approval.
- Import batches and per-item changes persist traces; updates save recovery snapshots; rollback re-verifies source, stable keys, and post-import summaries first, safely retaining user-managed or modified content.
- Team eval sets can only combine locally registered scenarios with locked expected contracts — arbitrary code and unknown scenarios are rejected; apply/rollback enters the unified audit.

- Third-party Skills, MCP, and workflow templates share the extension-governance ledger: records source, exact revision, and SHA-256, supports Ed25519 detached signatures, and clearly distinguishes signature validity from publisher-identity trust.
- Extension calls add per-instance per-minute quotas and cross-restart persistent circuit breakers; Skill content drift, invalid signatures, and tripped instances fail closed without affecting other extensions.
- Extension registration, signature verification, invocation, rate limiting, failures, and policy changes enter the unified audit; Skill/MCP pages show authenticity and isolation state, and governance IPC supports read-only inventory and constrained policy adjustments.

### HarmonyOS Project Semantic Model

- New versioned `HarmonySemanticModel` single parse source of truth: uniformly represents apps, products, nested modules, HAP/HSP/HAR artifact types, Ability, ExtensionAbility, and OHPM dependency edges.
- Deployment-required bundle, entry module, API level, signing state, and HAP output directory are now derived from the unified model; the project capability panel reuses the same module/dependency baseline instead of only scanning one directory level below the root.
- Semantic model schema upgraded to v2: structurally records root/module manifest sources and parse errors, supports OHPM v1/v3 and targetName lockfiles, and keeps declared constraints, locked versions, and lockfile sources on dependency edges.
- Semantic model schema upgraded to v3: full-module aggregation of main pages, router maps, permission usedScene, SystemCapability checks, and ArkTS/TS cross-module imports, generating project-relationship edges with manifest or source positions; old page summaries are now derived from this graph.
- Semantic model schema upgraded to v4: adds product API level, runtime OS, root & module build modes, apiType, device types, redacted signing completeness, and difference fields relative to the default product.
- Agent file changes hook into the semantic-model incremental cache: only the owning module is re-parsed, affected modules/products/recommended verifications are computed backwards along OHPM dependencies and real imports; root-structure changes or missing cache baselines safely fall back to full parsing.
- The Workspace project analysis adds a traceable product matrix, module artifacts/Abilities, manifest states, and relationship evidence, plus a read-only file-change impact preview explaining per-module sources: direct changes, OHPM dependencies, real imports, or project-structure propagation.
- `build_project` upgraded to an environment → dependencies → build → artifacts recoverable workflow: verifies against the unified model and can auto-install OHPM dependencies, durably records project fingerprints and redacted checkpoints, and only completes when HAP/HSP/HAR artifacts are actually discovered after a successful build.
- Hvigor/ArkTS errors uniformly carry source location, numeric or named error codes, build phase, and root-cause category, including inheriting the phase from the task line and reading the next `Error Message:` line; Agent error evidence and Workspace cards use the same structure.
- Build failures add joint log—semantic-model targeted diagnostics: identifies dependency version conflicts, cache/integrity corruption, missing SDKs, signing failures, and API-level incompatibility, outputting confidence, redacted evidence, auto-recovery boundaries, and ordered fix steps.
- `build_project` adds impact-driven build planning: accepts product and changed-file constraints, selects each product's minimal top-level artifacts along the dependency/import closure, schedules HAP/HSP/HAR tasks separately, and folds the deterministic target set into the recovery key.
- Successful builds add persistent HAP/HSP/HAR manifests: record content SHA-256, dual timestamps, module/product/mode, originating Hvigor step, and tiered signing evidence; the artifacts phase is no longer marked successful when files are unreadable or the manifest cannot be written.
- `deploy` / `deploy_all` default to manifest-driven deployment: re-verify project fingerprints, path boundaries, content hashes, and signing structure before deploying; only the unique newest trusted HAP is auto-selected; cross-product/module or simultaneous multiple candidates list evidence and require explicit confirmation.
- Device list upgraded to a unified state snapshot: provides hdc raw/normalized connection state, authorization, OS/API level, ABI, physical screen, evidenced capabilities, and observation time; Workspace and Agent share one data source, with online probing concurrent and timeout-constrained.
- Deployment loop uniformly wired into device-state and capability gates: explicit devices also re-verify connection/authorization/install/Ability/Hilog capabilities; launch failures after install first retain log evidence, only perform compensating uninstall for the newly installed app and re-verify the result, and never destructively uninstall during covered installs; standalone launches and uninstalls also add state confirmation.
- HarmonyOS build and runtime evidence joins the Agent's durable Run event stream: build plans/results/artifacts, per-device installs and states, Hilog, ArkTS exceptions, native crashes, and AppFreeze/ANR share one `run_id` and monotonic sequence; stale background listeners are constrained by the existing worker-lease fencing.
- `deploy_all` adds `serial|parallel` multi-device policy: serial fixes per-device execution order; parallel defaults to a cap of 2 with a hard cap of 4, no longer spawning all devices at once; device results are deterministically ordered and written into the current Run as per-device events plus batch summaries.
- `deploy_all` recovery reuses parent-Run per-device success evidence by HAP content hash: only failed or unexecuted devices are retried; artifact changes automatically redeploy all targets, avoiding duplicate installs of already-successful devices during recovery.
- `run_ui_flow` connects actions, UI trees, key-page assertions, and screenshot evidence: supports text/type/id/bundle existence/non-existence and exact/containment matching; action or assertion failures now genuinely return failure — `smoke_test` no longer misjudges failed steps as passed — and evidence paths are written into the current Run.
- `run_perf_benchmark` adds Ability-launch state confirmation, CPU/memory means and peaks, battery delta, temperature, FPS, and trusted HAP package size; failed pre-UI flows no longer produce invalid baselines; results and availability evidence are written into the current Run.
- Real-device resilience scenarios cover offline/authorization gates, install identity conflicts, permission denial, background recovery, and weak networks: unauthorized devices are no longer misattributed to signing errors; existing apps are not auto-uninstalled; permission revocation, background-to-foreground, and network qdisc set/restore all leave Run evidence.
- The local SDK API index upgrades to file-level incremental updates: unchanged declarations are reused, changed declarations rescanned, deleted declarations invalidated; the index covers types, all permissions/SystemCapabilities, introduced versions, and deprecation states, with reverse queries and refresh stats.
- Local declarations, official changes, and official reference retrieval uniformly bind to the current project product's compile/compatible/target APIs and installed SDK; results are marked per-item as usable, runtime-guard required, above-compile-SDK, deprecated, or removed, with alternatives only when `@useinstead`/official evidence is explicit.
- ArkTS build errors add API-evidence mapping: extracts type/module symbols from errors, correlates the current product's API level, local `.d.ts` official definitions, and official version changes, writing auditable evidence and recovery steps back to source locations and the same Run.
- `check_sdk_alignment` upgraded to a project-consistency audit: scans SDK imports and checks API levels, exact permissions, SystemCapability guards, device types, entry Abilities, permission usedScene, and product/module ownership; deterministic issues, risks, and degradation hints are graded and never auto-mutate configuration.
- `search_api` adds Android/Web/TypeScript migration modes: maps common implementations to HarmonyOS candidates by architectural semantics, marking each item verified/conditional/unavailable/unverified against the current project API level, local SDK module/symbol, and official sources, with risk boundaries and a full verification loop.
- The post-ETS-write verification plan upgrades to a mandatory loop: after the last write, local SDK/consistency audit, per-file error-free LSP, lint, tests, Hvigor build, and final diff evidence must be obtained in order; missing any required step keeps the execution loop in Verify, and deleted files no longer produce unreachable LSP gates.
- `environment_check` adds SDK/official-source provenance: uniformly displays source, version, update time, entry count, and coverage for local `.d.ts`, official API changes and reference libraries, and OpenHarmony doc mirrors, explicitly downgrading indexes older than 30 days, missing provenance, or missing versions, and forbids using them as the sole basis for generated code.
- `ohpm_search` upgrades to pre-adoption package auditing: compares explicit or project-locked versions with latest using official registry metadata, verifies package declarations against the project's compatible API, classifies licenses, and checks integrity digests, deprecation states, install-time scripts, and external-source dependencies; explicitly marks "unknown" when the registry has no vulnerability-advisory evidence. The OHPM candidate list shows licenses too.
- `get_project_info patterns=true` adds GitHub/Gitee HarmonyOS open-source project pattern analysis: binds redacted origin, branch, and commit; extracts modularization, product, routing, Ability, dependency, state, network, storage, testing, native, and multi-device patterns using only the semantic model and exact source evidence, giving per-pattern applicability boundaries, reuse steps, and risks; scanning has deterministic caps, does not follow symlinks, and never executes third-party code.
- `search_knowledge` establishes unified HarmonyOS ecosystem knowledge records: beyond team experience, third-party package compatibility rules, common errors, and device differences can be retrieved by API level, device type, and error fingerprint; each record binds applicable conditions, regression sources, verification state, and unknown boundaries; `ohpm_search detail=true` converts live registry audits into the same versioned records.
- `environment_check path=...` adds a DevEco public-config interop report: generates deterministic config fingerprints for AppScope, product/module, OHPM, Hvigor, and manifest; explicitly ignores `.idea` and `local.properties` private content; only hints machine-absolute paths and sensitive config via field paths — never outputs values or depends on IDE private state.
- The release security domain switches to per-time explicit approval: release builds, signing references, OTA, credential reads, and release/signing commands cannot be bypassed by allow-all, project/session whitelists, or historical authorization; approval parameters are redacted first, and sensitive-call failures fall to manual recovery.
- `copy_signing_from` only allows non-sensitive signing metadata inside authorized roots and in-project material references; passwords/tokens/private-key fields and out-of-directory materials are rejected outright; unknown fields are dropped by whitelist; app-store release capabilities stay disabled until the same governance contract is met.
- Skill manifest v1 adds independent SemVer, HarmonyAgent compatibility range, permission enumeration, compatibility state, and `SKILL.md` content hash: legacy Skills are explicitly `legacy_unverified`, incompatible items stay disabled, and post-import content drift blocks instruction injection and invocation; Skill declarations cannot expand existing tool permissions.
- Built-in capability packs add schema/version, minimum Agent version, and `read_only|project_write|device_write|delivery` permission caps; selection policy can evolve independently of the tool protocol.
- New project-level workflow template v1: supports validation, import, listing, enabling, disabling, and SemVer upgrades; per-step verification of registered tools, acceptance conditions, and permission lists; recursive templates and incompatible Agent versions are rejected.
- Workflow imports/upgrades require per-time explicit approval; upgrades only accept higher versions, new permissions require separate confirmation of permission diffs, and old versions are archived in the project's `.deveco-agent/workflow-templates/history/` for manual recovery; templates never auto-execute on import or enable.
- MCP switches to explicit project authorization: after upgrading old configs, instances default to unauthorized; global config only serves as a clonable template; only instances precisely bound to the current project, enabled, and configured with tool/directory/network/credential whitelists enter the Agent.
- MCP tool discovery and actual invocation double-check authorization; path parameters are restricted to the project-relative root; deny network policies block network-address parameters without injecting the proxy; Agent child processes clear inherited environments, passing only minimal runtime variables and explicitly approved environment variables; server cards no longer show environment-variable values.
- MCP command or environment-config changes invalidate authorization and disconnect old processes; authorization changes write redacted audits; OS-level sandboxing, mandatory network isolation, and third-party-source governance remain deferred to EC10.

### Long-Session Context V2 (M1 Foundation)

- Agent tool parameters gain schema-level pre-checks at the unified execution entry point: correction suggestions for JSON syntax, object shape, missing required fields, and unknown fields; no parameter is silently rewritten, and sensitive fields (tokens, certificates, signing materials, device identifiers) explicitly forbid auto-correction.
- The phase tool selector upgrades to evidence-driven ranking: combines the capability-pack prior with 90-day success rates, average duration, estimated result-token cost, side-effect level, and current HarmonyOS project/Git repo/device availability; only the top 32 scored tools are exposed each round, with explainable rankings recorded.
- New unified execution-loop state machine: converges the goal contract, verifiable plan, per-phase minimal tool set, real execution evidence, independent verification, and final acceptance into one snapshot; phase changes are durably recorded as `workflow.stage` events and re-injected every round; a successful write cannot skip the verification gate.
- File changes auto-generate verification plans from the real success trajectory: ArkTS/ETS picks formatting, lint, tests, Hvigor build, and diff; generic code picks formatting, static checks, tests, build, and diff; doc changes at least check the diff. Failed writes do not produce fake verification scope.
- Deploy, Ability launch, Git commit/push/merge, database migrations, key & knowledge-base writes, and non-read-only HTTP requests add a write-then-read confirmation matrix; deploy and Git acceptance must bind to time-ordered subsequent state reads — the writing tool's own success text no longer self-certifies completion.
- `compose` multi-step tool flows upgrade to recoverable logical transactions: successful steps write Durable checkpoints; main-step failures can take fallback degradation; overall failures execute explicit compensation in reverse order and list uncompensated side effects for manual recovery; nested composite transactions are forbidden; unhandled failures no longer return fake success.
- New tiered context model: task snapshots, sourced facts, artifact references, summary-coverage cursors, and explicit token budgets.
- When new facts conflict with old ones, historical versions are retained and marked invalid; Context summaries are no longer designed as the single source of truth for files, Git, tools, or device state.
- New Context projection checkpoints and invalidation epochs, with compatible reads of existing session summaries and task ledgers.
- The chat loop rebuilds structured context every round from the Durable Run, execution steps, and sourced facts; summaries dual-write checkpoints by message/event cursors, and read failures auto-fall back to the old path.
- Build, Git, and device tool results and artifacts automatically enter the Context projection; file modifications, branch switches, project-identity changes, and device side effects invalidate related old facts.
- The Workspace context status bar expands to show the current goal, tiered token budgets, summary-coverage cursors, facts, and artifact sources.
- Hot context adds recent messages, current errors, active files, and items awaiting user confirmation; approvals, plan reviews, and Agent questions all persist request, Owner, timeout, and terminal state; restarts only converge orphaned Runs and never auto-approve.
- Post auto/manual compaction runs summary—fact reconciliation, appending machine-generated authoritative fact blocks; conflicts between summaries and failed builds/tests, unfinished Runs, or pending-approval state record correction audits.
- New regression tests for 120-message compaction, SQLite close/reopen, and fact-conflict generation changes.
- Project long-term memory upgrades to the Context V2 project layer, adding architecture/build-command/module-responsibility/user-preference categories plus source, confidence, version, confirmation, pinning, and explicit invalidation conditions.
- Branches, project identity, file paths, and device side effects precisely invalidate old knowledge per declared memory conditions, keeping source, version, and invalidation reasons in the memory panel for explanation.
- Key messages, human decisions, active files, and acceptance conditions can be durably pinned as authoritative context, surviving compaction and participating in summary-fact reconciliation; the original-message pin entry point syncs with Context V2.
- Explicit notifications for proactive-compaction thresholds, over-limit retries, or summary-fact conflicts; recovery verification continues with explicit progress and error-state feedback.
- After restart, tasks resume from the Durable Run, steps, Context snapshots, and event cursors; recovery plans first verify files, Git, artifacts, devices, and external state; the durable queue adds safe pause, resume, and cancel controls with audit records.
- Recovery supports incremental append, explicit removal, and whole replacement of goal requirements; goal-contract diffs enter events and audits; plan items under the old goal that are unfinished and no longer applicable are auto-cancelled; negative expressions like "don't push yet" no longer generate false acceptance requirements.
- Sessions can create durable branches anchored at messages, checkpoints, build failures, or Git commits; merges are strictly limited to pinned decisions, acceptance conditions, artifact references, and sourced verification facts — never concatenating messages or summaries.
- Sub-agent delegation upgrades to protocol V2: scoped context references, tool ranges, and nesting depth, explicitly not copying the parent session's full text; return values unify into `SubAgentResultV2` with acceptance, artifacts, evidence, blockers, and errors.
- Long-session M2 automated acceptance completed: 120-message reopen recovery, four-hour equivalent checkpoint/lease recovery, goal changes, provenance tracing, and side-effect anti-replay all form repeatable test evidence.
- Tool results unify into an extensible `ToolResultV2`: all registered tools stably output status, modifications, artifacts, verification, recovery, suggestions, and an error envelope, compatible with legacy V2 records and unknown future fields.
- Tool execution contracts add side-effect, idempotency, timeout, cancel, retry, approval, and recovery metadata; Tool Worker timeouts are contract-driven; unknown MCP tools use a conservative always-approve write strategy.
- Successful and failed long tool outputs uniformly externalize to retention-policy-managed artifacts; the model only receives bounded head/tail summaries and read references; `ToolResultV2` records artifact paths.
- Text and JSON redaction converges into a unified entry point covering tokens, certificates/private keys, signing materials, sensitive environment variables, connection passwords, and device unique identifiers; MCP errors, long-output artifacts, tool audits, and human interactions no longer have bypass paths.
- Validators and recovery actions enter the tool-contract source of truth: verification evidence no longer relies on result-layer hardcoding; all side-effecting tools declare snapshot recovery, Git compensation commits, redeploy, verify-then-compensate, or manual recovery strategies.
- New 7 capability packs: project understanding, compile fixing, feature development, refactoring, build & deploy, device diagnostics, and Git delivery; system prompts and native tool schemas share a bounded selector, each pack declaring minimal tool sets, order, stop conditions, and acceptance.
- Each round's model request switches between explore/modify/verify/deliver/recover phases based on persistent tool evidence, dynamically injecting up to 32 phase tools; Git push only opens after verification passes and the goal explicitly requires delivery.
- The reliability panel adds a tool-governance list: identifies high-failure-rate and genuinely long-unused tools by window, listing conservative functional-overlap candidates with fix/hide/merge review suggestions.
- Fixed `062_tool_execution_threads.sql` not being registered in the unified migration list, ensuring Tool Worker thread fields actually apply when existing users upgrade.

### Goal Contract and Evidence Acceptance

- User goals compile into a structured `GoalContract` recognizing required conditions such as modify, verify, build, test, deploy, commit, and push.
- Tool results convert to structured evidence recording artifacts, verification scope, errors, compensation strategies, metrics, and evidence digests.
- The model can only claim completion; the runtime kernel adjudicates against the real tool trajectory. Verification after modification must occur after the last write; missing evidence enters an automatic remediation loop.
- When the remediation budget is exhausted without satisfying the contract, the Run converges to `interrupted/continuation_required`; natural-language completion claims are never treated as success.

### Durable Run, Scheduler Queue, and DAG

- `agent_runs` extends with goal contracts, dynamic budgets, leases, recovery info, and quality snapshots; Run terminal states are irreversible.
- New persisted `agent_task_queue` supporting priority, claim, backoff retries, checkpoints, resume tokens, concurrency keys, and tenants.
- New Agent DAG nodes/edges: main tasks and sub-agents record dependency conditions, failure policies, independent attempts, and acceptance results; root acceptance merges child-node evidence.
- New execution-step coordination and side-effect-aware recovery: reads can safely retry; writes/commands/deploys verify effects first; indeterminate cases require human confirmation.

### Multi-Process Agent Worker

- Each desktop process registers a unique Worker, PID, host, capacity, and heartbeat; starting a second instance never interrupts the first instance's still-healthy tasks.
- Queue claims generate lease tokens and incrementing epochs; checkpoint, lease-renewal, and terminal-state writes enforce Owner fencing; late writes from old workers are rejected.
- Heartbeat scans only reclaim genuinely expired or orphaned Owners; new real-process crash E2Es cover claim, process exit, lease expiry, and takeover recovery.

### Tool Execution Kernel

- `tool_runs` adds protocol version, structured results, idempotency keys, execution workers, leases, attempts, verification state, recovery count, and outcome-commit time.
- Side-effecting tools adopt prepared → running/verifying → committed semantics; duplicate side effects in the same Run are blocked by idempotency keys; late results are dropped by lease fencing.
- Actual tool futures move to named dedicated OS threads; thread panics are isolated by `catch_unwind` without taking down the main process.
- Caller timeout/cancel with the thread still running marks it stuck; a background scan simultaneously detects lease-expired calls; the control plane adds `stuck_tools` metrics and worker-thread identity.
- Added isolation tests for stuck threads, uncancellable late results, output floods, and real orphan processes; Unix process-tree cleanup first lets wrappers reap terminated children before force-killing as a fallback, avoiding zombie PIDs.
- Tool quality metrics add success rate, parameter-error rate, timeout rate, retry rate, cancel latency, and average duration, distinguishing direct contribution from "succeeded but did not advance acceptance" calls after final acceptance.
- Tool SLOs add duplicate-side-effect, out-of-capability-pack mis-selection, and invalid-success caps; the reliability panel compares success rates, contribution rates, and durations by tool, capability pack, model, project, and protocol/app version.
- New tool-protocol version directory and producer-version dimension: V1 history stays read-only compatible; V2 retains unknown future fields; future incompatible changes must use new schema versions with explicit migrations.
- `ToolResultV2` adds backward-compatible impact explanations; failures uniformly give cause, real state impact, completed parts, and recovery next steps; the phase gate adds a 12 high-frequency-tool fault-protocol matrix and post-trimming completability tests for typical tasks.
- New E2Es for tool-thread panics, process crashes, side-effect recovery, and duplicate-execution protection.

### Reliability Control Plane and Quality Gates

- New SLO policies, alerts, audit events, quotas, and eval history; the cost page displays acceptance rates, quality scores, recovery rates, structured-evidence coverage, queues/DAGs, Agent Workers, Tool Workers, and stuck tools.
- CI adds reliability, Execution Kernel, multi-process Worker crash, and Tool Worker crash E2E gates on macOS/Windows.

### Documentation Calibration

- Rewrote the architecture doc, replacing the outdated "frontend TS orchestration" design with the Rust-backend Agent main loop and the dual execution kernel.
- README codebase scale updated to 198 tools, 29 Agent top-level modules, 29 tool files, 33 command modules, 36 service modules, 281 IPC entry points, 68 migrations, and 14 pages.
- Clarified the cadence difference between capability-batch versions and the app manifest version; this batch does not change the release version — `package.json`, Cargo, and the Tauri manifest remain `2.0.0`.

### Fixes

- Unified preferred-HAP output directory and recursive-fallback artifact ordering: `-signed.hap` outranks newer unsigned packages, avoiding accidental selection of non-installable unsigned artifacts during deployment; regression test added.
- Admin sidebar removed the hardcoded `v0.1.0`, now showing the current manifest version via Tauri `getVersion()` with the `2.0.0` startup fallback kept.

### Migrations

| Number | Content |
|---|---|
| `057_agent_governance.sql` | Goal contracts, remediation, Run leases, quality snapshots |
| `058_reliability_control_plane.sql` | Structured evidence, scheduler queue, DAG, evals |
| `059_execution_kernel_v2.sql` | Queue protocol, tool protocol V2, SLO/alerts/audit/quotas |
| `060_multi_worker_runtime.sql` | Agent Workers, lease tokens, claim epochs, attempt ledgers |
| `061_tool_execution_kernel_v2.sql` | Tool Workers, execution leases, verification/recovery and attempt ledgers |
| `062_tool_execution_threads.sql` | Tool thread identity and stuck counts |
| `063_conversation_context_v2.sql` | Tiered context, sourced facts, artifact references, summary cursors |
| `064_pending_interactions.sql` | Persistent lifecycle for approvals, plan reviews, Agent questions |
| `065_context_reconciliation.sql` | Conflict detection between summaries and structured facts, correction audits |
| `066_structured_project_memories.sql` | Project memory source, version, confirmation, pinning, conditional invalidation |
| `067_context_pins.sql` | User-pinned messages, decisions, files, acceptance conditions |
| `068_conversation_branches.sql` | Session-branch lineage and structured merge manifests |

---

## v2.2 — Eight-Repo Inventory Landing: Hybrid Retrieval + Time Travel + Scheduled Reminders + Cross-Session References (2026-08-20)

Positioning: capability landing after a full inventory of 8 reference repos (deepseek-harness / qwen-code / Qwen-Agent / langgraph / OpenHands, etc.) — retrieval, session management, task orchestration, and tooling each gained a batch of high-value capabilities; tools **193 → 198**.

### 🔍 Retrieval and Memory Upgrades (desA, aligned with Qwen-Agent)

- **BM25 re-ranking**: new `utils/tokenizer.rs` (Chinese 2-4 char sliding-window n-grams + English whole words + stopword filtering) and `utils/relevance.rs` Okapi BM25 index (k1=1.2 / b=0.75, consistent with rank_bm25); `keyword_search` / memory retrieval results go from SQL dictionary order to **BM25 relevance re-ranking** (title double-injection approximating position weights + time decay + category weighting).
- **front_page pinning**: when the memory-injection budget allows, the 2 most recently updated memories are unconditionally pinned to the front (aligned with Qwen-Agent front_page_search), skipped automatically when the budget is tight.
- **RRF fusion**: embedding vector retrieval and BM25 keyword retrieval fuse via dual-path RRF (aligned with Qwen-Agent hybrid_search's three-piece hybrid retrieval).
- **pitfall weighted front-loading**: in build-error-fix tasks, build-class historical memories are weight-fronted so the Agent sees the same pitfalls this project has hit before acting.

### 🧭 Session Time Travel (aligned with langgraph checkpoints)

- **Automatic snapshot saving** (migration `051_conversation_snapshots.sql`): after each tool-execution round, a state anchor is saved (visible-message rowid + ledger + model-output summary), capped at 50 per session; rounds without execution traces are not saved.
- **Bidirectional recovery**: `restore_snapshot` archives post-anchor messages (hidden, old branches stay traceable), re-materializes archived segments before the anchor, and writes the ledger back to the snapshot moment (continued runs inherit that point's execution trajectory); recovery is rejected while a task is running (prevents message-write races).
- **Frontend timeline**: More menu → "Session Timeline" dialog with snapshot points (label/time/tool count/current marker); "Return Here" recovers after a warn confirmation and refreshes messages/ledger/audit traces (`task.timeline`).

### ⏰ Scheduled Reminders (aligned with deepseek-harness schedule)

- New tools `schedule_create` (after / at / every; error codes include invalid_prompt / invalid_selector / not_future / frequency_too_high) / `schedule_list` / `schedule_delete`; every-anchors advance without enumerating missed historical periods.
- New service `services/reminders.rs` + migration `052_reminders_feedback_terms.sql` (`message_reminders` table); lib.rs setup polls every 30s to dispatch due reminders → session queue injection (`inject_message`, session-local without interrupting the current round) + desktop notification.

### 📊 Message Feedback Correction (A2)

- Disliked message high-frequency words (top 5 with frequency ≥2) are written into the `feedback_terms` bag; before memory injection the negative-feedback bag loads; memories hitting ≥2 distinct words are dropped from injection, and those hitting 1 are moved to the end — content users don't want stops reappearing in context.

### 🛡 Invariant Guards (A5)

- New `agent/invariants.rs` registry (`Invariant { name, check }` + static array + unified `check_write` entry): 3 invariants — `.env*` prefixed files, 8 key/certificate suffixes (`.key/.pem/.pfx/.p12/.keystore`, etc.), and existing `migrations/*.sql` (executed migrations immutable, new ones allowed); `fs_tools::is_protected_file` delegates to the registry, with 2 tests.

### 🔗 Cross-Session References (B6)

- `references_json` supports the `conv:<id>` prefix: historical replay injects the session title + summary (`messages.summary` non-empty preferred, falling back to the last assistant content; 2000 chars per session / 8000 total cap); the frontend @ panel adds session candidates (same project, current excluded, fuzzy title match, chat icon); selecting one inserts title + recent content into the draft, isomorphic with message references (Quote).

### 🛠 Streaming Robustness Hardening

- **Silent no-output timeout**: connection alive but nothing parseable for 60s → keep received content and auto-continue (same chain as interrupted continuation).
- **Pre-output interruption freeze-replay**: stream breaks before outputting anything → freeze and resend the request verbatim (≤5 times, aligned with the DeepSeek-Reasonix mechanism; the model needn't re-think and prompt caches stay valid).
- **Tool loop detection** (lightweight version aligned with qwen-code LoopDetectionService): consecutive identical calls (name+args) / consecutive same-name calls (parameter jitter) / per-round total tool soft and hard caps; hitting any injects a correction prompt, wrapping up after at most two interruptions.
- **Action-promise fake-completion correction**: when the model announces it's starting development or only outputs a plan with no tool markers, inject a correction prompt demanding immediate execution (capped to prevent infinite loops).
- **reasoning_content multi-round compliance**: DeepSeek reasoning models carry the full thought chain back on requests with tools (missing it causes 400/broken chains); V4 thinking mode parses content array blocks (text blocks into the body / thinking blocks into reasoning); assistant messages with tool-only calls echo reasoning (plain-text answers don't echo and don't consume input budget).
- **run_command output overflow spill**: when response exceeds limits, full text spills to disk + head/tail sampling + `store_overflow` path marker; the Agent can read back the full output on demand.

### 🧰 Other

- `ui_focus` tool (aligned with OpenHands canvas_ui_control): drives UI focus after Agent output (switch right panel / open file preview, L0 permission).
- `memorize` tool + `replay_memories` (aligned with Qwen-Agent MemoAssistant): replays memorize calls from historical messages to rebuild key-value state, injected as system content each round.
- File-tree panel: auto-reloads when expanded but the cache is missing (expanded directories need no manual re-click after refresh).
- Logger test isolation fix (pid-reuse leftover files caused occasional assertion failures).

### ✅ Verification

- `cargo check`: 0 errors / 0 warnings
- `cargo test --lib`: **446 passed / 0 failed** (new: reminders 2 + invariants 2 + retrieval/protocol several)
- Frontend `tsc --noEmit`: passed

### 🔄 Migration Notes

- New migrations `051_conversation_snapshots.sql`, `052_reminders_feedback_terms.sql` (auto-applied on existing databases, no breaking changes).
- Tool total 193 → **198** (+memorize / ui_focus / schedule_create / schedule_list / schedule_delete); `TOOL_SPECS` count is authoritative at `src-tauri/src/agent/tools/mod.rs`.
- `inject_references` signature adds a `conn` parameter (conv: session-summary queries); internal call sites synced.

---

## v2.1 — Conversation Flow Hardening + Minimal Whitespace UI (2026-08-19)

Positioning: a full check-up and fix around "can conversations flow properly", resolving state inconsistencies in stop/delete/approval/error boundary scenarios, and restyling the conversation area to a minimal whitespace aesthetic.

### 🐛 Conversation Flow Fixes (Backend)

- **Stop semantics fixed**: after the user clicks stop, queued messages no longer auto-resume (`stream_chat_body` terminates queue consumption when `stats.stopped`), eliminating "I clicked stop, but the AI started working again later".
- **Deleting a running session**: `delete_conversation` now stops first + aborts background tasks (new `TaskRegistry::abort_conversation`) + releases project locks, then deletes from the DB, eliminating orphan tasks, continued file writes, and long-held project locks. Deletion also cleans `tool_limits` / `task_guard` in-process state, fixing memory growing monotonically with session count.
- **Stop during approval/plan review**: new `InterceptKind::Cancelled`, `ApprovalOutcome::Cancelled`, `PlanReview.cancelled`; clicking stop while waiting on tool approval/plan review now wraps up as "stopped" (`chat-stopped`) instead of being treated as "rejected", which caused another round to run or a normal-completion display. Both serial and batched tool paths are covered.
- **Task watchdog**: `TaskRegistry` registers all `stream_chat` tasks uniformly; no heartbeat for 8 minutes / stop ineffective for 40 seconds → force abort and emit `chat-error`; `stream_once` touches frequently per phase (send→first byte→stream→parse).
- **New migration `050_task_ledger.sql`**: persists `task_runs.target_text / target_passed / target_evidence`.
- `chat-done` event adds a `user_message_id` field for frontend optimistic-placeholder replacement.

### 🐛 Conversation Flow Fixes (Frontend)

- **Error state coexisting with streaming ghosts**: on error, clear `conversationId` / `startedAt` so the typing cursor/three-dot animation disappears immediately, keeping only generated content + the error card.
- **Optimistic user message ID not replaced**: `chat-done` replaces the `local-` placeholder with the real `user_message_id`, so edit/delete/branch-regenerate/Fork within the current session cycle takes effect immediately.
- **Stop fallback timer killing new tasks**: `stopGeneration`'s 60s fallback validates with a `startedAt` generation token, so resending right after stop is no longer mis-marked by the old timer.
- **Watchdog killing background approval sessions**: switched to judging by `pendingConfirmations[convId]` (including background sessions) rather than only the current session view array.
- Optimistic message IDs get random suffixes to avoid same-second cross-session collisions; queued failures pop error notifications; `chat-done` refreshes lists by the completing session's own `project_id`; new `conversation-deleted` event listener (multi-client/LAN deletions sync-clean and switch sessions).

### 🎨 Minimal Whitespace Conversation Styling

- Message-header model/duration/token/message-ID badges **hidden by default, shown on hover**; assistant avatars lose the purple gradient for plain dots; user bubbles lose colored borders/shadows for neutral backgrounds.
- Tool cards / sub-agents / plan cards / ledger cards / task progress bars unify to **plain text lines + collapse**: colored backgrounds, left bars, icon color blocks, shadows, and completion pulses removed.
- Thinking blocks become thin left lines; error cards weaken to neutral borders; `.task-*` CSS classes drop backgrounds/bars/shadows.

### ✅ Verification

- `cargo check`: 0 errors / 0 warnings
- `cargo test --lib`: **418 passed / 0 failed** (incl. ask/guards/pipeline)
- Frontend `tsc --noEmit`: passed

### 🔄 Migration Notes

- New migration `050_task_ledger.sql` (auto-applied on existing databases, no breaking changes).
- `delete_conversation` changed from sync to `async`; signature adds `app/cancel/lock/registry` state parameters; the LAN service uses the synchronous internal function `delete_conversation_sync` instead — HTTP behavior unchanged (deletion still cascades to stop running tasks).

---

## v2.0 — Agent Workspace Wrap-Up (2026-08-16)

Positioning: upgraded from a "Provider switcher" to a **full Agent Workspace** — tools 117 → **191**, covering the full HarmonyOS development chain; added 9 capability tools + structured ToolError; command palette and i18n landed together; oversized single files split by responsibility.

### ✨ New (9 A-class tools + 1 error-system upgrade)

| ID   | Tool | Capability |
|------|------|------|
| [14] | `log_query`         | Structured queries across hilog / runtime_log / faultlog (since / level / keyword / regex / device filter), time-aggregated output with hit-segment truncation |
| [23] | `docx_read`         | `.docx` body text (pure standard-library `zip` + XML streaming parse, zero deps) |
| [26] | `audio_transcribe`  | Local `whisper.cpp` transcription (auto-locates whisper binary + ggml model) |
| [28] | `attach_debugger`   | `hdc shell debuggerd -p <pid>` attach + `aa debug` fallback; outputs PID / bundle / wait_secs and next-step guidance |
| [29] | `step_debug`        | step / next / continue / interrupt / where / info six-action debugger driver |
| [30] | `memory_snapshot`   | take / list / diff actions; two consecutive >10% growths auto-suggest "possible leak" |
| [36] | `ota_pack`          | Built-in `packagingtool.jar` → `.pkg` packaging (auto-locates the jar, optional profile_path signing injection) |
| [48] | `license_check`     | Scans `oh-package.json5` / `Cargo.toml` / `pyproject.toml` against built-in allow/deny lists, outputting violations |
| [49] | `vuln_scan`         | Built-in 10 known vulnerabilities (lodash/axios/requests/spring/jackson, etc.), matched by dependency version, giving CVEs and suggested versions |
| [65] | `ToolError`         | 7 categories (network/permission/not_found/invalid_input/internal/timeout/conflict) + retryability + auto-suggested next steps; `run_tool` exit auto-wraps the envelope, zero-intrusion coverage of all 191 tools |

### 🛠 Refactors and Splits

- **Toolset reorganization**: several large v1 tools split into more focused variants (e.g., 9 `lsp_*`, build/deploy/signing items, debug `attach/step/breakpoint` independent), final count **117 → 191**.
- **TOOL_GROUP**: grouped into 8 domains (`build` / `fix` / `explore` / `deploy` / `refactor` / `test` / `debug` / `other`), frontend renders and limits by group.
- **TASK_GROUPS**: aligned with TOOL_GROUP; limits and guards apply per group (fixing "limit by tool name" throttling hot tools globally).

### 🎨 Command Palette + i18n (Frontend Companion)

- Command palette adds **28 high-frequency tool actions** (`Cmd+K` instant trigger), covering debug 4 / refactor 5 / build 2 / deploy 1 / security 4 / knowledge 4 / data 2 / governance 5 / multimodal 3.
- zh/en `i18n` adds 30 tool labels (30 each language); frontend falls back to `t('toolToolName')` for misses.

### 🧹 Code Structure Cleanup

- `agent/tools/quality_tools.rs` split from a **2400+-line single file** into a facade + 4 sub-files, **sliced completely by method** (not by line count — every `fn` with a multi-line signature plus body lands intact in one file):

  | Sub-file | Tools | Functions | Content |
  |--------|-------:|------:|------|
  | `quality_metrics.rs`  | 7 | 15 | code_metrics / metric_export / log_aggregate / log_query / memory_snapshot / snippet_insert / replay_trace + 7 helpers + `FileMetrics` / `SOURCE_EXTS` / `SKIP_DIRS` |
  | `quality_security.rs` | 4 |  9 | obfuscate / sandbox_exec / license_check / vuln_scan + 5 helpers |
  | `quality_runtime.rs`  | 6 | 11 | api_test / api_mock / api_health / attach_debugger / step_debug / ota_pack + `MockRoute` struct + 4 helpers + `hdc_shell` |
  | `quality_media.rs`    | 2 |  5 | docx_read / audio_transcribe + 3 helpers |

  Split principles:
  - `pub use module::*` re-exports in the facade; external `quality_tools::code_metrics(...)` calls stay unchanged.
  - Helpers follow the "main consumer" file (e.g., `parse_dep_line` goes to security with `license_check`).
  - `pub(super) async fn` → `pub async fn` (`pub use` cannot re-export private items).
  - `super::xxx` → `crate::agent::tools::xxx` (facade can't see it, absolute paths required).
  - Cross-file shared constants (e.g., `SKIP_DIRS`) are copied to whoever needs them to avoid reverse dependencies; for genuinely multi-use cases, `pub` on `scanner.rs`.

- Root directory cleanup: **59 debug/analysis scripts** archived into `scripts/legacy/` (11 Python processing scripts + 48 old log/test artifacts).
- `.gitignore` adds `scripts/legacy/` / `__pycache__/` / `*.pyc` / `*.log` rules to avoid accidentally committing temp files.

### 📚 Documentation

- `README.md` expanded from 1642 bytes to 12k+ bytes, repositioned as "Agent Workspace", with the tool list, capability matrix, command-palette usage, security governance, and bundled-runtime notes completed.
- `docs/tool-enhancement-backlog.txt` upgraded to v2 completion state (56/76 delivered; 3 external figma/feishu/jira items deferred per user request).
- `docs/ARCHITECTURE.md` synced: module map after quality sub-file split, TOOL_GROUP × TASK_GROUPS relationship table.

### ✅ Verification

- `cargo check --lib`: **0 errors / 0 warnings**
- `cargo test --lib`: **346 passed / 0 failed** (7 are new ToolError unit tests)
- Post-split `quality_tools::xxx(...)` call sites: **0 changes needed** (facade re-export compatible)

### 🔄 Migration Notes

- No breaking changes. `quality_tools` public API is 100% compatible; external imports need no changes.
- `agent::scanner::SKIP_DIRS` changed from `const` → `pub const` (borrowed by `quality_security`); note if external code depended on its privacy.
- Command-palette default ordering changed: high-frequency tools pinned to the top; long-tail tools collapse into a second-level menu.

---

## v1.0 — Initial Commit (2026-08-14)

Positioning: HarmonyOS desktop AI coding IDE prototype, **117 Agent tools** + multi-provider routing + bundled runtimes.

### Foundational Capabilities

- **AI Agent core**: multi-turn conversation, sub-agent spawning (`spawn_agents`), task planning (`plan_task`), TodoWrite progress tracking, `undo_edit` undo stack, cross-turn diagnostic memory, `ask_user` proactive questioning, background tasks (`run_command --background`), runtime logging (`hdc shell hilog -L E`).
- **Deep HarmonyOS integration**: hdc device management / wireless on-device connection / emulator start-stop / hvigor builds / ohpm dependencies / faultlog crash attribution / real-time hilog streaming / multi-module workspace detection.
- **Multi-provider routing**: multiple LLM providers (Huawei / Zhipu / Qwen, etc.) + local HTTP proxy + circuit breaker + automatic failover + cost tracking + request logging.
- **API knowledge base**: built-in HarmonyOS API index (vector retrieval + symbol index) + cross-version diff + compatibility scanning + user notes.
- **Security governance**: tool-call whitelist / tool limits / task guard / budget control / permission management / approval interception pipeline (pre/post hooks).
- **Bundled runtimes**: ships Node + JDK + Git runtime environments (`src-tauri/runtime/`), no dev environment pre-installation needed on user machines.
- **Code understanding**: tiered scanning (`check_code` / `deep_scan` / `codebase_search` / `get_symbol_details`) / symbol index / filesystem toolset.
- **Ecosystem capabilities**: MCP server management / Skill enable-disable / HarmonyOS official doc retrieval / web search & fetch / knowledge base import-export.

### Key Modules

| Module | Lines (end of v1) | Description |
|------|-------------:|------|
| `agent/tools/mod.rs`        | ~4200 | Tool registry (TOOL_SPECS / TOOL_GROUP / 191-tool dispatcher) |
| `agent/tools/fs_tools.rs`   | ~1500 | File read/write / search / folding / gitignore |
| `agent/tools/build_tools.rs`| ~1200 | hvigor / ohpm / signing / deploy / artifact analysis |
| `agent/tools/cmd_tools.rs`  | ~ 700 | run_command dangerous-command blacklist + sandbox + background tasks |
| `agent/agent_board.rs`, etc. | 200-600 each | Agent orchestration, reflection, memory, session events, task queues |

### Known Legacy Issues (end of v1 → fixed in v2)

- Conversation SSE streaming split multi-byte characters at chunk boundaries → `U+FFFD` persisted permanently (v1.1 fix: byte-buffered whole-line decoding)
- `list_dir` didn't honor `.gitignore` (v1.1 fix: subdirectories + submodule rules)
- `read_file` didn't fold when comment ratio was high, drowning code (v1.1 fix: consecutive long-comment blocks folded into one-line summaries)
- gitignore silently failed at runtime (`canonicalize`'s `\\?\` prefix inconsistent with `normalize`; v1.1 fix)
- Lots of debug/analysis scripts scattered in the root (v2 fix: archived into `scripts/legacy/`)

---

## Version Comparison Quick Reference

| Dimension | v1.0 | v2.0 | Delta |
|------|-----:|-----:|-----:|
| Tools (TOOL_SPECS) | 117 | 191 | **+74** (+63.2%) |
| New tools | — | 9 + ToolError | — |
| TOOL_GROUP domains | 3 | 8 | +5 |
| Command-palette actions | 0 | 28 | +28 |
| i18n tool labels | 0 | 30 × 2 languages | +60 |
| Docs (README + ARCHITECTURE + backlog) | 60+1248+0 | 12000+1500+700 | +10× |
| cargo test | 282 passed | **346 passed** | +64 |
| Compile errors/warnings | 0/0 | **0/0** | flat |
| Root debug scripts | 59 | 0 | -59 |

---

## Maintenance Notes

- The tool total is authoritative at the length of the `TOOL_SPECS` array in `src-tauri/src/agent/tools/mod.rs` (currently 201).
- Task groups are authoritative at the `TASK_GROUPS` constant (currently 8: `build` / `fix` / `explore` / `deploy` / `refactor` / `test` / `debug` / `other`).
- `quality_tools::*` is exposed via the facade; **importing the 4 sub-files directly (`quality_metrics`, etc.) is forbidden** — they are internal modules; external coupling goes through the facade.
- Any "split by line count" of tools is forbidden. **Must slice completely by method**, with signature + body in the same file. Script aid: `scripts/legacy/_split_quality.py`.
- Any CHANGELOG change should be committed with `docs(changelog): <one-liner>` in the commit message; do not edit this file and then commit with plain `docs:`.
