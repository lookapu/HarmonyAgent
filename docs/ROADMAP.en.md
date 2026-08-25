# HarmonyAgent Continuous Evolution Roadmap

> Status: in progress  
> Scope: long sessions, the Agent toolchain, the HarmonyOS development loop, ecosystem integration, and the quality system  
> Update rules: check items off in this file as tasks are completed, and record user-visible changes in `CHANGELOG.md`; database structure may only evolve through incrementing-numbered migrations.

[简体中文](ROADMAP.md) | English

Long-session data mapping and fact priority are described in [CONTEXT_V2.md](CONTEXT_V2.md).

## 1. Goals and Advancement Principles

The next stage's goal is not to keep stacking more tools, but to make HarmonyAgent an engineering partner that can collaborate long-term, execute reliably, understand HarmonyOS projects, and complete on-device verification.

The advancement order is:

1. Long-session execution kernel V2: ensure tasks remain continuable after context compaction, app restarts, and goal changes.
2. Agent toolchain quality engineering: unify contracts, reduce mis-invocations, and strengthen verification and recovery.
3. HarmonyOS development loop: connect project understanding, build, deploy, runtime, and diagnostics.
4. Ecosystem integration and productization: connect SDKs, ohpm, the open-source ecosystem, and team workflows.

All phases share the following principles:

- Completion must be adjudicated by tool evidence and the goal contract, never by model self-report alone.
- File, build, test, device, Git, and external-system states are saved as structured facts; summaries only serve navigation.
- Side-effecting operations must be identifiable, approvable, and recoverable; external state must be verified first when safe replay is impossible.
- Prioritize improving the success rate and composition power of the existing 201 tools before adding new ones.
- Every milestone must deliver implementation, migration, tests, docs, metrics, and failure-recovery verification together.
- Eliminating all historical warnings is not a precondition for any single feature, but no new warnings or technical debt may be introduced.

## 2. Current Baseline

The project already has the foundation this roadmap needs:

- Durable Runs, execution steps, checkpoints, goal contracts, and structured tool results.
- Durable queue, DAG, multi-worker leases, fencing, and crash recovery.
- Dedicated OS threads for tools, panic isolation, side-effect prepared/committed states, and stuck metrics.
- SQLite sessions, messages, snapshots, memories, knowledge base, and run events.
- HarmonyOS project detection, Hvigor/OHPM, HDC, HAP deployment, Hilog, LSP, and API knowledge capabilities.
- Reliability control plane, SLOs, fault-scenario evals, and worker/tool crash E2E gates.

This roadmap evolves incrementally on top of these capabilities; it does not build a parallel runtime.

## 3. Phase One: Long-Session Execution Kernel V2

### 3.1 Tiered Context

- [x] `LC-01` Define a unified `ConversationContextV2` splitting current window, task state, project memory, and history archive.
- [x] `LC-02` Add source, time, scope, confidence, version, and invalidation conditions to context entries.
- [x] `LC-03` Compose recent messages, current errors, active files, and pending approvals into a bounded hot context.
- [x] `LC-04` Save goals, plans, acceptance criteria, completed items, blockers, and next steps as a structured task snapshot.
- [x] `LC-05` Store per-project architecture conventions, build commands, module responsibilities, user preferences, and confirmed decisions.
- [x] `LC-06` Replace historical tool outputs with artifact references and summaries; raw content stays in traceable storage.
- [x] `LC-07` Implement memory invalidation policies for project switches, branch switches, file changes, and device changes.

### 3.2 Summaries and Fact Retention

- [x] `LC-08` Build an incremental summarization flow to avoid re-summarizing the full session every round.
- [x] `LC-09` Reconcile facts before and after summarization, covering file modifications, test results, build artifacts, Git state, and device state.
- [x] `LC-10` Summaries must not override unfinished tasks, user constraints, pending approvals, or failure reasons.
- [x] `LC-11` Record summary versions and the message cursors they cover, supporting location and rebuild.
- [x] `LC-12` When structured facts conflict with natural-language summaries, verifiable facts win and a correction event is emitted.

### 3.3 Recovery, Branching, and Goal Changes

- [x] `LC-13` Resume current tasks after app restart from the Durable Run, task snapshot, and event cursors.
- [x] `LC-14` Verify workspace, processes, build artifacts, devices, and remote Git state before recovery; never blindly replay side effects.
- [x] `LC-15` Support pause, resume, cancel, retry, and resume from the latest safe checkpoint.
- [x] `LC-16` On goal changes, generate a contract diff, cancel no-longer-applicable steps, and recompute acceptance criteria.
- [x] `LC-17` Support session branching from messages, checkpoints, build failures, or Git commits.
- [x] `LC-18` Branch merges only merge structured decisions, artifacts, and verification evidence — never concatenate full contexts directly.
- [x] `LC-19` Sub-agents communicate with the main task via task contracts, scoped contexts, and structured results.

### 3.4 Long-Session Product Experience

- [x] `LC-20` Show the current goal, phase, context budget, summary count, and recovery points in the Workspace.
- [x] `LC-21` Provide explanation entry points for "why remembered", "why forgotten", "fact source", and "reload history".
- [x] `LC-22` Let users pin key messages, decisions, files, and acceptance criteria.
- [x] `LC-23` Give clear notices for imminent context compaction, goal conflicts, and recovery verification failures.

### 3.5 Phase Acceptance

- [x] After 100+ rounds, goals, completed items, blockers, and next steps can still be stated accurately.
- [x] After hours of continuous execution, the app can resume from a safe checkpoint after restart.
- [x] After context compaction, file modifications, test results, uncommitted state, and user constraints stay accurate.
- [x] After mid-flight goal changes, dangerous or irrelevant steps from the old contract are no longer executed.
- [x] Crash recovery does not duplicate deploys, commits, pushes, deletions, or external writes.
- [x] Key conclusions are traceable to message, file, tool-result, or external-state evidence.

### 3.6 Long-Session Hardening and Observability (2026-08-22 batch)

Hardening items completed after the long-session foundation, all landed and committed (`bcee1e2`/`8bb9865`/`3015a9c`, see [CONTEXT_V2.md](CONTEXT_V2.md)):

- [x] `LC-24` Context budget is dynamically profiled by session phase: four profiles `balanced/explore/execute/verify`, derived at read time from `agent_runs.phase`; the hot window always absorbs remaining budget; persisted `budget_json` is audit-only.
- [x] `LC-25` A "compression imminent" warning fires at 85% of the context window: `chat-context-warning` event `compression_imminent` + frontend notification, noting that pinned items and structured facts are reconciled first.
- [x] `LC-26` The Workspace context panel shows a tiered budget bar (5 segment colors), profile label, invalidation count, and a session-health panel, auto-refreshing after compaction events.
- [x] `LC-27` 100+ round long-session stress regression: after 100 interleaved rounds of messages/fact flips/compaction checkpoints/invalidations/reconciliations, fact versions stay intact, budgets stay bounded, and cursors stay monotonic (`long_session_100_round_stress_keeps_facts_and_budget_bounded`).
- [x] `LC-28` Multi-token fuzzy knowledge retrieval: split tokens into independent LIKE queries taking the union, ranked by token hit count → character overlap → historical hit count; a single token degrades to the original exact LIKE (`search_knowledge_fuzzy`).
- [x] `LC-29` Session health and summary degradation detection: four metrics (compaction count / fact-flip rate / reconciliation correction count / budget usage), degradation verdicts (summary depth ≥2, corrections ≥1, flip rate >30%; compaction count is only a secondary signal since LC-34) produce pinned key conclusions and new-session suggestions (`get_session_health` + `076_session_health.sql`).

### 3.7 Long-Session Correlation Strengthening (audit finding, implemented)

Improvements found in the full correlation audit (2026-08-22), all landed (2026-08-22 correlation-strengthening batch, see [CONTEXT_V2.md](CONTEXT_V2.md) §12/§13 and [EVALUATION_RUN_SNAPSHOTS.md](EVALUATION_RUN_SNAPSHOTS.md)):

- [x] `LC-30` Unified task-phase classifier: the audit found the execution loop's `recommended_phase` was process-local while `agent_runs.phase` holds real runtime state (initializing/recovering/orchestrating); implemented as `BudgetProfile::from_goal_or_phase` joint derivation — structured phase first, goal keywords as fallback, keyword semantics aligned with `TaskPhase` in `capabilities.rs` (Modify→Execute, Deliver→Verify, Recover→Execute); dynamic budgets no longer always land on Balanced. Depends on `LC-24`/`TC-09`.
- [x] `LC-31` The context panel shows fact-invalidation reason details (`recent_invalidations`: superseded/project switch/device change, etc., sourced from `invalidation_reason`), completing the "why forgotten" explanation entry point. Depends on `LC-26`/`LC-21`.
- [x] `LC-32` The core metrics table adds long-session degradation metrics (fact-flip rate, degradation warning count); eval snapshots (`EVALUATION_RUN_SNAPSHOTS.md`) record compaction and flip counts (`EvalLongSessionMetrics`, from real EC-19 scenario runs), making degradation measurable. Depends on `LC-29`/`EC-14`.
- [x] `LC-33` Compaction-warning events are written into the session event stream (`context_compress` event: three triggers — proactive/over-limit/manual — all traced uniformly, replayable in Timeline), measuring post-warning user pinning behavior and the "compression without warning" experience improvement. Depends on `LC-25`/`LC-29`.
- [x] `LC-34` Health degradation verdicts derive the true summary depth from the summary-coverage cursor chain (`summary_coverage`/`summary_depth`, LC-11 cursor chain), with compaction count as a secondary signal only, replacing the "compaction ≥2" heuristic. Depends on `LC-29`/`LC-11`.

## 4. Phase Two: Agent Toolchain Quality Engineering

### 4.1 Unified Tool Contract

- [x] `TC-01` Unify `ToolResultV2` fields for all tools: status, modifications, artifacts, verification, recovery, suggestions, and error envelope.
- [x] `TC-02` Add side-effect level, idempotency capability, timeout, cancel, retry, and approval metadata to tools.
- [x] `TC-03` Save long outputs as artifacts; the model context only receives bounded summaries and references.
- [x] `TC-04` Unify sensitive-information redaction rules covering tokens, certificates, signing materials, environment variables, and device identifiers.
- [x] `TC-05` Distinguish success, partial success, verification failure, pending approval, retryable failure, and permanent failure.
- [x] `TC-06` Tools declare result validators; side-effecting tools declare compensation or manual-recovery guidance.

### 4.2 Capability Packs and Tool Selection

- [x] `TC-07` Build capability packs for project understanding, compile fixing, feature development, refactoring, build & deploy, device diagnostics, and Git delivery.
- [x] `TC-08` Each capability pack defines the minimal tool set, recommended order, stop conditions, and acceptance conditions.
- [x] `TC-09` Dynamically trim tool descriptions by task phase, avoiding exposing all tools to the model every round.
- [x] `TC-10` Detect functionally duplicated, long-unused, and high-failure-rate tools, forming merge/hide/fix lists.
- [x] `TC-11` Provide schema-level correction suggestions for parameter errors without auto-fixing sensitive parameters.
- [x] `TC-12` The tool selector ranks by historical success rate, estimated cost, duration, side effects, and current environment.

### 4.3 Execution and Verification Loop

- [x] `TC-13` Institutionalize the "understand goal → verifiable plan → minimal tool set → execute → verify → accept" loop.
- [x] `TC-14` After file modifications, automatically pick formatting, static checks, tests, or build verification — write-only evidence is not enough.
- [x] `TC-15` Read real state after deploy, Git, and external writes to confirm results.
- [x] `TC-16` Multi-step tool flows support transaction boundaries, checkpoints, compensation actions, and post-failure fallback plans.
- [x] `TC-17` Add isolation tests for stuck threads, orphan processes, output floods, and uncancellable calls.
- [x] `TC-18` Treat human approvals as recoverable state — after restart they can keep waiting or cancel safely.

### 4.4 Metrics and Governance

- [x] `TC-19` Collect tool success rate, parameter-error rate, timeout rate, retry rate, cancel latency, and average duration.
- [x] `TC-20` Collect each tool's contribution to final task completion, distinguishing "call succeeded" from "goal advanced".
- [x] `TC-21` Set SLOs for duplicate side-effect rate, wrong-tool-selection rate, and invalid-call rate.
- [x] `TC-22` The reliability panel supports comparison by tool, capability pack, model, project, and version.
- [x] `TC-23` Every tool-protocol change ships with a compatibility migration or an explicit version boundary.

### 4.5 Phase Acceptance

- [x] All registered tools have complete metadata and structured results, with backward-compatible unknown fields.
- [x] High-frequency tools have success, failure, timeout, cancel, retry, and recovery tests.
- [x] The number of tools exposed to the model for typical tasks drops significantly without lowering completion rates.
- [x] The duplicate side-effect execution rate is measurable and stays at zero in crash evals.
- [x] On tool failure, users can see the cause, impact, completed parts, and actionable next steps.

## 5. Phase Three: HarmonyOS Development Loop

### 5.1 Project Semantic Model

- [x] `HM-01` Unify parsing of projects, products, modules, HAP/HSP/HAR, Ability, ExtensionAbility, and dependency relationships.
- [x] `HM-02` Structurally parse `build-profile.json5`, `module.json5`, `oh-package.json5`, and lockfiles.
- [x] `HM-03` Build route, page, permission, system-capability, and cross-module reference graphs.
- [x] `HM-04` Identify SDK/API levels, device types, build modes, signing configuration, and product differences.
- [x] `HM-05` Incrementally update the project model on file changes, marking potentially affected modules and verification scope.
- [x] `HM-06` Provide a traceable project overview and impact analysis in the Workspace.

### 5.2 Build and Dependencies

- [x] `HM-07` Unify environment checks, OHPM installs, Hvigor builds, and artifact discovery into a recoverable workflow.
- [x] `HM-08` Structure Hvigor/ArkTS errors, extracting file, location, error code, phase, and root-cause category.
- [x] `HM-09` Provide targeted diagnostics for dependency conflicts, cache corruption, missing SDKs, signing failures, and API incompatibility.
- [x] `HM-10` Select modules, products, and modes by impact scope before building, avoiding needless full builds.
- [x] `HM-11` Maintain a HAP/HSP/HAR artifact manifest recording signing state, timestamp, hash, product, and originating step.
- [x] `HM-12` Deployments default to the newest verifiable signed artifact and require user confirmation on ambiguity.

### 5.3 Devices, Deployment, and Runtime Diagnostics

- [x] `HM-13` Build a unified device state: connection, authorization, OS version, API level, architecture, screen, and available capabilities.
- [x] `HM-14` Connect device discovery, install, Ability launch, state confirmation, log capture, and uninstall recovery.
- [x] `HM-15` Correlate Hilog, ArkTS exceptions, native crashes, ANRs, and build info into a single Run.
- [x] `HM-16` Support serial or controlled parallel deployment across devices, recording each device's result independently.
- [x] `HM-17` Add UI tree, screenshot, interaction-step, and key-page assertion capabilities.
- [x] `HM-18` Add basic performance verification for startup time, CPU, memory, battery, and package size.
- [x] `HM-19` Cover offline devices, authorization denial, install conflicts, permission denial, background recovery, and weak-network scenarios.

### 5.4 SDK and API Intelligence Layer

- [x] `HM-20` Build an incrementally updatable index of APIs, types, permissions, system capabilities, and versions for the local SDK.
- [x] `HM-21` Retrieval results always bind to the current project's SDK/API level, annotating introduced, deprecated, and replacement versions.
- [x] `HM-22` Map ArkTS compile errors to API changes, type constraints, and official definitions.
- [x] `HM-23` Check consistency among API usage, permission declarations, device capabilities, and module configuration.
- [x] `HM-24` Provide verified HarmonyOS migration advice for common Android/Web/TypeScript implementations.
- [x] `HM-25` After code generation, close the loop via local type definitions, LSP diagnostics, and build results.

### 5.5 Phase Acceptance

Execution records, evidence, and environment blockers are in [HarmonyOS Phase Three Acceptance Records](HARMONY_STAGE5_ACCEPTANCE.md). Only entries with matching evidence may be checked off.

- [x] Can explain a real multi-module project's entry points, dependencies, routes, permissions, and build-artifact relationships. Evidence in [HarmonyOS Phase Three Acceptance Records](HARMONY_STAGE5_ACCEPTANCE.md) (hongmeng-app dual-module entry+application: EntryAbility/ApplicationAbility entry points, 124 page routes, INTERNET permission, module dependencies, and dual-module signed-artifact relationships fully parsed).
- [x] Can locate root causes of common ArkTS/Hvigor errors, fix them, and pass the build.
- [x] Can install, launch, collect logs, locate anomalies, fix, and re-verify on a real device. Evidence in [HarmonyOS Phase Three Acceptance Records](HARMONY_STAGE5_ACCEPTANCE.md) (2026-08-22 on a CHZ-AL00 device: signed HAP build → install → launch → hilog baseline → injected JSON parse fault → hilog located SyntaxError/RuntimeError → fixed → rebuilt → re-verified onCreate/loadContent success).
- [x] Can identify SDK/API-level incompatibilities and give alternatives based on local definitions.
- [x] In multi-device tasks, a single device's failure does not pollute other devices' results, and recovery does not duplicate successful deployments. Evidence in [HarmonyOS Phase Three Acceptance Records](HARMONY_STAGE5_ACCEPTANCE.md) (real device + emulator dual-device external acceptance: the real device's PID stayed unchanged during the emulator uninstall failure; after recovery only the emulator was reinstalled; anti-replay content-hash gate automated tests passed).

## 6. Phase Four: Ecosystem Integration and Productization

### 6.1 HarmonyOS Ecosystem Connections

- [x] `EC-01` Connect traceable HarmonyOS/OpenHarmony SDK and official-doc indexes.
- [x] `EC-02` Enhance ohpm package search, version comparison, compatibility, license, and security information.
- [x] `EC-03` Support analyzing GitHub/Gitee HarmonyOS open-source projects and extracting reusable project patterns.
- [x] `EC-04` Build a knowledge base of third-party library compatibility, common errors, and device differences.
- [x] `EC-05` Support interop with DevEco project configuration without depending on IDE private state.
- [x] `EC-06` Publishing, signing, certificates, and app-store operations require explicit approvals and isolated credentials. Governance boundaries, the approval matrix, and regression requirements are in [HarmonyOS Release and Signing Governance](HARMONY_RELEASE_GOVERNANCE.md).

### 6.2 Skill, MCP, and Workflow Ecosystem

- [x] `EC-07` Define the HarmonyAgent Skill/capability-pack version spec, permission declarations, and compatibility scope. Spec, legacy-format policy, and content-drift gates in [Skill and Capability Pack Spec v1](SKILL_CAPABILITY_SPEC.md).
- [x] `EC-08` Support importing, validating, enabling, disabling, and upgrading workflow templates. Format, compatibility, permission differences, approval, and version-archive rules in [Workflow Template Spec v1](WORKFLOW_TEMPLATE_SPEC.md).
- [x] `EC-09` MCP services are authorized per project, scoping tools, directories, networks, and credentials. Fail-closed migration, global-template policy, call-time gates, and process environment boundaries in [MCP Project Authorization and Scope](MCP_PROJECT_AUTHORIZATION.md).
- [x] `EC-10` Add signing, provenance, audit, rate limiting, and fault isolation for third-party extensions. Provenance tiering, spec payloads, persistent quotas/circuit breakers, and recovery rules in [Third-Party Extension Supply Chain and Runtime Governance](EXTENSION_GOVERNANCE.md).
- [x] `EC-11` Support team-shared project memory, engineering conventions, and eval sets with provenance and change history. Package format, coexistence of conflicts, version upgrades, per-item history, conservative rollback, and eval safety boundaries in [Team-Shared Project Context Spec v1](TEAM_SHARING.md).
- [x] `EC-12` Build exportable problem-reproduction bundles, redacted by default and generated only after user confirmation. Collection caps, attachment rejection, preview-summary binding, explicit confirmation, ZIP manifest, and validation boundaries in [Problem Reproduction Bundle Spec v1](REPRODUCTION_BUNDLES.md).

### 6.3 Eval and Release Governance

- [x] `EC-13` Build a fixed eval suite covering new-project scaffolding, compile fixing, cross-module modifications, real-device diagnostics, and long-session recovery. Versioned scenarios, evidenced HarmonyOS fingerprints, production-kernel reuse, negative examples, and boundaries in [Fixed Evaluation Suite and HarmonyOS Fingerprinting](FIXED_EVALUATION_SUITE.md).
- [x] `EC-14` Eval records capture model, prompt, tool versions, SDK, device, cost, duration, and final evidence. Versioned schema, privacy boundaries, historical compatibility, and evidence summaries in [Evaluation Run Snapshots](EVALUATION_RUN_SNAPSHOTS.md).
- [x] `EC-15` CI blocks significant regressions in task completion rate, duplicate side-effect rate, recovery rate, or critical latency. Baseline save/restore, comparison rules, tolerances, and local reproduction in [Evaluation CI Baseline Gates](EVALUATION_CI_GATES.md).
- [x] `EC-16` Build a real-failure sample feedback loop, redacting failures into regression scenarios. Validation, distillation, registration, and gate flow in [Failure Reflow](FAILURE_REFLOW.md).
- [x] `EC-17` Release notes automatically aggregate migrations, tool-protocol changes, risks, compatibility, and rollback paths. The generator is `scripts/gen-release-notes.py`, wired into the Create Release step of release.yml.
- [x] `EC-18` Define version-compatibility policies for the database, workflows, Skills, tool protocol, and knowledge index. Asset version manifest and verification entry point is `src-tauri/src/agent/versioning.rs`; policy in [Version Compatibility Policy](VERSION_COMPATIBILITY.md).
- [x] `EC-19` The fixed eval suite gains a 100-round long-session compaction-recovery case (multiple compactions + fact conflicts then acceptance), included in the EC-15 baseline gate so stress scenarios are protected against regression. Depends on `LC-27`/`EC-13`/`EC-15`.

### 6.4 Phase Acceptance

- [x] SDK, docs, ohpm, and open-source knowledge all trace source, version, and update time. Evidence: `environment_check` uniformly presents source/version/update-time/entry/coverage for local `.d.ts`, official API changes and reference libraries, and OpenHarmony doc mirrors, degrading missing items; ohpm audit records registry sources and version relationships; open-source pattern extraction keeps `source_file`/`source_kind` and locked versions.
- [x] Third-party extensions cannot bypass project permissions, side-effect approvals, or the audit chain. Evidence: MCP is authorized per project with tool/directory/network/credential scoping (`MCP_PROJECT_AUTHORIZATION.md`), extensions are governed uniformly by registration, signature verification, quotas, circuit breakers, and audit (`EXTENSION_GOVERNANCE.md`), all with fail-closed tests.
- [x] Every release has repeatable long-session, tool-recovery, and real-device HarmonyOS eval results. Evidence: `agent_harmony_fixed_v3` covers long-session recovery, tool recovery, and recorded real-device faultlog diagnostics; every CI/release run executes fixed evals and saves execution snapshots and baselines (`EVALUATION_CI_GATES.md`); release notes automatically aggregate eval and migration evidence; real-device scenarios validate the diagnostics kernel and never pass off recorded faultlogs as live device acceptance.
- [x] Team-shared content is auditable and revocable without overwriting user-local facts.

## 7. Cross-Cutting Quality Tasks

These tasks span all phases and must not be postponed to the end:

- [x] `Q-01` New migrations must have forward-rollback, duplicate-execution protection, and legacy-data compatibility tests. Evidence: the migration registry and `db/mod.rs` tests cover every migration being executable and legacy-compatible columns existing (e.g., 075 snapshot column assertions).
- [x] `Q-02` New background tasks must have cancellation, timeouts, resource caps, and app-exit behavior. Evidence: scheduler leases/claims/cancellations and execution attempt ledgers, plus tool-thread stuck attribution, all with E2Es.
- [x] `Q-03` New side effects must have an approval level, idempotency policy, validator, and audit event. Evidence: tool contracts declare effect/recovery/retry_safe/idempotency, and governance audit leaves a uniform trace.
- [x] `Q-04` New UI states must cover loading, empty data, partial success, failure, recovery, and no-permission states. Evidence: all 16 pages declare an `@ui-states` coverage list at the top (LanPage, a pure container page, declares delegated); `scripts/check-ui-states.py` validates declaration existence, state-name validity, and declaration-to-code evidence consistency (self-tests cover four regressions: illegal states, declarations without evidence, and delegated misuse); wired into the quality.yml Frontend gate; spec and status audit matrix in [UI State Coverage Spec](UI_STATE_COVERAGE.md).
- [x] `Q-05` Logs and artifacts are redacted by default; plaintext credentials and signing private keys are never persisted.
- [x] `Q-06` Every milestone runs frontend tests, lint, build, Rust tests, Clippy, and related E2Es. Evidence: quality.yml runs the full frontend tests/lint/build, Rust tests, the fixed-eval gate, the execution-kernel gate, crash-recovery E2Es, and Clippy.
- [x] `Q-07` Gradually converge existing formatting, Clippy, ESLint, and dependency-security warnings and establish a no-new-baseline. Evidence: ESLint 9→0 (real react-hooks fixes and reasoned exemptions; quality.yml blocks regressions with `--max-warnings 0`); Clippy 338→44 (two rounds of clippy --fix plus batch sort_by_key conversions, fixing suspected real bugs like lockfile truncation/process termination; all mechanical warnings cleared; the remaining 44 are all structural — 31 too_many_arguments + 13 type_complexity — kept as baseline); `scripts/check-warnings.py` deduplicates unique clippy warnings by (lint, location), wired into quality.yml dual-platform gates, blocking above baseline; all fixes verified with zero regressions across cargo test's 667 tests.
- [x] `Q-08` Counts, paths, interfaces, and states in docs are validated by scripts or tests to prevent drift. Evidence: `scripts/check-docs.py` extracts 8 categories of counts from code truth sources (tools 201/migrations 77/IPC 298/commands 38/services 56/pages 16/agent 36/tools 29-30) and validates 54 patterns across 11 Chinese and English documents, checking ROADMAP and docs relative links, backticked code paths, and tests/scripts referenced by quality/release.yml; wired into quality.yml dual-platform gates; self-tests cover five regressions, including tampered Chinese and English counts, deleted link targets, broken CI test names, and references to nonexistent paths (see [Documentation Drift Checks](DOCUMENTATION_CHECKS.md)).

## 8. Core Metrics

| Dimension | Metrics | Direction |
|---|---|---|
| Tasks | final task completion rate, one-shot completion rate, human intervention count | completion up, intervention down |
| Long sessions | fact retention rate, recovery success rate, wrong-continuation rate, fact-flip rate, degradation warning count | retention/recovery up, wrong-continuation down; flips and warnings down |
| Tools | parameter-error rate, invalid-call rate, timeout rate, duplicate side-effect rate | continuously down, duplicate side effects near zero |
| HarmonyOS | build-fix rate, real-device deployment success rate, diagnostic hit rate | continuously up |
| Efficiency | completion duration, tokens, cost, tool-call count | down without lowering completion quality |
| Experience | user cancel rate, repeated-explanation count, post-recovery confirmation count | continuously down |

Metrics must be grouped by version, model, project type, and task scenario so averages don't hide severe regressions.

## 9. Recommended Milestones

### M1: Long-Session Fact Foundation

- Scope: `LC-01` to `LC-12`, `Q-01` to `Q-05`.
- Deliverables: Context V2, task snapshots, incremental summaries, fact reconciliation, migrations, and regression tests.
- Exit criteria: after 100 rounds of session compaction, key-fact retention tests pass.

### M2: Reliable Recovery and Branching

- Scope: `LC-13` to `LC-23`, long-session phase acceptance.
- Deliverables: restart recovery, goal changes, branching, sub-task result protocol, and Workspace visualization.
- Exit criteria: no duplicate side effects in restart and crash scenarios; users can trace recovery rationale.

### M3: Toolchain Quality Loop

- Scope: `TC-01` to `TC-23`.
- Deliverables: tool contracts, capability packs, dynamic trimming, metrics, and complete failure UX.
- Exit criteria: the high-frequency tool fault matrix passes; tool exposure and invalid calls drop significantly for typical tasks.

### M4: HarmonyOS Project and Build Intelligence

- Scope: `HM-01` to `HM-12`, `HM-20` to `HM-25`.
- Deliverables: project graph, build diagnostics, artifact manifest, and SDK/API intelligence layer.
- Exit criteria: understanding, modification, and build evals pass on a real multi-module project.

### M5: Real-Device Runtime Loop

- Scope: `HM-13` to `HM-19`, HarmonyOS phase acceptance.
- Deliverables: multi-device state, deployment, launch, logs, crashes, UI, and performance verification.
- Exit criteria: at least one end-to-end real-device fault-fix scenario passes stably.

### M6: Ecosystem and Scale Governance

- Scope: `EC-01` to `EC-18`, all cross-cutting quality tasks.
- Deliverables: ecosystem indexes, extension security, team sharing, fixed evals, and release gates.
- Exit criteria: third-party integrations are auditable; releases carry repeatable quality evidence.

## 10. Definition of Done for Each Task

A task may only be checked off when ALL of the following hold:

1. The implementation is wired into a real user path, not an uncalled isolated module.
2. Data structures have migration and compatibility policies; side effects have recovery policies.
3. Unit, integration, or E2E tests cover success paths and key failure paths.
4. Observable metrics, structured logs, or audit events are produced.
5. User-visible behavior is reflected in the README, architecture doc, or a topic doc.
6. Frontend tests, lint, build, and relevant Rust tests pass; zero new warnings.
7. Acceptance evidence is traceable to Runs, steps, tool results, artifacts, or external state.

## 11. Current Immediate Execution Queue

The next development round starts at M1, progressing in this order:

- [x] `NOW-01` Inventory existing session message, snapshot, memory, Run, step, and event tables; produce the Context V2 data mapping.
- [x] `NOW-02` Define `ConversationContextV2`, `TaskSnapshotV2`, and `ContextArtifactRef` Rust types and versioning policy.
- [x] `NOW-03` Add migrations saving task snapshots, summary cursors, fact references, and invalidation state.
- [x] `NOW-04` Implement the context budget allocator and hot-context assembler.
- [x] `NOW-05` Implement incremental summaries and structured-fact reconciliation.
- [x] `NOW-06` Wire Context V2 into the Agent loop in `commands/chat.rs`, keeping a controllable fallback switch.
- [x] `NOW-07` Show context, summaries, and fact sources in the Workspace.
- [x] `NOW-08` Add 100+ round, compaction, restart, and fact-conflict regression tests.
- [x] `NOW-09` Update the architecture doc, migration notes, metrics, and troubleshooting guide.
- [x] `NOW-10` After passing M1 exit criteria, proceed to recovery and session-branch implementation.

If new tasks surface during execution, file them into the existing phases with dependencies noted; only items affecting milestone exits enter the current queue, so the roadmap is not fragmented by ad-hoc feature requests.
