# DevEco Switch · CHANGELOG

> A desktop AI coding IDE for HarmonyOS / OpenHarmony developers.
> This file records user-visible changes, migration notes, and rollback guidance in reverse version order.

[简体中文](CHANGELOG.md) | English

---
## Unreleased — Batch 82: checkpoint forks no longer lose task state (2026-10-22)

Scenario: "fork" on the timeline creates a new conversation from a checkpoint by
copying messages and audit events, but the conversation ledger and task list
(`conversation_todos`) were not copied. The new branch carried the full dialogue up to
the anchor yet had no execution trace or todos: the task ledger card was empty,
continuing the task started from scratch, and the per-round todo injection into the
model context had nothing to inject — the branch looked like it continued history
while silently dropping the task's current state.

- `fork_conversation` now delegates to a testable `fork_conversation_conn`, and the
  anchor kind decides whether task state is inherited:
  - `checkpoint`: copies the ledger and task list recorded in that snapshot (snapshots
    since migration 089 always carry a list; legacy ones only contribute the ledger);
  - `latest`: nothing is cut off, so the current ledger and list are inherited;
  - `message` / `build_failure` / `git_commit`: later trace may exist past the cutoff,
    so the current ledger/list do not represent the branch — inheriting nothing is
    safer than leaking the future branch into the new conversation;
- Opening the new branch already reloads the task list, so no frontend change is needed.

### Verification

3 Rust unit tests added (a checkpoint fork inherits the snapshot ledger/list and copies
only messages up to the anchor; a latest fork inherits current state; a message-anchor
fork leaks neither the current ledger nor the list). Rust full suite **1320/1320**
(+3) · frontend vitest **252/252** (no frontend changes) · `tsc` / `eslint` / clippy
baseline 57/57 · i18n keys unchanged (2298, union 2300) · all three i18n gates,
21 gate self-tests, `check-docs`, and the production build pass. `TOOL_SPECS` to **204**
(unchanged in this batch), and database migration count reaches **89** (unchanged).

---
## Unreleased — Batch 81: the timeline now has a "just finished" checkpoint (2026-10-22)

Scenario: conversation snapshots were only saved before each request round, anchored
at the last visible message *before* that round, while the task's final reply is only
persisted by `persist_turn` after the loop ends. Nothing was captured in between, so
after a task completed the newest timeline entry still sat mid-task and restoring it
archived the final answer; pure Q&A (no tools) produced no snapshot at all because of
the pre-round "no execution trace, don't save" rule, leaving the timeline empty; and
the newest snapshot in multi-round tasks was never the visible tail, so the "current"
marker never appeared.

- When a task finishes (after both the message and the final ledger are persisted), a
  checkpoint is appended: it anchors the final reply and carries the final ledger
  (empty when the completed task's ledger was cleared), so the newest timeline entry is
  the "just finished" state and pure Q&A becomes rewindable too; failure to append it
  does not affect task finalization;
- Snapshot saving now deduplicates identical states: the same anchor + ledger + todo
  list keeps a single row, so a pre-round save and the final append landing on the same
  state (empty round, resuming right after a restore) do not create duplicate entries.

### Verification

3 Rust unit tests added (the final snapshot anchors the final reply and carries the
persisted ledger; a completed task does not resurrect an old ledger; identical states
are saved once). Rust full suite **1317/1317** (+3) · frontend vitest **252/252**
(no frontend changes in this batch) · `tsc` / `eslint` / clippy baseline 57/57 · i18n
keys unchanged (2298, union 2300) · all three i18n gates, 21 gate self-tests,
`check-docs`, and the production build pass. `TOOL_SPECS` to **204** (unchanged in
this batch), and database migration count reaches **89** (unchanged).

---
## Unreleased — Batch 80: time travel brings the task list back with the history (2026-10-22)

Scenario: the task list (`conversation_todos`) is conversation-level persisted state
and is injected into the model context every round. Restoring an earlier snapshot used
to archive messages and rewind the ledger only, while the list stayed on the archived
future branch: the UI showed future todos, and the next round fed them back to the
model, so the rewound conversation kept following the old plan; the previous task's
tool/sub-agent trace panels likewise kept showing the future branch.

- Migration 089 adds `todos_json` to `conversation_snapshots`: saving a snapshot now
  also records the task list at that moment (an empty list is stored as `[]`);
- Restoring writes the list back to `conversation_todos` in the same transaction and
  refreshes the in-memory copy after commit, so messages, ledger, and list rewind
  together; snapshots older than migration 089 have no such field (NULL), and since the
  list at that time cannot be inferred, restore leaves the current list untouched
  instead of clearing it;
- The frontend `restoreToSnapshot` clears the tool/sub-agent trace panels, hides the
  future-branch list immediately, and reloads it from the backend (the database wins).

### Verification

3 Rust unit tests added (the list rewinds with the snapshot; an empty list at snapshot
time clears future todos; a legacy snapshot neither changes the list nor invents a DB
row). Rust full suite **1314/1314** (+3) · frontend vitest **252/252** (restore case
extended, count unchanged) · `tsc` / `eslint` / clippy baseline 57/57 · i18n keys
unchanged (2298, union 2300) · all three i18n gates, 21 gate self-tests, `check-docs`,
and the production build pass. `TOOL_SPECS` to **204** (unchanged in this batch), and
database migration count reaches **89**.

---
## Unreleased — Batch 79: forced aborts no longer leave unanswerable approvals/plans behind (2026-10-22)

Scenario: while a task is waiting on tool approval or plan review, deleting a
conversation, clearing all content, deleting a project, or reloading the WebView
aborts the tokio task immediately. Abort skips the `remove` cleanup inside the
waiting future, but entries in `ToolApprovalState` / `PlanApprovalState` do not
disappear with the future: after deletion/clearing they leak in memory, and after a
reload the frontend restores a "pending" badge and approval dialog from
`list_pending_confirmations` even though the task no longer exists. Approving or
rejecting then has no effect, and the dialog only goes away after a restart.

- `ToolApprovalState` / `PlanApprovalState` gain `clear_conversation`: deleting a
  conversation, clearing all content, deleting a project, and the task shell being
  dropped (WebView reload) all clear the target conversation's approval/plan wait
  entries; the watchdog now reuses the same implementation instead of keeping its
  own `retain`;
- The task shell clears leftovers after `join` returns (covering panics and other
  non-cooperative exits) before unregistering;
- `list_pending_confirmations` gains a backstop: entries whose wait channel is closed
  (the waiter is gone) are discarded and their interaction ledger rows are marked
  `interrupted`, so badges and restore-on-open never point at dead requests;
- The frontend periodic sync now treats the server as the source of truth: expired
  approval dialogs and plan cards close automatically; requests that arrive while
  the query is in flight are protected by a "known before the query" set and are
  not closed by the stale snapshot.

### Verification

2 Rust unit tests added (a forced stop clears only the target conversation's
approvals/plans and closes the wait channels; closed waiters are pruned from the
list) and 2 frontend unit tests (a stale server snapshot closes the dialog and plan
card; a confirmation arriving mid-sync is not closed by the stale snapshot). Rust
full suite **1311/1311** (+2) · frontend vitest **252/252** (+2) · `tsc` / `eslint` /
clippy baseline 57/57 · i18n keys unchanged (2298, union 2300) · all three i18n
gates, 21 gate self-tests, `check-docs`, and the production build pass. `TOOL_SPECS` to **204** and database migration count reaches **88** (both unchanged in this batch).

---
## Unreleased — Batch 78: rewinding history no longer resurrects the rolled-back branch's task (2026-10-22)

Scenario: restoring a historical snapshot or deleting a run of messages only rewrites
`messages` and the ledger; `agent_runs` / `run_events` must be retained as the audit
source of truth. But when a conversation is reopened, the frontend reads the "latest
durable Run" to restore the unfinished task, partial output, and tool trace. After time
travel that latest Run usually belongs to the archived future branch, so the
rolled-back unfinished task reappears in the UI and its interrupted output can even be
injected into the rewound conversation. Batch 76 only cleared the in-process view
state; switching conversations or reloading brought it back.

- Migration 088 adds `history_rewritten_at_ms` to `conversations`: restoring a
  snapshot or deleting messages raises the Run recovery floor in the same transaction;
- `latest_run` only returns Runs started after the floor. Runs before it (including
  the future branch) remain fully available in `agent_runs` / `run_events` for audit,
  but are no longer returned as resumable state; a floor of 0 keeps the old behavior;
- Runs started after the rewrite recover normally; resend/regenerate is unaffected;
- The floor shares the message/ledger transaction, so either everything commits or
  everything rolls back — never "messages rewound but the Run is still resumable".

### Verification

3 Rust unit tests added (floor filtering with a fresh Run, snapshot restore raising
the floor and hiding the future branch, message deletion raising the floor with
rollback on failure). Rust full suite **1309/1309** (+3) · frontend vitest
**250/250** (unchanged, no frontend changes) · `tsc` / `eslint` / clippy baseline
57/57 · i18n keys unchanged (2298, union 2300) · all three i18n gates,
21 gate self-tests, `check-docs`, and the production build pass. Tool count
unchanged: `TOOL_SPECS` to **204**; database migration count reaches **88**.

---
## Unreleased — Batch 77: snapshot anchors resolve by message id, so reused rowids no longer land on new messages (2026-10-22)

Scenario: time-travel snapshots and branch checkpoints both recorded their anchor as
`messages.rowid`. The `messages` table has no `AUTOINCREMENT`, so SQLite reuses the
maximum rowid after the tail message is deleted: an old snapshot's rowid can be taken
over by a completely different new message. Restoring such a snapshot treats the new
message as the anchor and archives/restores the wrong branch, and branch checkpoints
split the conversation at the wrong place. The deletion cleanup added in batch 72 only
covers deletions performed by this app; it cannot eliminate rowid reuse itself.

- Migration 087 adds `anchor_message_id` to `conversation_snapshots`: new snapshots
  record both the rowid and the immutable message id;
- Snapshot restore resolves the anchor by id first (validating conversation
  ownership) and fails with an explicit error without writing anything when that
  message is gone; checkpoints also prefer the id and report that the checkpoint
  anchor message has been deleted when it is missing;
- Snapshots created before the migration have a NULL `anchor_message_id` and keep
  using the rowid for compatibility;
- The snapshot list's "is current" check compares message ids as well, so a reused
  rowid is not mistaken for the current point.

### Verification

3 Rust unit tests added (restore prefers the id, an id anchor whose rowid was reused is
rejected, legacy snapshots stay rowid-compatible) plus 1 project unit test (checkpoint
prefers the id, missing anchor reports an error). Rust full suite **1306/1306** (+4) ·
frontend vitest **250/250** (unchanged, no frontend changes) · `tsc` / `eslint` /
clippy baseline 57/57 · i18n keys unchanged (2298, union 2300) · all three i18n gates,
21 gate self-tests, `check-docs`, and the production build pass. Tool count unchanged:
`TOOL_SPECS` to **204**; database migration count reaches **87**.

---
## Unreleased — Batch 76: time travel no longer leaves the future branch's view state behind (2026-10-22)

Restoring an earlier snapshot archives every message after the anchor and writes the
ledger back to that moment, but the frontend `restoreToSnapshot` only replaced the
message list and token stats, and wiped the feedback/version maps wholesale. The UI
therefore kept "future branch" state after the restore:

- The "continue task" breakpoint (`unfinishedConv`) and the approved plan
  (`approvedPlan`) still pointed at the archived execution trace; continuing would
  resume from the wrong place;
- The task summary and plan progress card still showed the future branch's result;
- The queued list was not refreshed: queued messages archived with the future branch
  stayed visible as if they would still be sent;
- Wiping the feedback/version maps also dropped existing ratings for messages that
  remained visible.

- After a successful restore, clear `lastTaskSummary` / `plan` / `approvedPlan` /
  `unfinishedConv`;
- Reload feedback and versions per conversation instead of wiping the maps, so
  messages that stayed visible keep their badges;
- Refresh the queued list from the database: archived queued messages disappear from
  the queue bar immediately.

### Verification

1 vitest added: messages/cursor replaced, four future-branch fields cleared, tokens
cleared, feedback/versions/queue/snapshots reloaded, and the ledger written back.
Frontend vitest **250/250** (+1) · Rust full suite **1302/1302** (unchanged, no
backend changes) · `tsc` / `eslint` / clippy baseline 57/57 · i18n keys unchanged
(2298, union 2300) · all three i18n gates, 21 gate self-tests, `check-docs`, and the
production build pass. Tool and migration counts are unchanged: `TOOL_SPECS` to **204**;
database migration count reaches **86**.

---
## Unreleased — Batch 75: a rejected history edit no longer "pretends to rerun" (2026-10-22)

Editing a historical user message means deleting that message and everything after
it, then sending the edited content again. A task can start while the edit dialog is
open (or be waiting in the project queue, or between two turns). The backend gate
added in batch 73 then rejects the deletion, but the frontend catch said "delete
failed, still rerun with the new content": the old history was never truncated, yet
the new content was appended as an ordinary message — one edit became two prompts and
the model context still carried the old branch. A deletion failure for any other
reason produced the same "thought it replaced, actually appended" outcome.

- After a failed deletion, re-read the message list to confirm whether the target is
  still there: if it is (deletion rejected/failed), abort the rerun, restore the
  edited content to the composer, and show the failure reason; only continue when the
  target is confirmed gone (concurrent deletion / already absent);
- If the message list cannot be read, treat the target as still present (fail closed):
  make the user retry rather than append a duplicate branch;
- The decision is `canRerunAfterDeleteFailure`, with unit tests for all three cases
  (still present / gone / unreadable).

### Verification

3 vitest cases added (abort while present, continue when gone, abort when unreadable).
Rust full suite **1302/1302** (unchanged, no backend changes) · frontend vitest
**249/249** (+3) · `tsc` / `eslint` / clippy baseline 57/57 · i18n keys unchanged
(2298, union 2300) · all three i18n gates, 21 gate self-tests, `check-docs`, and the
production build pass. Tool and migration counts are unchanged: `TOOL_SPECS` to **204**; database migration count reaches **86**.

---
## Unreleased — Batch 74: time-travel restore passes the task gate and no longer lands on ghost anchors (2026-10-22)

Scenario: restoring a snapshot (time travel) only checked `ChatLock`, which is held
only once a task actually starts executing. While a registered task waited in the
project queue (up to 10 minutes) or between two turns, a restore could slip through
and rewrite the visible history of a task that was running or about to run. The
existing snapshot test also never called the restore implementation — it hand-wrote
the same UPDATE statements, so a broken restore would still stay green. Older
versions did not clean snapshots when deleting messages, so a snapshot whose anchor
message was gone could still be restored onto a rowid that no longer existed. The
returned `went_back` was computed as `restored > 0`, contradicting its own "true =
moved earlier" comment.

- `restore_snapshot` now delegates to a testable `restore_snapshot_conn`; the command
  only performs the gate and locking;
- The gate combines `TaskRegistry.is_running` with `ChatLock`, covering queue waits and
  between-turn gaps;
- Before restoring, the anchor message must still exist: a stale snapshot returns
  "the snapshot anchor message was deleted; this history point cannot be restored"
  without touching visibility or the ledger;
- `went_back` is now `archived > 0` (archiving visible messages after the anchor is
  what moving back means).

### Verification

The existing snapshot test now calls the real `restore_snapshot_conn` (bidirectional
visibility, archived/restored/went_back, ledger written and cleared); 3 unit tests
were added: another conversation's snapshot is rejected, a deleted anchor is rejected
with zero writes, and the runtime gate catches both the project lock and the registry
while allowing other conversations. Rust full suite **1302/1302** (+3) · frontend
vitest **246/246** (unchanged, no frontend changes in this batch) · `tsc` / `eslint` /
clippy baseline 57/57 · i18n keys unchanged (2298, union 2300) · all three i18n gates,
21 gate self-tests, `check-docs`, and the production build pass. Tool and migration
counts are unchanged: `TOOL_SPECS` to **204**; database migration count reaches **86**.

---
## Unreleased — Batch 73: a running task also blocks message deletion over direct IPC (2026-10-22)

Scenario: while streaming, the frontend forbids editing/deleting historical messages
(editing history first deletes the messages after it and re-runs), but that constraint
lived only in the frontend. `delete_message` took a message id, looked up its
conversation and cascaded the delete without ever checking whether that conversation
had a running task. Through direct IPC, automation, or a window where streaming state
was out of sync, the delete could succeed while the main loop kept appending messages
from its own context — the timeline then showed a reply to a prompt that no longer
existed, and task-final writes could overwrite each other with the delete. The frontend
also could not stop the race where a task starts while the confirmation dialog is open.

- `TaskRegistry` gains `is_running(conversation_id)`: true while the registry holds
  that conversation; a poisoned lock is treated conservatively as running (make the
  user retry rather than rewrite history concurrently);
- `delete_message` now takes the task registry: it resolves the owning conversation
  from the message id and returns "the conversation task is running; stop it before
  deleting messages" while a task is registered;
- The gate sits at the top of `delete_message_checked`, before any feedback/version/
  message/snapshot cleanup; editing and removing queued messages use separate
  commands and keep working during a run.

### Verification

Added 1 registry unit test (registration/unregistration tracking, only the target
conversation is blocked) and 2 delete tests (a running task rejects the delete with
messages, feedback, versions and snapshots untouched; an idle conversation deletes
normally). Rust full suite **1299/1299** (+3) · frontend vitest **246/246** (unchanged,
no frontend changes in this batch) · `tsc` / `eslint` / clippy baseline 57/57 · i18n
keys unchanged (2298, union 2300) · all three i18n gates, 21 gate self-tests,
`check-docs`, and the production build pass. Tool and migration counts are unchanged:
`TOOL_SPECS` to **204**; database migration count reaches **86**.

---
## Unreleased — Batch 72: deleting a message cleans its feedback, versions and dead snapshots (2026-10-22)

Scenario: deleting a historical message cascades to every visible message after it, but
only the `messages` table was touched. Likes/dislikes (`message_feedback`) and the old
replies kept for regeneration (`message_versions`) have no foreign-key cascade, so they
kept pointing at messages that no longer existed. Time-travel snapshots anchor on
`msg_rowid`; once the anchor message was deleted, the snapshot stayed on the timeline
and restoring it could only land on a ghost position. The frontend also replaced only
the message list: feedback/versions of deleted messages, token stats, the last task
summary, plan, approved plan and the "continue task" checkpoint stayed visible, while
the snapshot list and queued bar were not refreshed — users could see data from a
deleted task or even be invited to continue a task that no longer existed. The function
comment claimed "transactional" but the statements were not wrapped in a transaction.

- `delete_message` now delegates to a testable `delete_message_conn` that, inside one
  transaction, removes feedback and user-message versions in the deletion range, deletes
  the messages, then removes snapshots whose anchor rowid no longer exists; any failure
  rolls everything back;
- Frontend `removeMessage` now trusts the reloaded message list: it filters feedback and
  version maps to surviving message ids, clears the last task summary, plan, approved
  plan and continuation checkpoint, and reloads token/cost stats, snapshots and the
  queued list.

### Verification

Added 2 Rust unit tests: deleting a middle message clears only the target range, keeps
the hidden archive branch and other conversations, and removes feedback/versions/dead
snapshots; a trigger-blocked delete rolls the side-table cleanup back. Added 1 vitest:
after deleting a message, feedback/versions only keep surviving messages, task summary/
plan/continuation are cleared, and token, snapshot and queued data are reloaded. Rust
full suite **1296/1296** (+2) · frontend vitest **246/246** (+1) · `tsc` / `eslint` /
clippy baseline 57/57 · i18n keys unchanged (2298, union 2300) · all three i18n gates,
21 gate self-tests, `check-docs`, and the production build pass. Tool and migration
counts are unchanged: `TOOL_SPECS` to **204**; database migration count reaches **86**.

---
## Unreleased — Batch 71: deleting the current conversation no longer leaves its view behind (2026-10-22)

Scenario: after deleting the current conversation from the sidebar — especially the
last one — the chat pane cleared only messages and streaming content. The queued
bar, pending tool approvals, pending plan, task ledger, terminal entries, build
logs, last-task summary, snapshots, feedback/version/token stats and diagnostic
cards stayed on screen, and deleting the last conversation left a title for a
conversation that no longer existed. When another window or a LAN client deleted a
conversation, the `conversation-deleted` event reset only a subset of those fields,
inconsistent with the full reset on project deletion. The delete action also read
the current conversation to determine the project only after `await`; if the event
listener cleared the selection first, the action returned early and the sidebar kept
showing the deleted conversation.

- Added `emptyConversationView()`: one complete empty state for the current
  conversation view (messages, paging cursors, streaming content, tool/terminal/
  build/sub-agent records, plan and approvals, todos, ask card, diagnostic cards,
  queued messages, unfinished task, task summary, snapshots, feedback/version/token
  stats);
- Added `releaseConversationRuntime()`: releases the module-level per-conversation
  caches (stream deltas, perf trace, optimistic message, tool-call dedupe sets and
  terminal-run tombstones);
- `conversation-deleted`, `project-deleted`, `conversations-cleared` and the
  `deleteConversation` action now share both helpers; the latter two also clean
  `pendingConfirmations` / `genStatus` / `taskLedgers`, which were previously missed;
- The delete action snapshots the target conversation, its project and the
  "is current" flag before calling the backend, so it can still refresh the sidebar
  if the event arrives first; when the listener has already opened the next
  conversation, the action only updates the list instead of clearing the view again;
  if the list refresh fails after a successful delete, the conversation is removed
  locally so it no longer lingers in the sidebar;
- The second-click confirmation for deleting a conversation now states that running
  tasks stop first, matching project deletion.

### Verification

Added 3 frontend vitest cases: `conversation-deleted` zeroes every view field after
the current (last) conversation is deleted; `deleteConversation` resets the view and
clears the per-conversation streaming bucket, pending confirmations, generation
status and ledger after deleting the last conversation; and a failed list refresh
still resets the view and removes the conversation locally. Rust full suite
**1294/1294** (unchanged; no backend changes) · frontend vitest **245/245** (+3) ·
`tsc` / `eslint` / clippy baseline 57/57 · i18n keys unchanged (2298, union 2300) ·
all three i18n gates, 21 gate self-tests, `check-docs`, and the production build
pass. Tool and migration counts are unchanged: `TOOL_SPECS` to **204**; database
migration count reaches **86**.

---
## Unreleased — Batch 70: deleting a project now stops its tasks and cleans conversation data (2026-10-22)

Scenario: deleting a project relied on foreign-key cascades to remove conversations
and messages, but running tasks were never told. They kept calling the model and
writing `session_events` / `task_runs`, creating fresh orphan rows; the
project-level conversation lock stayed held; tools could keep writing files after
the directory had been moved to the Recycle Bin. Six conversation side tables
without cascades (feedback, versions, task traces, events, todos, reminders) were
left behind, and due reminders could still fire for deleted conversations. The
project-scoped approval whitelist has no foreign key, so it kept an "always allow"
grant for a project that no longer existed, and generated media waited for the next
startup sweep. Other windows and LAN clients received no event. When deletion
failed (for example, the directory was locked and could not be moved), the
frontend only logged to the console, so the second confirmation click appeared to
do nothing.

- Before deleting, read all conversation ids and stop each runtime via the shared
  `stop_conversation_runtime` (stop flag, unanswered questions, durable approval
  revocation, task abort, project conversation lock, budgets/guards/session
  context), then move the directory and delete rows;
- Added `purge_project_content`: clear the six side tables for known
  conversations, delete the project-scoped approval whitelist, delete the project
  row (cascading conversations/messages/tool traces/run events/project memories),
  then sweep orphans to catch conversations created between reading ids and
  deleting rows;
- Reclaim generated media (images/video/audio) per conversation on a background
  thread; failures are logged as `generated_media_cleanup_failed`;
- Emit `project-deleted` (project id plus all conversation ids) and
  `projects-changed` after deletion; add `project-deleted` to the LAN event
  whitelist;
- The frontend chat slice listens for `project-deleted` and clears streaming
  buckets, pending confirmations, generation status, task ledgers and tool-call
  sets in one pass; if the current conversation was deleted it resets the whole
  conversation view;
- `refreshProjects` now clears the current project reference, file tree, branches
  and directory cache when the project has disappeared; `projects-changed`
  reloads project data only after the list refresh completes, so it no longer
  queries a deleted project;
- Deletion failures now raise an error notification instead of being silent, and
  the confirmation copy states that running tasks will be stopped first.

### Verification

Added 1 Rust unit test: project deletion clears the six side tables and the
project-scoped approval whitelist, cascades conversations/messages, prunes side
rows of a conversation created inside the deletion window, and leaves another
project's data untouched. Added 2 frontend vitest cases: `refreshProjects` clears
the reference and caches once the current project is gone, and `project-deleted`
resets the runtime state of all affected conversations in one pass. Rust full
suite **1294/1294** (+1) · frontend vitest **242/242** (+2) · `tsc` / `eslint` /
clippy baseline 57/57 · i18n +1 key (2298, union 2300) · all three i18n gates,
21 gate self-tests, `check-docs`, and the production build pass. Tool counts are
unchanged: `TOOL_SPECS` to **204**; database migration count reaches **86**.

---
## Unreleased — Batch 69: clearing content now stops running tasks first (2026-10-22)

Scenario: "Clear all content data" in Settings previously only deleted database rows
and generated media. Running conversation tasks were never told their conversation
was gone: they kept calling the model and consuming tokens; later writes to
`session_events` / `task_runs` became fresh orphan rows; writes to foreign-keyed
tables failed outright; the project-level conversation lock stayed held and blocked
new conversations in the same project; and the frontend received no event, so the
sidebar still showed deleted conversations and streaming state.

- Extracted `stop_conversation_runtime` from conversation deletion (stop flag,
  closing unanswered questions, durable approval revocation, aborting the tokio
  task, releasing the project lock, clearing budget/guard/session context); both
  deletion and clear-all use it;
- Failure semantics changed: even when approval revocation fails, local stop and
  runtime cleanup always finish. Conversation deletion still requires a successful
  revocation before deleting rows; clear-all logs
  `conversation_runtime_stop_failed` and continues, because `tool_runs` /
  `run_events` are wiped right after and the active call has already been aborted;
- Clear-all reads every conversation id first and stops each runtime; if reading ids
  fails, nothing is cleared (there would be no ids to stop);
- `TaskRegistry` gained abort-with-reason so logs distinguish
  `conversation_deleted` from `content_cleared`;
- On success a `conversations-cleared` event resets the frontend conversation list,
  streaming buckets, pending confirmations, generation status, ledgers, and current
  conversation;
- The confirmation dialog now states that running tasks will be stopped, so the
  interruption is not a hidden side effect.

### Verification

Three new Rust unit tests and one frontend interaction test: successful revocation
writes a revocation record; failed revocation still clears the stop flag and project
lock; abort-with-reason unregisters and cancels the task; clear-all is not called
before confirmation and is called afterwards with the deleted counts shown. Rust
**1293/1293** (+3) · frontend vitest **240/240** (+1) · `tsc` / `eslint` / clippy
baseline 57/57 · i18n keys unchanged (**2297**, union **2299**) · three i18n gates,
all 21 gate self-tests, `check-docs` and the production build pass. Tool count
unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 68: deleting a conversation or clearing content no longer leaves side data behind (2026-10-22)

Scenario one: when a conversation was deleted, `conversation_todos` and
`message_reminders` had no foreign-key cascade, so their rows remained forever.
A due reminder was still injected into the conversation and raised a desktop
"Reminder" notification even though the conversation no longer existed — a phantom
notification the user could not locate, and `every` reminders kept firing.

Scenario two: "Clear all content" missed `session_events` (the complete event
transcript of user messages, assistant replies, and tool calls), conversation
todos, and reminders. Users believed content was gone while the transcript was
still in the database.

- Added a `CONVERSATION_SIDE_TABLES` registry (feedback / versions / task runs /
  events / todos / reminders) shared by single-conversation deletion, clear-all,
  and the startup sweep, so future tables cannot be missed again;
- Conversation deletion explicitly clears every side table instead of relying on a
  cascade being declared;
- Clear-all empties every table in the registry inside the same transaction;
- Startup maintenance removes historical orphan rows left by older versions,
  logs `orphan_conversation_data_pruned`, and reclaims space;
- Audit events (`agent_audit_events`) and skill usage (`skill_usage`) are kept:
  they contain run metadata and statistics only, not conversation content, and are
  separate audit/analytics scopes.

### Verification

Two new Rust unit tests plus one extended test: single-conversation deletion only
clears the target, the startup sweep removes orphans only, and clear-all covers the
events/todos/reminders tables that were previously missed. Rust **1290/1290** (+2) ·
frontend vitest **239/239** unchanged · `tsc` / `eslint` / clippy baseline 57/57 ·
i18n keys unchanged (**2297**, union **2299**) · three i18n gates, all 21 gate
self-tests, `check-docs` and the production build pass. Tool count unchanged:
`TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 67: deleting a conversation or clearing content reclaims generated media (2026-10-22)

Scenario: generated images, videos, and audio live under
`{app_data}/generated/<conversation_id>/<uuid>/`. Deleting a conversation only
removed database rows, and clearing all content never touched the files, so media
kept occupying disk indefinitely; crashes, locked files, or failed deletes also
left unowned directories behind.

- New `utils/generated_media`: a conversation id must be a single safe path segment
  (empty, `.`, `..`, path separators, drive colons, and NUL are rejected), so
  cleanup can never escape the generated root;
- Deleting a conversation reclaims its directory only when the conversation row was
  actually deleted, and runs on a background thread so large video files do not
  block the command;
- "Clear all content" removes the whole generated root after the database clear and
  VACUUM;
- A startup orphan sweep removes directories that belong to no live conversation;
  if listing conversations fails, the sweep is skipped entirely (an empty set would
  delete everything), and stray files or non-UTF-8 directory names are left alone;
- Cleanup failures are logged as `generated_media_cleanup_failed` and never block
  conversation deletion; the next startup sweep retries.

### Verification

Five new Rust unit tests: unsafe conversation ids are rejected without touching the
generated root; per-conversation removal only affects the target; clear-all removes
the whole root; the orphan sweep keeps live conversations, removes orphans, and
leaves stray files; a missing root is a no-op. Rust **1288/1288** (+5) · frontend
vitest **239/239** unchanged · `tsc` / `eslint` / clippy baseline 57/57 · i18n keys
unchanged (**2297**, union **2299**) · three i18n gates, all 21 gate self-tests,
`check-docs` and the production build pass. Tool count unchanged: `TOOL_SPECS` to **204**,
migration count reaches **86**.

---
## Unreleased — Batch 66: failed media tasks always release the composer (2026-10-22)

Scenario: media generation emits `gen-start` and only then resolves the generation
model, reads the provider, and builds the HTTP client. Those early failures returned
straight through `?` without ever emitting `gen-error`; a watchdog abort or a panic
left the task body with no chance to emit it either. The frontend clears `genStatus`
only on `gen-done` / `gen-error`, so the "generating" state stayed forever and both
Send and Generate stayed disabled until an app restart.

- Terminal events are now centralized in the `generate_media` command: ordinary
  failures, watchdog aborts, and panics all emit exactly one `gen-error` (success
  still emits `gen-done` from the body);
- Early errors such as no generation model, no active provider, or client setup
  failure now release the status entry too;
- Abort and panic use distinct messages; a `gen_task_join_error` log records the
  cancelled flag and JoinError details for diagnosis;
- The terminal event is emitted after the task is unregistered, so an immediate
  retry after the error does not hit "a task is already running in this conversation";
- An intentional stop still reports "Generation stopped", never a system failure.

### Verification

Two new Rust unit tests: abort and panic map to different user-facing messages; an
aborted JoinHandle is indeed classified by `is_cancelled()` (rather than reported as
a panic). Rust **1283/1283** (+2) · frontend vitest **239/239** unchanged ·
`tsc` / `eslint` / clippy baseline green · i18n keys unchanged (**2297**, union
**2299**) · i18n gates, all 21 gate self-tests, `check-docs` and the production build
pass. Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 65: media generation can actually be stopped (2026-10-22)

Scenario: while an image/video/audio generation is running, the user clicks "Stop" on
the status bar. Four problems existed: the button set the chat-stream `stopRequested`
flag, but media has no streaming bucket, so the flag never reset and later sends were
silently blocked until an app restart; image/audio requests had no stop check at all
and ran to completion; video could take up to 10 seconds (one poll interval) to notice;
and an intentional stop was reported as "Generation failed" with a red toast.

- `generate_media_body` now races the generation future against a 200 ms stop poll;
  a stop drops the request (the reqwest connection is cancelled with the future), so
  image and audio can also be stopped mid-flight;
- Stopped and failed attempts both remove that request's private output directory, so
  partial files do not accumulate;
- An intentional stop returns "Generation stopped"; the frontend restores the prompt
  and generation mode without showing a failure notification;
- `stopRequested` resets once the media status (`currentGen`) clears, so the composer
  no longer locks up.

### Verification

Two new Rust unit tests (tokio timer driven): a pre-set stop flag cancels a generation
future that never completes; without a stop request the successful result passes
through unchanged. Rust **1281/1281** (+2) · frontend vitest **239/239** unchanged ·
`tsc` / `eslint` / clippy baseline green · i18n keys unchanged (**2297**, union
**2299**) · i18n gates, all 21 gate self-tests, `check-docs` and the production build
pass. Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 64: new tasks no longer inherit the previous stop flag (2026-10-22)

Scenario one: the user stops a conversation and immediately generates a video. The
media task reads the previous task's stop flag at its first stop check and fails with
"task stopped"; the flag stays set, so every later video generation is cancelled at
once. Scenario two: after a stop, a new chat task cleared the old flag only late in
startup, so a Stop arriving between successful registration and that clear was
discarded — the task kept running even though the user clicked Stop.

- `ChatCancel` gains `clear_stale_stop`, whose meaning is "a new task registered
  successfully"; it clears only this conversation;
- `stream_chat` and `generate_media` clear the stale flag only after
  `TaskRegistry::register` succeeds and before the task body is released. A failed
  registration means the old task is still running, so nothing is cleared and a Stop
  aimed at that task is not swallowed;
- The late clear inside `stream_chat_inner` is removed: Stop requests that arrive
  after release now stay visible to the project-lock wait and the normal safe points;
- A `stale_stop_cleared` log line is written when a leftover flag is actually removed,
  so "Stop leaking into the next task" is easy to audit.

### Verification

Two new Rust unit tests: a new task clears only its own conversation's stale flag,
leaves another conversation's stop request untouched, and reports no false removal
when repeated; a stop requested after registration stays sticky for the whole task
and is cleared only when the next task starts. Rust **1279/1279** (+2) · frontend
vitest **239/239** unchanged · `tsc` / `eslint` / clippy baseline green · i18n keys
unchanged (**2297**, union **2299**) · i18n gates, all 21 gate self-tests,
`check-docs` and the production build pass. Tool count unchanged:
`TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 63: take over other conversations' queued messages when the project lock frees (2026-10-22)

Scenario: conversation A is running and the user switches to conversation B to send a
message. B's message is queued, but the project lock is held by A, so auto-start does not
jump ahead by design. Previously nothing picked B's message up when A finished cleanly;
it waited until the user returned to B and clicked it again. Now A's teardown:

- Checks whether this conversation has no queued message, then finds the earliest queued
  message in another conversation of the same project (ordered by `(created_at, rowid)`)
  and reuses the normal auto-start path (including B's last-sent conversation options);
- Only takes over when this conversation's queue is also empty and the task finished
  cleanly; failed or stopped teardowns do not scan, preserving "queued messages wait
  after a failure" and "no auto-start after Stop";
- Takes over one conversation at a time when several have queues, so they do not race
  for the same project lock.

### Verification

Two new Rust unit tests: cross-conversation handoff picks the earliest while excluding
the finished conversation, assistant rows, hidden rows, and other projects; only the
finished conversation's own queue returns None (it cannot restart itself twice).
Rust **1277/1277** (+2) · frontend vitest **239/239** unchanged · `tsc` / `eslint` /
clippy baseline green · i18n keys unchanged (**2297**, union **2299**) · i18n gates,
all 21 gate self-tests, `check-docs` and the production build pass. Tool count unchanged:
`TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 62: auto-start reuses the last conversation options (2026-10-22)

The idle auto-start added in the previous batch has no frontend request to carry
parameters, so it started with defaults: the selected model, temperature, tool approval
policy, plan mode, and "submit together" were all lost. A conversation set to confirm
every tool would auto-approve L1 tools under `auto`, and queued messages with
"submit together" enabled would run one by one. Task starts now remember the explicit
options per conversation:

- When `stream_chat` receives options it writes them to `settings` (key
  `chat_options:<conversation id>`); a failed write never blocks the send;
- Idle auto-start reads that snapshot back and passes it to the task, matching the
  semantics of a normal in-task continuation;
- The one-shot `resume_run_id` is cleared on both save and load, so auto-start cannot
  resume an old Run by accident;
- A missing or corrupt snapshot silently falls back to backend defaults without
  affecting queue startup.

### Verification

Two new Rust unit tests: model/temperature/approval/plan/submit-together/reply-language
survive a round trip with only the latest snapshot kept per conversation, and the
one-shot resume id is stripped; missing, corrupt, and other-conversation snapshots all
return None. Rust **1275/1275** (+2) · frontend vitest **239/239** unchanged · `tsc` /
`eslint` / clippy baseline green · i18n keys unchanged (**2297**, union **2299**) ·
i18n gates, all 21 gate self-tests, `check-docs` and the production build pass. Tool
count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 61: messages queued after a task ends start on their own (2026-10-22)

There was a narrow window at task end: the backend had already checked that the queue
was empty but had not unregistered yet, while the frontend still believed it was
streaming and wrote the new user message into the queue. Nothing picked that message up
afterwards, so it sat in the queue bar until the user clicked it again. Two fallbacks
now close the window:

- After unregistering, the task checks the head of the queue once more. If it finished
  successfully and the user did not press Stop, it starts that message itself through
  the normal claim/start path; the `chat-queued-consumed` claim event now carries the
  full message, so the frontend can move a bubble that only existed in the queue bar
  into the timeline;
- The enqueue commands check as well: a single `queue_message` starts the queue head
  when the conversation is idle, and `queue_messages` gains `autoStart`, passed by a
  caller that believes a task is still running, so a batch written after the task ended
  is started by the backend; the idle batch path still starts its first line from the
  frontend in input order.

Auto-start never jumps ahead when another conversation holds the project lock (the
message stays visible in the queue bar instead of disappearing into a silent wait), and
a queue left by an explicit Stop is preserved as before. Auto-start reuses the same
claim path, so nothing is inserted twice; both the normal continuation and auto-start
claim events now include the full message row, so the frontend has a single landing
path.

### Verification

Two new Rust unit tests: the queue-head probe returns the full row without consuming it
(hidden and assistant rows excluded, insertion order kept within one second), and
auto-start requires both an idle project and no Stop request. Two new frontend unit
tests: an auto-started message already in the timeline only clears its queued flag, and
one that exists only in the queue bar is appended to the timeline and removed from the
bar. Rust **1273/1273** (+2) · frontend vitest **239/239** (+2) · `tsc` / `eslint` /
clippy baseline green · i18n keys unchanged (**2297**, union **2299**) · i18n gates,
all 21 gate self-tests, `check-docs` and the production build pass. Tool count
unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 60: batch send enqueues and starts in input order (2026-10-22)

When idle, batch send used to enqueue lines 2..N first and then send line 1:

- line 1's user message was only inserted after the project lock was acquired, so its
  rowid landed after every queued follow-up;
- after the task finished, the queue resumed by rowid, leaving the first instruction
  permanently below the rest of the batch (not transient — reopening showed the same);
- per-line inserts had no transaction boundary; a mid-way failure left half a batch,
  and retrying from the dialog could shuffle the order.

Batch send now uses `queue_messages`: the whole batch is inserted in input order in one
transaction, and any failure rolls the batch back. When idle, the backend then *claims*
the first row (`queued_message_id` on `stream_chat`); the claim is a conditional
`queued 1 → 0`, so the message is not inserted twice and its rowid always precedes the
follow-ups. A successful claim emits `chat-queued-consumed` to clear the queued badge;
if task registration fails, the row is restored to queued so the queue bar can retry —
it never becomes an orphan question without an answer.

Existing queue reads, model history and pagination keep the `(created_at, rowid)`
ordering from Batch 59. The messages schema is unchanged; no migration is needed.

### Verification

Three new Rust unit tests: a claim uses the stored body and cannot be repeated; rows
from another conversation, hidden rows and missing ids are rejected; a same-timestamp
batch inserts in input order and keeps that order after the first row is claimed.
Rust **1271/1271** (+3) · frontend vitest **237/237** unaffected · `tsc` / `eslint` /
clippy baseline green · i18n keys unchanged (**2297**, union **2299**) · i18n gates,
`check-docs` and the production build pass. Tool count unchanged: `TOOL_SPECS` to
**204**, migration count reaches **86**.

---

`messages.created_at` only has second precision, yet the timeline, pagination and model
context previously sorted by `created_at` alone:

- `list_messages_page` used the message id as the tie-breaker, and ids are UUIDs — a
  question and its reply created in the same second could be reversed by lexicographic
  order, putting the answer above the question after reopening a conversation;
- model history and the compaction window had no deterministic order for same-second
  rows, so a tool result could appear before its call;
- memory replay and exports used the same ordering, so external views could disagree
  with the UI.

Every chronological message read now uses `(created_at, rowid)`: rowid is the real
insertion order, which branch anchors and snapshot restore already rely on. The page
cursor also moves from `(created_at, id)` to `(created_at, rowid)`, so paging across a
same-second boundary neither duplicates nor loses rows. Covered reads: message list and
pagination, model history, compaction window and character accounting, memorize replay,
memory-summary draft, reply version list, session-share export and reproduction bundle.

This is a read-order fix only: the messages schema, queue semantics and caches are
unchanged, no migration is needed, and existing conversations display in the correct
order as soon as they are reopened.

### Verification

Three new Rust unit tests: the list keeps rowid order even when UUIDs sort opposite to
insertion; a same-second cursor pages with no duplicates or losses; model history keeps
insertion order and excludes hidden/queued rows. Rust **1268/1268** (+3) · frontend
vitest **237/237** unaffected · `tsc` / `eslint` / clippy baseline green · i18n keys
unchanged (**2297**, union **2299**) · i18n gates, `check-docs` and the production build
pass. Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 58: stopping during the project-lock wait still keeps the user event (2026-10-22)

Auto-continue and "Submit together" tasks wait **before acquiring the project lock**
(one task per project, up to 10 minutes). If the user stops during that window,
`stream_chat_inner`'s cancellation branch only looked up the latest user message id:

- queued messages (`persist_user = false`) are not re-inserted and got no
  `UserMessage` event;
- `chat-run-started` had already been emitted for this task, so the timeline and
  trajectory were missing that round's user input, and reflexion's round boundary
  lost a segment as well;
- the continuation message is a normal `queued = 0` history row, so the UI and the
  event log disagreed.

The cancellation branch now calls `record_queued_user_on_cancel`: it first appends a
`UserMessage` event (`queued: true`, owned by this task's trace), then returns the
latest visible `queued = 0` user message id. If either the event write or the lookup
fails, the stop itself still proceeds — stopping wins; only the id alignment is lost.

### Verification

Two new Rust unit tests: the event is appended and hidden rows are skipped when
picking the latest visible user; with no alignable row the event is still written and
the id is `None`. Rust **1265/1265** (+2) · frontend vitest **237/237** unaffected ·
`tsc` / `eslint` / clippy baseline green · i18n keys **2297** (no changes, union
**2299**) · i18n gates, `check-docs` and the production build pass. Tool count
unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 57: the timeline follows queued-message consumption immediately (2026-10-22)

Queued messages can be consumed in two ways. Both took effect in the database, but the
frontend was **notified of neither**:

- single auto-continue: `take_next_queued` clears `queued`, yet the bubble kept
  showing "queued" until the whole conversation was reloaded;
- "Submit together" batch merge: the backend deleted the originals and wrote the
  merged row in one transaction, while the frontend kept stale queued bubbles (whose
  rows no longer existed) and never showed the instructions that were actually
  submitted — they appeared only after switching away and back.

Users could conclude the message was never submitted, send it twice, or delete a
running task by mistake.

Two events now report consumption:

- `chat-queued-consumed` (single): the frontend clears `queued` on that bubble and
  removes it from the queue bar;
- `chat-queued-batch-merged` (batch): carries the merged row id, content, references,
  the deleted original ids and the stored timestamp; the frontend drops the originals
  by id and inserts the merged message, with no conversation reload.

The batch payload comes from the row actually committed in the transaction (unit
tests assert id / content / timestamp match), so the UI cannot build a shadow message
that differs from the database.

### Verification

Rust **1263/1263** unaffected (batch-consumption tests now also assert the returned
merged id, original-id order and timestamp match the stored row) · frontend vitest
**237/237** unaffected · `tsc` / `eslint` / clippy baseline green · i18n keys **2297**
(no changes, union **2299**) · i18n gates, `check-docs` and the production build pass.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 56: batch-merge rows and original deletions share one transaction (2026-10-22)

"Submit together" used to happen in two steps: `take_all_queued_conn` deleted all
queued originals first and returned the merged text, while the real merged message
row was only written once the next task started and `stream_chat_inner` reached its
user-message insert. In between sat project-lock queueing (up to 10 minutes),
recovery validation, scheduler registration and provider resolution — any failure or
a user cancellation left the merged text only in memory, with **originals deleted and
no replacement row**, permanently losing every queued instruction.

The merged row and the original deletions now happen in **one transaction**: merged
text, deduplicated references, `conversations.updated_at` and the deletions either
commit together or roll back together. On failure the queued messages stay exactly as
they were and the user can retry; after success the inner task no longer inserts a
duplicate, while the references still feed the focused-module hint (history injection
reads `references_json` from the merged row). The stop path also returns the already
persisted user message id so the UI does not need a reload to reconcile.

### Verification

Rust **1263/1263** (+2: merged row content/references/timestamp match the original
deletion; a failing delete rolls back the merged row and leaves originals queued) ·
frontend vitest **237/237** unaffected · `tsc` / `eslint` / clippy baseline green ·
i18n keys **2297** (no changes, union **2299**) · i18n gates, `check-docs` and the
production build pass. Tool count unchanged:
`TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 55: failures when stopping the running tool are no longer silent (2026-10-22)

"Stop the running tool" is a two-step backend action: first the conversation's stop
generation is bumped (the running tool observes it and kills its child process), then
the approval credential for that call is persistently revoked. The frontend
`stopCurrentTool` treated the whole thing as ignorable:

```ts
try { await stopToolApi(conv.id) } catch { /* ignored */ }
```

Whenever the backend returned an error — typically the revocation hitting a busy or
broken database lock — the click produced no feedback at all: the local interrupt may
have been sent, but whether the credential was revoked and whether a retry is needed
was invisible. All three entries (tool-row cancel, composer button, overflow menu)
shared this silent function.

Errors now propagate, and the three entries share `handleStopTool`: on failure it
posts a `warn` notification `home.stopToolFailed` ("Stopping the running tool didn't
finish cleaning up") with the backend message. The success path is unchanged; tool
state still updates from streaming events.

### Verification

Rust **1261/1261** unaffected · frontend vitest **237/237** unaffected ·
`tsc` / `eslint` / clippy baseline green · i18n keys **2297** (+1, union **2299**) ·
i18n gates, `check-docs` and the production build pass. Tool count unchanged:
`TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 54: queued messages no longer vanish from the session event log (2026-10-22)

`session_events` is the append-only source shared by the timeline panel,
`replay_trace`, eval trajectory export and reflexion. Queued messages have two
consumption paths that **do not insert a new message row**:

- auto-continue after a successful task: the original row is already stored, only
  `queued` is cleared before it becomes the next round's input;
- safety-point merge mid-task: injected as an additional user instruction without
  being stored again.

Neither path wrote a `UserMessage` event, so:

- the timeline and trajectory showed assistant/tool events for the continuation
  round with no corresponding user input;
- reflexion splits rounds at the last `UserMessage`, so failures from the
  continuation round were counted into the previous round, potentially producing
  duplicated or inflated reflection cards.

Both paths now append a `UserMessage` event: the payload carries `queued: true`,
and safety-point merges add `merged: true`. Auto-continue uses the **new task's own
trace_id**, so that round's user input and tool calls share one trace.

### Verification

Rust **1261/1261** (+2: continuation event type/flags/trace ownership, merge flag
and projection order) · frontend vitest **237/237** unaffected · `tsc` / `eslint` /
clippy baseline green · i18n keys **2296** (no changes, union **2298**) · i18n
gates, `check-docs` and the production build pass. Tool count unchanged:
`TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 53: "Insert now" only affects the task it was requested for (2026-10-22)

"Insert now" on the queued-message bar used to store a per-conversation boolean.
When the click arrived after the task's last safety point (or just as the task
ended), the flag was never consumed and **leaked into the conversation's next task**:

- the new task's first round force-merged the whole old queue, including ordinary
  messages that should have been processed one by one;
- the UI announced "inserted into the current task as requested" even though the
  user never clicked it for that task;
- multiple messages were poured into one round instead of being continued in order.

The flag now carries the **task generation** that requested it:

- `insert_queued_now` rejects the request when no task is running and stores nothing;
- the safety point validates the generation and drops stale requests with an
  `insert_queued_stale` log entry;
- task teardown clears only its own generation, never a request from a newer task.

The frontend no longer fails silently: clicking just as the task ends now shows
`home.insertQueuedFailed` and states that the queued messages are still kept.

### Verification

Rust **1259/1259** (+4: one-shot consumption, stale request, no-running-task
rejection, generation-scoped cleanup) · frontend vitest **237/237** unaffected ·
`tsc` / `eslint` / clippy baseline green · i18n keys **2296**
(+1, union **2298**) · i18n gates, `check-docs` and the production build pass.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 52: batch submission keeps queued messages' @ references (2026-10-22)

"Submit together" merges all queued messages into one numbered message and then
**deletes the originals**. The merged message was re-inserted with only its body,
so `references_json` disappeared along with the originals:

- @file references on queued messages were no longer injected by `inject_references`;
  the model only saw the path text;
- @conversation references lost their title and latest summary;
- the reference chips under the message bubble disappeared as well.

`take_all_queued` now collects references while reading the bodies (deduplicated
across messages, preserving queue order) and persists them with the merged message.
History assembly then injects file/conversation content as usual.

### Verification

Rust **1255/1255** (+4: reference merge/dedup, single message, corrupt refs,
empty queue) · frontend vitest **237/237** unaffected · `tsc` / `eslint` /
clippy baseline green · i18n keys **2295** (no additions, union **2297**) ·
i18n gates, `check-docs` and the production build pass.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 51: conversation-creation failures no longer swallow actions (2026-10-22)

Batch 48 fixed silent failures after an instruction was sent but not accepted. This
batch covers an earlier link in the same chain: **the conversation was never created**.
`newConversation` calls `create_conversation` / `list_conversations` and rejects on
failure, while several entry points only did `await newConversation()` with no catch:

- Main composer send: the click produced no task and no feedback (the draft was still
  there because it is cleared later, but the user could not tell why);
- Media generation send: same;
- Selection instruction: the selected-text instruction vanished silently;
- Runtime anomaly "Fix now": the card stayed, but nothing was reported;
- Build errors "Auto-fix": the fix request vanished silently.

All now catch the failure and report it: composer and media sends use
`home.sendFailed` (the draft is indeed still in the composer); button-generated
instructions use `home.actionNotSent` (the anomaly card is kept as well).

### Verification

Frontend vitest **237/237** (no new unit tests: error-handling wiring only) with
`tsc` / `eslint` green · Rust **1251/1251** unaffected · i18n keys **2295**
(no additions, union **2297**) · i18n gates, `check-docs` and the production build pass.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 50: the batch dialog's shortcut submits the latest text (2026-10-22)

The batch-task dialog registered its Ctrl+Enter listener with a dependency on
`lines.length`. After editing a line without adding or removing lines, the listener
still held the previous `submit` closure, so the shortcut enqueued the text from
*before* the edit — the dialog looked edited, the queue got the old instruction.

The listener is now registered once on mount and calls the latest `submit` through
`submitRef`. A regression test edits text within the same line count, presses
Ctrl+Enter, and asserts the new text is submitted.

### Verification

Frontend vitest **237/237** (+1 for the shortcut submitting the latest text) with
`tsc` / `eslint` green · Rust **1251/1251** unaffected · i18n keys **2295**
(no additions, union **2297**) · i18n gates, `check-docs` and the production build pass.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 49: batch sending no longer loses lines, and starts immediately when idle (2026-10-22)

The batch-task dialog (one instruction per line, executed serially) had three ways to
look submitted while doing nothing:

- **Submitting in a fresh chat**: the handler returned as soon as
  `currentConversation` was empty, the dialog closed as usual, and every parsed line
  was lost. It now creates the conversation first.
- **Submitting while idle**: all lines were stored with `queued=1`, but no task was
  running, so the backend never resumed them — the user had to click "Run now" on
  the queue bar. The first line now starts a new round immediately; the rest stay
  queued and are consumed serially when the task finishes.
- **Enqueue failures**: only a summary notification appeared and the composer was
  cleared anyway. Failed lines now remain in the dialog (successful lines are already
  queued and are not repeated), and the dialog closes only when everything succeeds.

### Verification

Frontend vitest **236/236** (+2 for batch-dialog parsing and failed-line retention) ·
`tsc` / `eslint` green · Rust **1251/1251** unaffected · i18n keys **2295**
(no additions, union **2297**) · i18n gates, `check-docs` and the production build pass.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 48: button-generated instructions no longer fail silently (2026-10-22)

Continuing the conversation-core polish, this batch audits instructions that are
generated by buttons rather than typed into the composer. They have no composer
content to fall back on, so a rejected send looked like a dead button:

- **Runtime anomaly "Fix now"** dismissed the anomaly card before sending. If the
  invoke failed, both the card and the instruction were gone. The card now closes
  only after delivery succeeds.
- **Build errors "Auto-fix"** cleared the user's draft and @ references, then used
  `void sendUserMessage`. A rejected send lost both the draft and the fix request.
  It no longer touches the composer.
- **Failed tool "Retry"** used `void queueUserMessage(...)` while streaming with no
  catch, so a failed insert became an unhandled rejection with no feedback.
- **Unfinished task "Continue / Recover safely"** also used `void sendUserMessage`;
  a rejected send did nothing visible.

All four now go through `deliverActionText`: try to send; if a task just started
streaming in the same conversation, queue the instruction instead; only when the
queue also fails (or the user switched conversations) report `home.actionNotSent`.

### Verification

Frontend vitest **234/234** (no new unit tests: the change is component wiring, the
pure helpers are unchanged) with `tsc` / `eslint` green · Rust **1251/1251** unaffected ·
i18n keys **2295** (+1, union **2297**) · i18n gates, `check-docs` and the production
build pass. Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

---
## Unreleased — Batch 47: the stop fallback actually starts; failed sends no longer eat input (2026-10-22)

Two more conversation-core paths had their fallback only in a comment:

- **Stop fallback**: `stopGeneration` awaited `stopChatApi` before arming the 60-second
  timer. That fallback exists precisely for "backend hung, the command never returns" —
  the await never finishes, so the timer never starts and the UI spins forever.
  The timer is now armed before the stop command is awaited; the existing `startedAt`
  generation check still keeps an old timer from killing a new task.
- **Failed sends**: `sendUserMessage` returned immediately while the same conversation
  was streaming, after the composer had already been cleared. If the invoke failed
  before the user message was persisted, the input only survived inside an optimistic
  bubble that was about to be replaced. `sendUserMessage` now reports whether it took
  the message: the duplicate guard and a not-persisted failure return false, and callers
  restore the input. After an invoke failure the latest database page decides whether
  the message landed; a persisted message counts as sent so the draft is never restored
  and sent twice, and a failed page fetch is also treated as sent.

### What is restored

- Composer: text, @ references, images and the quoted message; if the user switched
  conversations, the text is written back to that conversation's draft. New typing is
  merged, never overwritten.
- Queue bar "Run now": if a task starts streaming right after the queued row is removed,
  the message is queued again; only a second failure falls back to the composer.
- History edit re-run: the old messages were already deleted for the re-run, so a
  rejected send at least returns the edited content to the user.
- Selection instructions: a rejected send no longer clears the user's draft and reports
  that it should be retried.

Two new strings: `home.sendFailed` / `home.sendRejected`. Tool count unchanged:
`TOOL_SPECS` to **204**, migration count reaches **86**.

### Verification

Frontend vitest **234/234** (+3 for the persisted-user-message check, including image sends) with `tsc` /
`eslint` green · Rust **1251/1251** unaffected · i18n keys **2294** (+2, union **2296**) ·
i18n gates, `check-docs` and the production build pass.

---
## Unreleased — Batch 46: queued messages no longer use the history edit/delete path (2026-10-22)

Batch 45 closed the forced wrap-up gap; this batch fixes the conversation core around
queued messages. Editing and deleting a queued message used to go through the history
path, whose semantics are "cascade delete and re-run from here":

- **Editing a queued message** deleted it and everything after it, then called
  `sendUserMessage` — which returns immediately while the same conversation is streaming.
  The message was gone and the edit did nothing.
- **Deleting a queued message** also cascaded, removing content the user never meant to touch.
- **Editing history during a run** was allowed and could cut the context the running task
  was still using.

### Queued messages: edit in place, delete one row

- `update_message` gains `require_queued`: the backend puts `queued = 1` into the **UPDATE
  WHERE clause** instead of checking first. A queued message can be merged by the Agent
  while the dialog is open; check-then-write would rewrite a sentence the model already
  saw. Zero affected rows returns "already started executing", and the UI says the content
  was not changed.
- Deletion uses `remove_queued_message`, never the cascade.
- The edit dialog switches its hint: queued messages say "saving updates it in place
  without re-running the task".

### History is locked while running

Editing or deleting a non-queued message during streaming now shows "stop the task first"
instead of truncating live context. Queued messages are unaffected. The decision lives in
two pure functions, `planUserMessageEdit` / `planMessageDelete`; the submit path checks
again in case a run started while the dialog was open.

### Failures stay visible

Editing too late, a failed cascade delete, or a failed assistant edit now raises a
notification instead of silently doing nothing or leaving an unhandled rejection.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

### Verification

Rust `update_message_tests` **4/4** · frontend chatUtils **25/25** and edit dialog **3/3** ·
i18n keys **2292** (+6, union **2294**) · i18n gates and `check-docs` green; the full test
suite and clippy baseline are part of the pre-commit gate.

---
## Unreleased — Batch 45: force wrap-up — stop the tool loop when level 3 is ignored (2026-10-21)

Batches 32–34 made stalls visible and actionable, but the guard itself was still **only
text**: level 3 said "choose one of two", and the model could keep exploring until the
budget ran out. This batch adds the missing piece: once the force-wrap-up line is crossed,
the tool loop **actually stops** and the model may only produce one text report.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

### Why "strictly greater", not "stop on arrival"

The level-3 warning is appended to the tool result in the round where
`since_progress = 3 × threshold`; the model only sees it on the next request.
Stopping on arrival would mean the strongest warning never gets a chance to work — it
would turn "warn, then act" back into "act immediately".
⇒ The force condition is `since_progress > stall_force_wrapup_threshold`: the model gets
one full round of action after seeing the highest warning; only a round with no real
progress after that is cut off.

### Wrap-up is not a silent stop

- A **stall-specific** wrap-up prompt names the consecutive count, the stalled context
  bytes and the repeated actions, and asks for text only: confirmed facts / ruled-out
  possibilities / what is still missing / suggested next step.
- Whether or not the model produces a summary, the assistant message always carries a
  visible reason: "N consecutive tool calls without real progress (about X KB entered the
  context during this stall); the guard forced a wrap-up."
- If the summary request fails, a fixed notice is used instead and points to "Change
  approach" or a new instruction — the user is never left with a stop and no explanation.

### Configurable, defaulting after level 3

Settings now expose **Stall Force Wrap-up** (`stall_force_wrapup_threshold`), default
**30** — 3× the default stall threshold of 10: the task passes the 1× / 2× / 3× warnings
and is forced to wrap up only **after more than** 30 stalled calls. Set 0 or -1 to disable;
it also stays inactive when stall detection itself is off.

An absolute N alone is not enough: if the user raises the stall threshold to 30, call 30
is exactly level 3, and a fixed N=30 would wrap up in that same round — **the strongest
warning would never be shown**. ⇒ The effective threshold is `max(N, 3 × stall threshold)`.
With defaults it is still 30; raising the stall threshold moves the wrap-up line with it,
so "warn before acting" holds under every configuration.

Configs saved by older versions lack this field: `AgentLimits` now uses `serde(default)`,
so missing fields fall back to their defaults instead of failing the whole deserialization
and resetting every guard.

### Boundary

- Only the **current task's** tool loop is stopped; produced content and the ledger are
  kept. The task finishes as **not completed** (`exhausted=true`), so acceptance cannot
  count it as done.
- The check runs **after all tools in a round finish**: progress actions (edit / build /
  test) clear the counter and reset the signal first, so a correct action the model just
  made is never cut off.
- No new tools, migrations or frontend events; just one setting and two i18n strings.

### Verification

`task_guard` tests **23/23** (2 new: strictly-greater + disabled when the guard is off /
fires once + resets after progress) · `agent_limits` **5/5** (missing-field fallback) ·
`cargo check` / clippy warnings stay at the **57** baseline · frontend `tsc` / `eslint`
green · i18n keys **2286** (+2, zh/en aligned; union **2288**) · wording, key-usage and
duplicate-key gates green.

## Unreleased — Batch 34: make a stall actionable — one click to send "change approach" into the running task (2026-10-21)

The previous two batches made stalls **visible** (batch 32) and made the banner
**name what the Agent is doing** (batch 33). But the last line still said
"stop it to redirect, or just tell it what you want" — **both suggestions require
the user to do the work**. This batch turns the first suggestion into a button.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

### What users want is not "see the stall", but "make it stop stalling"

The banner can already name "Most repeated: read_file src/main.ets × 9". After seeing it,
the user has only two options:

- click **Stop** — **terminates the whole task**, including work already done;
- compose a message in the composer — **the user has to translate the profile into
  an instruction**, which is exactly what the feature already computed.

⇒ A new **Change approach** button sends a **profile-bearing user instruction** into the
running task. It is still a user message, not a system prompt and not a decision made on
the user's behalf — it only removes the wording burden.

### The instruction names the profile, not just "stop stalling"

The body is generated from **the same `StallProfile` as the banner**:

> Let's change approach. You have gone 24 consecutive tool calls without real progress,
> most repeated: read_file(src/main.ets) × 9 · list_dir × 6.
> First state in one or two sentences what you know for certain and what is missing,
> then do one verifiable thing (edit code, build, test, or deploy) and report the result.
> Do not repeat the same read-only calls.

With no repeated action, the profile clause falls back to "mostly repeating grep_files
(24 times)". If the profile is missing entirely (older backend), only the consecutive
count remains — no empty parentheses or `undefined`.

### Why not `insert_queued_now`

The first attempt reused "insert now", but the backend command **unconditionally merges
every message queued at that moment** (the `ChatForceMerge` flag is per conversation, and
`assemble_round` drains the whole queue into the current round).
⇒ The user clicks "change approach" and accidentally promotes their other queued messages too.

Instead the nudge is queued as an **`agent_owned=1` message**: at the next safe point the
backend consumes exactly one Agent instruction and merges it, without stopping the task and
without touching the user's own queue. Same granularity (round boundary), no side effect.

Another path is impossible: `sendUserMessage` **returns early while the same conversation is
streaming** (duplicate-send guard), so clicking it during a run would do nothing. That is
precisely the gap between a "suggestion" and a "button" — **it looks clickable, but there is
no channel behind it**.

### State and failure

- One nudge per stall level: after success the button reads "Nudged" and is disabled;
  a level escalation (or a recovery followed by a new stall) yields a new stage and
  re-enables it.
- On failure a notification appears and the button becomes clickable again — a failure is
  **never latched as "Nudged"**.
- If the task ends between render and click (`isStreaming` is already false), it degrades to
  a normal send; the message is not lost.

### Boundary: still does not stop the task for the user

The button only delivers the user's intent faster. It **does not terminate the loop or force
a wrap-up**. "At which round to force-stop" remains the pending product decision in ledger
T-8; this batch continues to leave it alone.

### Verification

Frontend vitest **13/13** (7 new: profile in the body / dominant-tool fallback / no dirty
characters when the profile is missing / no button without a callback / one nudge per level /
failure allows retry / level escalation re-enables) · `tsc` / `eslint` green ·
i18n keys **2284** (+6, zh/en aligned) · wording, key-usage and duplicate-key gates green.

## Unreleased — Batch 33: the stall profile — so the user can see *what it is doing* (2026-10-19)

This batch continues the previous one. Batch 32 made stalls **visible** (level + stalled
bytes); this batch adds the **more important half** — **what it is actually doing**.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

### "24 consecutive calls" carries no information

Batch 32's banner said "24 consecutive tool calls without real progress". But the user
cannot tell two **completely different** situations apart:

- reading **the same file** 9 times — real spinning, time to stop;
- reading **24 different files** — exploration, also burning context, but not spinning.

The right response differs, yet "24" flattens them into one sentence.
⇒ The user saw "something is wrong" but still had **no idea what the Agent was doing**.

### Now the banner names it

> **Agent may be going in circles** · Level 2
> 24 consecutive tool calls without real progress · 160 KB added to context during the stall
> **Most repeated:** read_file src/main.ets × 9 · list_dir × 6
> Stop it to redirect, or just tell it what you want

When nothing repeats (24 different files), it falls back to one line:

> **Mostly using:** read_file × 24

- "Most repeated" lists the **same action** (same tool + same identifying argument)
  called at least twice;
- "Mostly using" is the tool called most during the stall window (**aggregated across
  targets**).

### The guardrail already had this — it just threw it away

`record_tool` has always received the **tool name and arguments**; they were only used
to count calls, never kept to answer "what did it do".
This batch records an **action profile** for the stall window, with the **same lifetime**
as the stalled-bytes counter (cleared on real progress — it describes *this* stall,
not the whole task).

What counts as "the same thing" **depends on the tool**, so it cannot always be `path`:
`read_file` keys on `path`, `grep_files` on `pattern`, `codebase_search` on `query`,
`run_command` on `command`.
Keying on `path` alone would collapse 20 greps with different keywords into "one
action" (they all carry the same default `path`) — and grep is one of the read-only
tools most likely to spin.

Reading **different windows of the same file** still counts as the same thing
(`read_file` can return 15K chars at a time, `AGENTS.md` 44.6KB in one go, so
re-reading it in slices is the textbook way to burn context) ⇒ window parameters like
`start` / `lines` are excluded from the identity.

### One profile, fed to both the model and the user

The tier 2/3 wording injected into the model used to say only "repeated read-only
calls of the same kind will not bring new information" — **without naming which
kind**. It now names it: "the most repeated action is read_file(src/main.ets) × 9".

Both sides share **the same structured data** (the backend `StallProfile`), so the
banner can never say `read_file` while the model sees `grep_files`.
(The prose is two versions on purpose: the model's prompt is Chinese, the UI goes
through i18n in Chinese and English.)

### Also added: frontend ⇄ backend event **payload field** reconciliation

While adding a field to an event, this batch ran into a spot **nobody was guarding**:
event names are string literals, and **payload field names are strings too** — the
frontend hand-writes `listen<{ conversation_id: string }>`, the backend hand-writes
`pub struct XxxEvent { … }`, and **neither compiler can see the other**.
⇒ With a mistyped field name: type check green, unit tests green, build green, no
runtime error — the field is **permanently `undefined`** ⇒ the feature silently dies.
And the 21st gate only checked **event names**, so it was completely blind to field
mismatches.

⇒ The 21st gate grew a **payload field reconciliation** pass: every field the
frontend declares in `listen<{…}>` must exist in the backend event struct's
serialized output. The judgment only looks in one direction — **frontend ⊆ backend**
(the backend sending extra fields is not a defect, and checking it would misfire).

**24** frontend/backend payload field sets are compared today, all consistent.
`json!` payloads and named types cannot be compared yet (they would need cross-file
type resolution); every skip leaves a note, and there is a **lower bound on the number
of comparisons** — if the extraction rules ever break, the gate goes red instead of
silently having nothing to check.

### Verification

Rust unit tests **21/21** (10 new: profile accumulation / only real repeats /
cross-target aggregation / cleared on progress / progress actions excluded /
per-tool identity / window params ignored / deterministic order / wording names the
action / hint carries the profile) · frontend vitest **6/6** (new `HomeBars.test.tsx`) ·
21st gate self-test **64/64** (was 41; 23 new payload-reconciliation cases) ·
mutation **13/13** (2 of them are **reverse-evidence** cases that must go green) ·
plus 2 no-change control runs · full gate suite **48/48**.

## Unreleased — Batch 32: making stalls visible — so the user can see the Agent going in circles (2026-10-19)

This batch is a **product feature** (the previous one was a gate). It adds a user-visible
notice: when the Agent makes several tool calls in a row without real progress, a stall
banner appears at the top of the conversation — **and shows how much context the stall
has already eaten**.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

### Until now, only the model could see it

The stall guardrail (T-8) has been injecting tiered wording into the **model's prompt**
for a long time, but **the user could see nothing**:

- the interface stayed quiet while the backend burned context at the rate T-8 measured —
  a **9-character question → 209,825 input tokens**, of which tool time was only **2.2%**;
- the "bytes accumulated during the stall" the guardrail already computed went **into the
  prompt and nowhere else**;
- the budget prices **rounds and duration only** — it is completely blind to this cost.

⇒ the user could only find out from the bill, with **no handle to stop it**.

### There is now a banner

A stall banner appears at the top of the conversation (just under the notes bar):

> **Agent may be going in circles** · Level 2
> 24 consecutive tool calls without real progress · 160 KB added to context during the stall
> Stop it to redirect, or just tell it what you want

- Levels 1/2 use the warning colour, level 3 the danger colour (matching the backend tiers);
- the data comes from a new backend event, `chat-guard`, emitted **only when the level
  changes** (with `stage=0` when it recovers) — every tool call during a stall trips the
  guardrail, so emitting each time would flood the interface;
- the backend also emits one `stage=0` **when the task ends** (including a user stop),
  otherwise the banner would keep a **stale warning**: the user would see "the Agent is
  stuck" long after it had actually finished.

### One source of truth for the level

The tiering was extracted into `task_guard::stall_stage` as the single source of truth;
both the wording and the notification go through it. Computing it in two places produces
states like "the banner says level 1 while the model sees level 2" — where each side
individually looks perfectly fine.

### Boundary: still only a notice, **no forced termination**

*Which round should force the loop to stop* remains a **product decision** (the open part
of T-8) and this batch does **not** touch it: even the heaviest tier only asks the model
to "do one of two things this round" — it **does not actually end the loop**.
What this batch adds is a handle for the **user** to stop it, not a decision made on the
user's behalf.

### Verification

Rust unit tests **11/11** (4 new: the level function, wording and level agreeing,
"notify only on level change", bytes carried by the event) ·
mutations **7/7** (including 2 reverse-evidence cases) · full gate suite **48/48**.

## Unreleased — Batch 31: the 21st gate (frontend/backend Tauri event reconciliation) (2026-10-19)

This batch changes almost no product behaviour: it adds one gate and four documents, and
removes one **never-emitted** dead event subscription.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

### The third face of the same problem

Gate 18 checks that i18n keys referenced by code exist. Gate 20 checks that the command
name a frontend `invoke`s is registered in the backend's `generate_handler!`. Tauri
**events** are the third face of the same problem:

- the frontend subscribes with `listen('evt', cb)` (`@tauri-apps/api/event`);
- the backend emits with `app.emit("evt", payload)`.

The event name is a **string literal** ⇒ renaming or deleting a backend event does
**not** break the frontend build; and nothing fails at runtime either — `listen`ing an
event that never arrives simply **never fires**. The symptom is "this feature just
doesn't do anything", with **nothing anywhere reporting an error**.

And the desktop app is **not the only consumer**: the LAN bridge keeps an
`EVENT_WHITELIST` in `lan_server.rs` (forwarded as SSE to the web client), and
`lan_ui/app.js` then subscribes with `on('X', …)`. ⇒ the event name is stored in
**three** places.

### The probe fell over first: 3 of the 4 "nobody emits it" were false positives

The first probe only matched `.emit("literal", …)` and reported 4 events the frontend
listens to but the backend never emits. Checking each one:

| Event | Reality |
|---|---|
| `node-runtime-progress` | The backend **does** emit it — the name hides behind `const EVENT: &str = "…"` |
| `git-runtime-progress` | Same |
| `jdk-install-progress` | Same, and the constant lives in **another file** |
| `chat-reasoning` | The backend **never emitted it** — a genuinely dead listener |

⇒ event names need **three** layers of resolution: literals / same-file constants /
**cross-file helper-function parameters** (including pass-through). A bare literal regex
only sees **90 of 91** `.emit(` call sites.

> ⚠️ **A false positive costs more than a false negative**: it makes someone delete a
> listener that is alive. **"I didn't see it" and "it doesn't exist" are not the same
> statement.**

### "Dynamic" has **opposite** polarity in the two directions

In gate 20 the dynamic part is a **call site** (`invoke(cmd)` with a variable `cmd`)
⇒ it makes "nobody calls it" an upper bound, and it does **not** suppress a statically
visible real gap ⇒ still judged red. Here the dynamic part is an **emission site**
⇒ it makes "nobody emits it" an upper bound ⇒ "this is a dead listener" is **not a
fact** ⇒ **downgraded to report-only**.

Copying one template into the other gets the direction exactly wrong.

### Real finding: `chat-reasoning` was a subscription nobody ever emitted

The backend merges content and reasoning increments into the `chat-stream-batch` event
(`StreamEventBatcher::flush()`) and **no longer emits `chat-reasoning` on its own** —
yet the `/// 思考增量（透传 emit chat-reasoning）` comment in `chat.rs` still described
the old design.

⇒ the desktop app carried a `listen('chat-reasoning', …)` that **could never fire**;
the LAN bridge whitelist carried the same dead entry; and the LAN web client carried
an empty `on('chat-reasoning', () => { /* keep alive */ })` listener.

All three removed, and the stale comment corrected. **Reasoning display is unaffected**
— it has always travelled in the `reasoning` field of `chat-stream-batch`.

### A mutation caught a hole in my own gate

Mutation N6 injected an emission site whose event name cannot be determined, expecting
the gate to **downgrade to report-only**. Instead it kept judging against the full
set — the point had been **silently dropped**.

Root cause: the classification logic was split in two, and "the identifier is neither a
constant nor an event-source parameter" was handled by **neither**.
⇒ **Classification must be done in one place.** This is the same trap as batch 30's
"write the zero-guard for only one side", in a different shape.

### Nearly wrote "nobody guards this" when a coarse guard already existed

The full gate run turned up a `cargo test` failure I had **not** expected:
`src-tauri/tests/frontend_backend_contract.rs` already contained an event-reconciliation
test. So this surface was **not** unguarded — that guard is simply **coarse**: it
extracts frontend calls from raw text, and only requires the event name to **appear
somewhere in the Rust sources** (comments, whitelists and constants all count).

`chat-reasoning` still had a literal in the LAN whitelist and in the docs ⇒ the coarse
guard **let it through**, while the fine guard (which requires the backend to **really
emit** it) flagged it. **Both layers stay.**

### My own comment made the coarse guard go red

The failing item did not come from code — it came from a **comment written in this
batch** that copied the shape of a listener call, which the coarse guard then read as a
real subscription. The comment was rewritten, and the coarse guard now strips comments
(**preserving length**) with a self-test for the stripper itself. The backend side
deliberately does **not** strip comments: in Rust `'` is a *lifetime*, so the JS-style
stripper would swallow real code.

### Verification

New gate self-test **41/41** · mutations **6/6** (including one reverse-evidence case:
after the backend switches a literal to a constant the gate must **still** resolve it,
not rely on a literal coincidence; and one downgrade case: an emission site whose event
name cannot be determined must be report-only, not red) ·
`check-self-test-counts.py` reports "the ledger matches the real source for **21** gates" ·
`check-docs.py` green.

## Unreleased — Batch 30: the 20th gate (frontend/backend IPC reconciliation) — which reported a false all-green on its very first run (2026-10-16)

This batch changes no product behaviour — it adds one gate and four documents.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

### The same problem on a different surface

Gate 18 checks that i18n keys referenced by the frontend exist in the translation
files. Tauri IPC is **structurally identical**, except the two ends are TypeScript
and Rust:

- every frontend call goes through the two wrappers in `src/api/invoke.ts`
  (`invokeWithError` / `invokeSafe`), which end up at
  `@tauri-apps/api/core`'s `invoke(cmd)`;
- the backend registers commands in `lib.rs`'s `tauri::generate_handler![ ... ]`,
  measured at **334** entries.

The first argument of `invoke` is a **string** ⇒ renaming or deleting a backend
command does **not** break the frontend build; it only fails with
`command not found` once a user reaches that screen. And `check-docs.py` only
counts the entries — it never compares names.

### The probe failed first: regex cannot handle nested generics

The first-draft probe reported "12 registered but never invoked". Grepping two of
them by hand:

```
src/api/project.ts:703:  invokeWithError<Record<string, ScopedCounts>>('project_scoped_counts')
src/api/project.ts:1014: invokeWithError<Array<[string, string]>>('list_tool_groups')
```

**Both are called.** `<[^>]*>` stops at the first `>` inside
`Record<string, ScopedCounts>>`, swallowing the whole call.

⚠️ This is the **second** appearance of batch 27's "hand-rolled lexer ate the real
references": **a hand-written parser necessarily breaks on nested structures, and
the way it breaks is silent under-reporting**, which is indistinguishable from
"really absent" in any report. Batch 27's conclusion (the repo already ships a
TypeScript parser — use it) was right; what was wrong is that I did not apply it
this time. Switching to `ts.createSourceFile`: **325** frontend commands,
**0** dynamic call sites.

### The gate reported a false all-green on its first run

```
$ node scripts/check-ipc-commands.mjs
IPC commands aligned: frontend invokes 0 commands, all registered in generate_handler! (backend registers 334)
$ echo $?
0
```

Root cause: in `import { invoke as tauriInvoke }`, `el.name` is **tauriInvoke**
and `el.propertyName` is **invoke**; I compared the wrong one ⇒ the derived
wrapper set came out **empty** ⇒ not a single call was collected ⇒ "zero missing"
⇒ all green. And I *had* written a "registered list parsed to 0 entries ⇒ red"
guard at the time — **I just only wrote it on the registry side.**

> **A check with N parse steps needs its zero guard N times over.**
> An empty `calls` can equally mean an empty `wrappers`, not "the frontend really
> doesn't call anything."
> ⚠️ Only the self-test can catch this: a human reading "frontend invokes 0
> commands" just assumes "oh, the frontend doesn't call anything".

### Four criteria

`scripts/check-ipc-commands.mjs`:

| # | Criterion | Direction |
|---|---|---|
| 1 | Frontend calls ⊆ backend registrations | **red** (the only direction that cannot produce false positives) |
| 2 | Entry-point closure: `invoke` from `@tauri-apps/api/core` may only be imported by `src/api/invoke.ts` | **red** (guards the gate itself: the wrapper list is *derived* from there) |
| 3 | A command name must not be registered twice | **red** (a duplicate makes `invoke` hit only the last one) |
| 4 | Registered but never invoked by the frontend | **report only** (`--report-unused`) |

The empirical basis for criterion 2: across the whole repo, only
`src/api/invoke.ts` actually binds `invoke` from `@tauri-apps/api/core`; the
other three sites only take `convertFileSrc`.

### The 9 "registered but never invoked" — verified not to be a bug

All 9 live in `commands::reliability::` (the list/get/claim/retry half of the
scheduler queue and the DAG). From "the underlying function has zero production
callers" I inferred that **tasks were being enqueued but never claimed** —
**reading the code overturned that**:

- `scheduler::register_active` (production path, called from
  `commands/chat.rs:4849`) performs the claim **in place** right after
  `enqueue`, with a concurrency-key-guarded `UPDATE`
  (`scheduler.rs:280-296`);
- `scheduler::claim_next` is the **pull-based API for an external worker to poll**
  the shared queue — and the desktop app is single-worker, so there is nothing
  polling it.

⇒ They are "built, tested, not yet wired into the UI" operational surface.
Whether to delete them is a **product judgement**, so criterion 4 reports only.

> "The function has zero callers" has at least four explanations: genuinely dead /
> reached via reflection or a macro / used only by tests (so still not deletable) /
> **superseded by another path with the same name**. A statistical signal is a
> lead, not a conclusion.

### Mutations 6/6: 5 load-bearing on real source + 1 reverse evidence

N1 frontend invokes a command the backend does not have · N2 backend drops a
registration the frontend still calls · N3 bypassing the wrapper to import
core.invoke directly · N4 the same command name registered twice · N6 the wrapper
list failing to derive ⇒ all five go red. N5 writing the command name as a
variable ⇒ **exit 0 plus an explanation**.

N5 caught a **contract/implementation mismatch**: the docstring said "does not
fail the build", while the code pushed that message into `problems` and exited 1.
While fixing it, the self-test blocked me from over-correcting — a dynamic call
only turns "nothing found missing" into an **upper bound**; it must **not**
suppress a genuinely missing command that is statically visible.

> **An upper bound must never swallow a fact.** The correct output is
> "N problems seen, plus M places I cannot see" — not "0 problems".

### Verification

New gate self-test **16/16** · mutations **5 load-bearing on real source + 1
reverse evidence** · `check-self-test-counts.py` reports "**20** gates match the
ledger" · `check-docs.py` green · gates **46/46**
(20 gates + 20 self-tests + tsc + eslint + vitest + build + bundle + cargo).

## Unreleased — Batch 29: the first gate on the Rust backend, plus one false premise that nearly became a contract (2026-10-16)

This batch changes no product behaviour — it adds one gate and three documents.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**
(nothing added, nothing removed).

### Topic: 20 gate scripts, zero of them on the Rust backend

Auditing `scripts/`: 20 `check-*` scripts, all of them about the frontend and the
docs — while `src-tauri` is **224** Rust files / **156,212** lines /
**86** migrations with **no gate coverage at all**.

The probe numbers were tempting: all **86** migrations replay in order on an empty
in-memory database with **0 failures**, producing **71** tables and **102** explicit
indexes. The first-draft topic was "the migration chain must replay from zero".

### But that premise was false — one grep before writing the docstring

`run_migrations` is called by **dozens** of tests in `db/mod.rs`, each replaying the
full 86-migration chain on an in-memory database. `db/mod.rs` also carries a
hand-written registry:

```rust
(86, "086_drop_api_details_members", include_str!("../../migrations/086_drop_api_details_members.sql")),
```

⇒ **"nobody replays the migration chain from zero" does not hold.** Had I not
checked, the contract of a brand-new gate would have rested on an unverified claim.

> **Before writing a "gap in coverage", go find out whether it is already covered.**
> "nobody watches this" and "nobody watches this **and I verified nobody does**"
> look identical in a report.

### The real hole: two **counts** impersonating two **sets**

Once that was settled, the actual gap is that `migrations/` and the registry are
kept in sync **by hand**. `check-docs.py` has a line that appears to cover it:

```python
if files != registered:      # two counts
```

Deleting one `.sql` **and** its registration line decrements both counts by one —
they stay equal.

⚠️ I overclaimed here at first, writing "check-docs cannot see it at all".
**Measurement refuted that**: deleting both sides without touching the docs *does*
make it go red — but red for a different reason (the docs still say 86 while the
source now says 85). The decisive experiment — **change all 7 documentation numbers
to 85 as well**:

| | Result |
|---|---|
| `check-docs.py` | **exit 0, fully green** |
| The 19th gate | **exit 1**: `migration numbering is not contiguous… missing [50]` |

⇒ **A backstop is not a guard.** The documentation-number checks stop you from
*forgetting* to update the docs; they do not stop the *normal* workflow.

### Gate 19: seven criteria

`scripts/check-migration-registry.py`. It judges **reconciliation** only, not SQL
content or semantic ordering (the latter is the job of `run_migrations` replaying
from zero — no duplicated work).

| # | Criterion |
|---|---|
| 1 | **Set equality** (reported in both directions) |
| 2 | `id` == the numeric prefix of the filename |
| 3 | `name` == the filename without `.sql` |
| 4 | `id` must be unique — **the most insidious one**: `run_migrations` is `if !applied { execute }`, so once the first entry with that `id` runs it is recorded as applied and **a second migration with the same `id` is silently skipped and never executes** |
| 5 | The registry must be sorted by ascending `id` (array order is apply order) |
| 6 | **Parser shape guard**: `include_str!` occurrences == parseable tuple count |
| 7 | Numbering is contiguous from `001` — **the only criterion that catches "delete both sides"** |

Criterion 6 is not a business rule but a **parser guard**: a gate that parses
source as literal text will report **all green** the moment its parsing breaks, and
"all green" is indistinguishable from "genuinely fine" in any report.

Self-test **12/12**; the baseline case uses **verbatim** line 166 of the real source.

### Mutations 6/6, all load-bearing on the real source

N1 delete file + registration together (numbering gap) · N2 duplicate `id` ·
N3 `id`/prefix mismatch · N4 `name`/stem mismatch · N5 out of order ·
N6 a new unregistered file. After each restore, `db/mod.rs` and all 86 `.sql`
files were sha256-verified, and the baseline returned to green.

**Load-bearing, stated honestly**:

- Criterion 6 (the shape guard) carries **zero real-source load** — all 86 entries
  parse, and no "unparseable" real-source shape can be injected without breaking
  the Rust build. It is **carried by the synthetic cases alone**.
- Criterion 1's "registration points at a missing file" direction is covered on the
  real source by `include_str!` failing at **compile time** (`cargo build` blows up
  first); this gate is the second line of defence.

### Found along the way: a test whose comment describes something it does not do

`db/mod.rs`'s `migrations_apply_ohpm_sort_columns` carries the comment "the
migration list must be fully registered … preventing a new migration file from
being unregistered". What it actually does is run the migrations and then
`PRAGMA table_info` **spot-check a few specific columns of two tables** — it never
reconciles the registry against the directory. Worse, the guard its comment names
is from a previous generation (`check-docs.py`'s count check already covered
"new file unregistered"), so **the guard in the comment and the guard in the code
are not the same one, with a generation in between**.

> **Treat assertions in comments as claims to be verified, not as facts.**
> Corrected it in passing, so the next person does not assume someone is watching.

### Verification

New gate self-test **12/12** · mutations **6/6 load-bearing on real source** ·
`check-self-test-counts.py` reports "**19** gates match the ledger" ·
`check-docs.py` green · gates **44/44**
(19 gates + 19 self-tests + tsc + eslint + vitest + build + bundle + cargo).

## Unreleased — Batch 28: the gate I had just shipped had three false negatives (2026-10-16)

This batch changes no product behaviour — only a gate and the translation files.
Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86**.

### It started with a **self-contradictory** report

The dead-key census (an independent implementation) reported
`cost.errKind.rate_limited` as dead. Yet `src/pages/CostPage.tsx:198`
plainly reads `t('cost.errKind.rate_limited')`.

**When a census and a gate disagree, suspect the gate.** Gate 18 turned out to
have three false negatives:

| # | Shape it missed | What the real source looks like | Root cause |
|---|---|---|---|
| 1 | **Underscore** inside a key segment | `cost.errKind.rate_limited` | `NS_SHAPE` used `[A-Za-z0-9]+`, which **rejects `_`** |
| 2 | Key passed through a **wrapper declared in the same file** | `run(() => stop(), 'config.lanStopFailed')` | the AST cannot follow a local function |
| 3 | Key inside a **conditional expression** | `t(loading ? 'teamShare.loading' : 'teamShare.working')` | only `arguments[0]` was checked for being a string literal |

The three fixes:

- `NS_SHAPE` now allows `_` inside a segment (`[A-Za-z0-9_]*`).
- When calling a function **declared in this file**, its namespace-shaped string
  arguments count as references too. The criterion is a `locals` set, **not a
  function-name blacklist** — `run` (a local `const`) and `track` (an imported
  telemetry API) have exactly the same shape. A blacklist is a guess; "was it
  declared in this file" is decidable.
- Arguments are **walked as a subtree**, but only the **first** argument of
  `t(...)`: walking the whole file drags in telemetry names (`project.trust`),
  module names (`highlight.js`) and file names (`diagram.svg`) — measured
  **15 false positives**.

### A fourth hole, closed along the way: file names were being read as keys

Added `FILE_SHAPE` to exclude shapes like `diagram.svg` / `image.png` /
`qrcode.react`.

### Dead keys: **reported, never failed**

The translation files carry **2271** leaf keys each in zh / en, **2273** in
union — the 2-key difference is the i18next plural form
(zh `home.batchLines` ↔ en `home.batchLines_other`).

| Item | Count |
|---|---:|
| `t()` static references | **2550** occurrences, hitting **1989** distinct keys |
| keymap references | **103** keys |
| Distinct keys referenced at all | **2088** / 2273 |
| Dynamic keys | **38** sites, covering **1101** keys across 13 files |
| Trustworthy dead keys (after excluding static refs / keymap / dynamic coverage) | **6** |
| Confirmed dead by hand, then deleted | **2** |

Every number in that table is printed directly by
`node scripts/check-i18n-key-usage.mjs --report-dead` — the reporting mode was
extended to emit those figures. The previous version of the table carried
"static references 1965 / keymap 145" from a throwaway census script, and
**those cannot be reproduced on another machine**: the census counted raw
entries, the gate counts deduplicated keys, and mixing the two left nobody able
to say what 1965 actually was.
**A number in the ledger has to be something the gate can print again**,
otherwise it is a second-hand remark nobody can check.

The deleted pair is `apiKb.embedReady` / `apiKb.embedUnavailable` (removed from
both languages); they had long been superseded by the `embedState*` family.

All 6 survivors were checked one by one and are **all alive**: 3 are keys passed
through a tuple array (`src/pages/ReproductionBundlesPage.tsx:62`), 3 live in the
non-i18n data file `src/data/providerTemplates.ts`.

**Why this direction does not fail the build**: the moment a dynamic key is
written as a bare prefix (such as `home.`), coverage degrades to the whole
namespace and what remains is an **upper bound**, not a fact. Failing would only
force a whitelist that then has to be maintained forever. The bound of the
criterion is right there; writing "must be zero" would be self-deception.

### A path rejected: deleting dead keys to buy first-screen budget

Real dead keys are **0.09%** of the file (2 / 2271), so the payoff is **zero**;
the cost is a new criterion to maintain long-term plus a whitelist that will
inevitably need entries. Recorded as a rejected path (same family as T-5,
"compacting the translation files").

### Methodology: run a **differently-implemented** census beside every gate

A freshly shipped gate has false negatives too, and it will **never announce
itself as "the gate is broken"** — it only surfaces when a different tool
reaches the opposite conclusion. All three false negatives here were found
exactly that way.

### Verification

New gate self-test **21/21** (+6) · mutations **13/13**:
N1 misspelled leaf key / N2 misspelled namespace / N4 underscore segment /
N5 conditional / N6 local wrapper = **load-bearing on real source**;
N3 injects a bad key into a comment only = **reverse evidence** (proving it
is not passing by "report anything that mentions `t()`");
M1 dropping the namespace-shape restriction makes the **real source** exit 1.
Each was restored with a sha256 check, and the baseline returned to green.

`check-i18n-dupe-keys.py` green (zero duplicate keys, empty whitelist) ·
`check-i18n-wording.py` green (**2271** keys / 0 unexplained conflicts) ·
`check-self-test-counts.py` reports "**18** gates match the ledger" (including
the `.mjs` one) · gates **42/42**.

### A documentation-structure defect, fixed in passing

Four consecutive batches (25 through 27) had been written into this file as
`###` subsections nested under **Batch 24**'s heading — a heading dated
2026-10-02. A changelog that describes itself as reverse-chronological had four
shipped batches buried inside someone else's subsection.

This batch promotes those four blocks to `##` level and moves them **above**
Batch 24.

⚠️ `check-docs.py` matches the current-scale declarations **only inside the
first `## ` section**, so the new top section must carry the `TOOL_SPECS` and
migration-count sentences itself — that is precisely the constraint behind its
"only the newest version section" rule, and it is easy to trip over when
reshuffling this file.

## Unreleased — Batch 27: the code references i18n keys that **do not exist** — three user-visible (gate 18, 2026-10-16)

| Key | Symptom in the UI |
|---|---|
| `common.off` | the dropdown option literally reads **`common.off`** |
| `md.copied` | after copying a code block the button literally reads **`md.copied`** |
| `home.queueFailed` | it carried a Chinese default, so **English users saw Chinese** |

i18next's fallback renders **the key name itself**, so the symptom is not "the
translation didn't load" — it is a raw key sitting on screen, which sends you
debugging in exactly the wrong direction.

`check-i18n-wording.py` structurally cannot catch this: it compares whether the
two languages line up, and when a key is missing on **both** sides the parity
check sees nothing wrong.

A fourth hit, `about.tauri`, turned out to be inside a **comment** — the code had
long since switched to `about.runtimeValue`. A dead key name in a comment is still
harmful, so the comment was fixed too.

### The big one: a hand-rolled lexer **ate the real references**

The first version used a hand-written Python state machine to strip comments.
Injecting a bad key and running it, **the gate was green**. Investigation showed:

| File | Real references | Left after comment stripping |
|---|---:|---:|
| `Home.tsx` | 424 | **35** |
| `ConfigPage.tsx` | 75 | **0** |
| `Markdown.tsx` | 27 | **0** |

Root cause is template literals —
`` setError(`${t('config.clearError')}: ${e}`) ``: inside `` `${...} `` there is
**real code**, and the state machine blanked the whole template as a string.

⇒ The repository already has TypeScript (`tsc` runs it daily). Switching to
`ts.createSourceFile` + AST walk: not one reference lost, dead keys inside comments
still correctly ignored, and an injected bad key is reported with an exact
`file:line`.

> "Nobody guards this" comes in two kinds: **the guard was never built**, and
> **the guard was built but has been dead all along**. The first mutation round
> reported "load-bearing: none" and I nearly concluded the former; the truth was
> the latter.

### A gate's language need not be uniform, but the **ledger parsing must follow**

With the gate now in Node, the gate-list regex in both `check-self-test-counts.py`
and the full verification script widened from `check-xxx.py` to `py|mjs`.
Otherwise the new gate is reported as "the ledger is missing N gates" —
**which looks like a documentation problem but is actually a parsing-scope problem**,
and is very easy to fix in the wrong place.

### Two directions are red; dead keys are not judged

"A namespace that does not exist" was simply filtered out in the first version,
but its symptom is identical to a missing key and the real source has **0**
such hits — so judging it produces no noise and there is no reason to leave a
silent hole.

Dead keys are not judged: 13 files use dynamic keys ``t(`...`)`` which cannot be
enumerated statically, so any number reported is an **upper bound, not a fact**.

### Mutations 8/8

| Mutation | Real source | Self-test | Load-bearing |
|---|---|---|---|
| Inject a **misspelled leaf key** | red | green | **Real source** |
| Inject a **misspelled namespace** | red | green | **Real source** |
| Bad key in a **comment** only | green | green | Reverse evidence (must not false-alarm) |
| No longer require namespaced shape | green | red | Synthetic only |
| No longer exclude test files | green | red | Synthetic only |
| No longer check "this file uses i18next" | green | red | Synthetic only |
| Silently drop "namespace missing" | green | red | Synthetic only |
| Missing keys never reported | green | red | Synthetic only |

### Verification

New gate self-tests **15/15** · mutations **8/8** ·
`check-self-test-counts.py` reports "**18** gates match the ledger" (including the
`.mjs` one) · `check-i18n-wording.py` green (**2273** keys / 0 unexplained
conflicts) · gates **42/42**.

## Unreleased — Batch 26 (cont.): C-4 settled: the root cause of those 5 conflicting strings was **one key shared by two panels** (2026-10-16)

Tracing the consumers showed the 5 conflicts whitelisted in the previous section
were **not a question of which wording is better**.

| Consumer | What it actually is | What it has always displayed |
|---|---|---|
| "More" menu button (`handleOpenTimeline` → `loadSnapshots`) | the **snapshot timeline** dialog | Timeline (back to a checkpoint) / No checkpoints yet… |
| Sidebar `timeline` item → **`TimelinePanel`** | the **session event log** (grouped by `trace_id`) | the very same two keys |

Two entry points leading to two different things while sharing one key means:

- the sidebar tab has always read "Timeline (back to a checkpoint)" while **what it opens
  is the event log**;
- `TimelinePanel`'s empty state has always told the user to wait for a checkpoint
  that has nothing to do with it — it lists **events**.

**Fixing one screen's wording broke the other, and nothing reported it.**

⇒ **The shadowed sentence was not an outdated draft — it was a sentence that belonged
to a different panel and was simply filed under the wrong key.** The right fix is not to
pick a winner but to **split the key per consumer**: `home.timelineLog` /
`home.timelineLogEmpty` (both languages), with the two shadowed sentences **moved back
verbatim** to where they always belonged — not one word of the copy was rewritten,
it was just filed correctly.

`home.forkFromHere` is a different case: the Chinese changed and **the English did not**
⇒ en now reads `Fork session from here` (keeping the "fork" term the code already uses,
adding the missing "session").

> **Generalisation**: the same string appearing in two places has **two** possible causes
> — the wording drifted, or two genuinely different features share one key.
> The first is fixed by editing copy; **the second merely moves the defect from one side
> to the other**. Grep the key's consumers and ask "are these the same thing?" first.

### The cost of emptying the whitelist: real-source load drops from 6 to 2

Settling C-4 empties the gate's whitelist — zero exemptions from here on. But with no
duplicate left in the real source, weakening the criterion can no longer turn it red.
The remedy is a different proof: **inject duplicates into the real files** (N1 identical,
N2 conflicting). The two kinds of proof guard different things — one asks "is the
criterion accurate", the other "would a duplicate in the real source be caught" — and
**once the debt is clean, only the latter remains**.

> A **mislabeled mutation** was also fixed: the first version labeled "key path loses its
> parent block" as "cross-object same-name also counts". The former changes **what gets
> reported**, the latter changes **how much gets judged**. A wrong label tells the next
> reader a property is guarded when nothing guards it.

### This gate caught a duplicate key **I created in the same batch**

When splitting the key I first named it `home.timelineEvents`; the gate went red
immediately — that name was already taken by "{{count}} events", i.e. **I had made
exactly the mistake it exists to catch**.

### Verification

`check-i18n-dupe-keys.py` **zero duplicates, empty whitelist** · self-tests **13/13** ·
mutations **8/8** (sha256-verified restore each time, green baseline) ·
`check-i18n-wording.py` green (**2270** keys / 0 unexplained conflicts) ·
`tsc` / `eslint` / `vitest` green · gates **40/40**.

## Unreleased — Batch 26: **9 keys written twice** in the translation files; the first copy can never display (gate 17, 2026-10-16)

JSON permits duplicate keys, and `JSON.parse` / Python `json` / `serde_json`
**all keep the last one and silently drop the earlier ones**. Measured:
**5** in `zh.json`, **4** in `en.json` — **all inside the top-level `home` block**.

### How silent it is — **the other i18n gate cannot see it either**

`tsc` / `eslint` / `vitest` / `build` all green is routine.
The notable part is `check-i18n-wording.py`: all three of its parse sites
(lines 183 / 512 / 513) call bare `json.loads(raw)` —
**the parser has already thrown the duplicate keys away for it**.

⇒ The two i18n gates are **complementary, not overlapping**: one covers duplicate
keys, the other covers wording, and **neither can find the other's blind spot**.

### "Whether to delete" and "which one to keep" are two different questions

Only the last value is ever used at runtime, so **deleting the shadowed copy is a
zero-behaviour-change edit** regardless of which wording is correct; but **which
one should stay** is a wording decision.

| Category | Count | This batch |
|---|---:|---|
| Both copies **textually identical** | 4 | **Deleted** (not even a wording question) |
| The two copies **differ** | 5 | Whitelisted, logged as content debt **C-4** for a human |

Collapsing this into "duplicate keys are just cruft, delete them" would have made
5 wording decisions on the user's behalf.

`home.forkFromHere` has an extra layer: **the Chinese changed and the English did
not**. The two `zh` values differ ("从这里分叉" / "从此派生会话") while both `en`
values are identical and both are literal translations of the *old* Chinese
⇒ the current Chinese string **has no counterpart in English**.
`check-i18n-wording.py` structurally cannot catch this: it compares casing and
word substitutions, and translations of the same key are *supposed* to differ.

### How "zero behaviour change" was proven

Not by "I don't think it affects anything", but by flattening both files before and
after and comparing **every key and value**: **2268** leaf keys per file, all
identical, two lines shorter each. That also shows **deleting duplicates changes no
number derived from the translation files**.

### New gate 17 — `scripts/check-i18n-dupe-keys.py`

The criterion covers duplicate keys **within the same object** only —
the same key name in *different* blocks is fine (`searching` legitimately exists in
`home`, `nav` and `perf`). Misjudging that turns a gate into noise,
and **the fate of noise is a whitelist**.

Self-tests **13/13**, mutations **8/8** (sha256-verified restore each time, green baseline):

| Mutation | Real source | Self-test | Load-bearing |
|---|---|---|---|
| Only flags keys seen 3+ times | red | red | **Real source** |
| Top-level objects only, no recursion | red | red | **Real source** |
| Cross-object same-name also counts | red | red | **Real source** |
| Whitelist filter inverted | red | red | **Real source** |
| Whitelist emptied | red | red | **Real source** |
| Whitelist carries one extra, non-existent key | red | green | **Real source** |
| Value comparison not normalised | green | red | Synthetic only |
| No longer reports stale whitelist entries | green | red | Synthetic only |

The two **"synthetic only"** rows are called out honestly: the real source has no
duplicate whose value is an object (so key-order normalisation is untested by real
data), and no stale whitelist entry currently exists to report.

### A real hole found during reverse review: a whitelist can turn from exemption into vulnerability

The first version only blocked duplicate keys that were **not** whitelisted.
But if someone fixes `zh.json`'s `home.timeline` and forgets to remove the whitelist
entry, then **any future duplicate at that same key is silently allowed** —
the whitelist has turned from an exemption into a hole, and **nothing reports it**.

⇒ The whitelist has to stay aligned with the real source in **both** directions:
unregistered duplicates must be blocked, and **stale entries must be blocked too**.
Adding that check also gave the criterion itself a **second net**: after M1 / M3
weakened the criterion, the real source flipped from green to red — and the reason it
went red was **not** "the duplicate wasn't detected" but **"the five registered
duplicates can no longer be found"**.

> The self-test caught a bug of mine: the sentinel used a `tuple` to preserve
> key order, so normalising a value made `json.dumps` serialise objects as
> **arrays**, pushing key order back into the "same value" decision.
> "Same value" has to be defined first: **equality by content or by spelling?**
> By spelling, `{"x":1,"y":2}` and `{"y":2,"x":1}` become two different sentences,
> though they are the same value in JSON semantics.
> The same run also caught **my test case being wrong** (I had treated "unregistered"
> as "stale") — the criterion was right, the expectation was wrong, and both had to
> change.

### Also corrected a number that **never existed**

The ledger's "i18n keys **2119**" is checked by **no gate at all**. Replaying
history under the same counting rule, the closest values are **2120** and **2118**,
both from 2026-09-15 — **2119 never existed**. It was not a stale value, it was a
**mistyped** one. Replaced with **2268**, the value `check-i18n-wording.py` reports.

### Verification for this batch

New gate self-tests **13/13** · real-source mutations **8/8** ·
`check-self-test-counts.py` reports "17 gates match the ledger" ·
`check-i18n-wording.py` green (2268 keys / 0 unexplained conflicts) ·
`check-docs.py` green · gates **40/40**.

## Unreleased — Batch 25: the stall guardrail went from "only reminds" to a three-stage ladder (T-8, 2026-10-16)

A review of one real session found the guardrail repeating **the same sentence 16 times
without the model ever changing strategy** — a 9-character question, 25 tool calls,
**209,825 input tokens**, of which tool execution was only **2.2%** of the time.
The old `stall_warning` was flat prose whose only variable was the round counter.

| Stage | Trigger | Content |
|---|---|---|
| 1 | `≥ threshold` | original wording kept — a task that just crossed the line shouldn't be startled |
| 2 | `≥ 2×threshold` | names the trap ("more of the same read-only calls will not produce new information") and demands one **verifiable** action this round |
| 3 | `≥ 3×threshold` | **must do one of two things this round**: produce a verifiable step, or report status and recommendations to the user |

Three design constraints:

- **Staged by multiples of the threshold, not by absolute rounds.** The threshold is
  configurable on the settings page (a user may set 3 or 30); hard-coded absolute rounds
  would strand one end at stage 1 forever and send the other straight to the top.
  The self-test checks `[3, 10, 30]`.
- **The guardrail can see bytes for the first time.** `record_tool` gained a
  `result_bytes` parameter, and at the call site `result` *already is* the tool output
  string, so `len()` is free ⇒ the **context KB consumed during the stall is now written
  into the stage 2/3 wording**, and reset the moment real progress happens. That targets
  "the budget counts only rounds and wall-clock, and is completely blind to bytes".
- The signature changed, so **all 12 call sites were updated** (including the existing
  self-tests). Each was done with `replace_all` then re-read for verification — the first
  pass missed one, found only by grepping (because `&args, true` and `&args, false` are
  different patterns).

The settings-page description for the stall threshold was updated to say "escalating
reminder" (both zh and en).

### Mutation verification caught a **fake self-test of my own**

The first version of the byte-counting self-test only asserted "no longer stalled after
progress" — but that assertion is decided by `since_progress` alone, so deleting
`stalled_bytes = 0` left it **fully green**. Mutation M2 produced no reaction, which
forced a rewrite into a criterion that can actually tell the difference:
**the second stall accumulates back to 160 KB rather than continuing on to 320 KB**.

| Mutation | Result |
|---|---|
| M1 three stages → one flat sentence (the original shape) | **red** (3 tests) |
| M2 remove `stalled_bytes = 0` | **red** (the first version could not be moved; it can now) |
| M3 keep only two stages (stage 3 never appears) | **red** (3 tests) |
| M4 stage by absolute rounds instead of threshold multiples | **red** (2 tests) |
| M5 bytes never accumulate (always 0) | **red** (1 test) |

### The half deliberately **not** done

"At which round should the guardrail **force the task to finish**" remains an undecided
product question. Even the heaviest stage only **asks** the model to pick one of two
things — **it does not actually terminate the loop**. Crossing that line would no longer
be an engineering judgement; it would be making a product decision on the user's behalf.

> **Stating plainly what this batch cannot verify**: whether the ladder actually changes
> the model's behaviour **cannot be checked here** — that needs a real Agent session.
> What *can* be verified is only the fact that the wording no longer repeats identically.

### Verification for this batch

`task_guard` self-tests **7** (+3) · mutations **5/5** · full `cargo test` ·
`check-i18n-wording.py` green · gates **38/38**.

## Unreleased — Batch 24: overturning my own root cause from the previous batch (2026-10-02)

Tool count unchanged: `TOOL_SPECS` to **204**, migration count reaches **86** (migration 086 added: dropping the write-only `api_details.members` column).

This batch started from an already-decided product choice (index only the latest
version, dedupe by natural key). **Pre-implementation verification showed that
decision rested on a false premise**, so implementation was paused and the real
cause was investigated first. What was found is considerably worse than the
original problem — and it overturned the root cause this same codebase recorded
in the previous batch.

### First, a retraction: the 72.2% vector collision was never a versioning problem

| Item | Assumed | Measured |
|---|---:|---:|
| distinct `doc_text` | 18,506 | **38,566** (text dedup saves only **14.0%**) |
| remaining 20,060 **distinct texts** with byte-identical vectors | version duplicates | **the tokenizer collapses every camelCase API name into `[UNK]`** |

The decisive evidence is one pair of numbers lining up:
**distinct token sequences = 18,506 = distinct vectors**. Rows with the same token
sequence necessarily get byte-identical vectors, so collisions are **100%
explained by tokenization**.

A reference implementation (numpy + torch) **reproduced the stored vectors to
1.1e-06** (cosine 0.9999998) and reproduced the same collisions — so the model
weights, the candle inference path, the indexing code and the database are all
innocent.

> Lesson: `version_label` genuinely does not participate in encoding, and 20,057
> rows concentrated under one version label looked exactly like the culprit — but
> **the denominator was never printed**. The same "duplicate" symptom has two
> completely different causes, and only one explanation happened to fit.
> **Separate "identical text" from "different text, identical vector"**: the former
> is data redundancy, only the latter is an encoding defect.

### Root cause: one flag against an all-lowercase vocabulary

`tokenizer.json` ships `lowercase: false`, but its vocabulary is **entirely
lowercase** (`get`∈V while `Get`∉V, `kit`∈V while `Kit`∉V). Every camelCase
identifier therefore collapses into a single `[UNK]`:

| Metric | Before | After |
|---|---:|---:|
| Rows containing `[UNK]` | **100%** (44,856 / 44,856) | **0%** |
| `[UNK]` share of 965,066 tokens | **24.8%** | **0.0%** |
| Non-empty API names collapsing entirely to `[UNK]` | **27,433 / 31,812 (86.2%)** | 0 |
| Largest collision group | **538 rows** | **24 rows** |

**The fix lives in source, not in the model file** — `src-tauri/resources/` is
gitignored, so editing the resource would not be committable and would be lost on
every CI build.

### Changing the encoding requires changing how "did the encoding change?" is decided

The old idempotence guard compared **row counts only** (`indexed >= total`), so a
tokenizer change with an unchanged row count would **skip the rebuild** — queries
encoded the new way against vectors encoded the old way, which is **worse than not
fixing it at all**.

An **index fingerprint** was added (`api_docs_meta.embedding_index_fingerprint`):
the source-level encoding-schema constants plus the **content** sha256 of the three
model files (not mtime). It is computed once at model load and cached, because this
is a search hot path. The guard now requires "row count sufficient **and**
fingerprint matching", and a missing fingerprint also forces a rebuild.
`vector_search` validates the same fingerprint before encoding the query and
degrades to keyword-only on mismatch.

### A third defect, uncovered along the way: a correctness bug that is shipping

This one is more serious than the other two because it is not a quality problem.
By unioning the two result lists, a document present **only** in the vector list
with no literal keyword match could outrank the BM25 #1 (vector #1 = 1/61, exactly
tying keyword #1 = 1/61), and ties were broken by `HashMap` iteration order, which
**differs per process**. Ties are not an edge case: **67.8% of queries have a
top-1/top-2 cosine gap of exactly zero**.

**Three contract changes (all narrowings, not new knobs)**:

- **When keyword hits exist**, that set is authoritative: the vector layer reorders
  within it and never introduces new documents. "Semantic but no literal match" is
  deliberately not enabled in that case.
- **When there are zero keyword hits**, vector hits are let through.
- **Ties break on ascending `doc_id`**, so identical input always yields identical output.

### Measured along the way: the keyword layer recalls **nothing** for semantic queries

To judge whether the vector layer is worth keeping, you first need to know how good
the keyword layer is — and nobody had measured it. Measured with the **production
code itself** (`harmony_api_diff::search` + `relevance::Bm25Index`; `cargo test`
does not enable the `embedding` feature, so the vector block is compiled out and
what gets measured is exactly the production keyword-only path):

| Query type | Keyword recall | Target rank |
|---|---:|---:|
| **Semantic / Chinese** (15 queries) | **0 rows (15/15)** | not recalled |
| `getWindowProperties` | 1 row | **rank 1** |
| `createWindow` | 6 rows | **rank 1** |
| `getBundleInfo` | 8 rows | **rank 1** |

Recall is `LIKE '%keyword%'` on literal substrings, and rows whose `declaration`
contains a non-ASCII character number **0 / 44,856**. **The keyword layer is a
reliable workhorse for exact identifiers (top1 3/3); it recalls nothing at all for
semantic queries.**

**This also exposed and fixed a regression from this same batch**: the first
narrowing filtered vector hits unconditionally, collapsing semantic search from
"50 results" to "**0 results**" — worse than before. The rule is now "a weak
signal is used only when there is no strong one", with no magic constants in either
direction. It also **overturns the "drop the vector layer, keep BM25" option**,
which would have driven Chinese semantic search to exactly zero.

### A zero-result search is no longer an unexplained dead end

When a keyword query matches nothing, the output used to say only
"共找到 0 条匹配（知识库总量 44,856 条）". Since Chinese / descriptive keywords are
**guaranteed** to miss at the keyword layer (table above), that was a high-frequency
dead end. The output now carries the **actual semantic-layer state plus what to do
next**:

| State | What the hint says |
|---|---|
| Ready | the semantic layer did not match either — try an exact API name or narrow the filters |
| Not indexed | build the index (about 20–30 min the first time) to enable semantic search |
| **Stale index** | vectors from before the upgrade or from the seed are not comparable — rebuild required |
| Local model unavailable | this run is keyword-only — use an exact API name for Chinese keywords |

The state is computed **from the database only, without loading the model** —
answering "why did nothing match" should not cost 1–2s of loading 91MB of weights.
The state enum and the degradation check inside `vector_search` **share one
function**, so the log line and the user-visible hint can never disagree.

### Stated honestly: fixing tokenization alone leaves search largely unusable

15 realistic developer queries (every target API verified to exist in the DB first):

| | Before | After |
|---|---:|---:|
| top1 | **0 / 15** | 1 / 15 |
| top10 | 1 / 15 | **3 / 15** |

The English query `get window properties` jumps from beyond rank 20 to rank 1;
Chinese queries still mostly fail. The cause is a **model/corpus mismatch**: the
model is a Chinese sentence encoder while the corpus is **100% ASCII**
(`declaration` rows containing non-ASCII characters = **0 / 44,856**) and consists of
English TypeScript signatures. Upstream was verified: `do_lower_case: false` and
`num_hidden_layers: 4` are **both official settings**, so the model files were not
corrupted. Whether to keep investing in the vector layer is now tracked as
product decision P-7.

### One change rejected by measurement (never landed in code)

After the tokenizer fix, 6,385 rows (14.2%) still shared a token sequence with
another row. "Fall back to `class_name` when `api_name` is empty" looked right:
`class_name` covers **99.9%** of rows, and 99.8% of the 13,044 rows with an empty
`api_name` have one. **The proxy metric did improve** — distinct sequences
38,471 → **41,939**, largest collision group 24 → **5**.

**But end-to-end over 15 queries, top1 fell from 1/15 to 0/15 and top10 from 3/15
to 2/15**, and the one query that actually worked, `get window properties`,
**dropped from rank 1 to rank 17**. The query side has no `class_name` to match
against, so the extra tokens are pure noise.

**Acting on the distinct-sequence count alone would have made the product worse.**
"More diverse outputs" and "more accurate results" are not the same thing, and only
the first one is easy to measure and looks good. Tracked in the rejected table with
all numbers attached.

### The semantic index card now tells the truth, and a 20–30 minute job is predictable and cancellable

The card previously reported only `available / indexed / total`. So **every user who
upgraded saw "Ready 44856/44856"** — and `indexed === total` is exactly the case that
**hides** "the row count is full but the encoding pipeline changed", while
`vector_search` is in fact **refusing to search** because the fingerprint does not
match. **The card claimed readiness while the feature was off.**

| | Before | Now |
|---|---|---|
| State display | two states (ready / unavailable); a stale index was invisible | **four states** (ready / not indexed / stale index / model unavailable), each with its own wording and tone |
| 20–30 minute progress | only `1234/44856`, which **conveys no duration at all** | shows an **ETA** (and gives **none** when the sample is too small, rather than a wild number) |
| Cancel | none | **yes** (checked at the top of each batch, so at most one more batch, about 3–5 seconds) |
| After cancelling | — | cancelling is **not a failure**: vectors already written are valid, so no red error is shown |

The ETA is a pure function (it takes `elapsed_secs`, not `Instant`, so its self-test
needs no `sleep`). **No ETA before the first batch**: 128 rows takes a few seconds on
CPU, and linear extrapolation would then say "3 seconds left" — the exact opposite of
the real 20–30 minutes, which is worse than saying nothing.

### The card above was still lying, in a fifth case

After turning the card into four states, **the new predicate itself was never
reviewed** — and it missed exactly the case created by the cancel button added in the
same change.

`Ready` checked only "fingerprint matches" and **never counted rows**, while the
fingerprint was written **unconditionally** after the loop — including on the path
that `break`s out on cancel. Together: cancel a build at 5,000/44,856 and the card
shows **green "Ready"** while the semantic layer covers 11% of the library. A Chinese
query still returns nothing, and the zero-result hint then says "the semantic layer
**is ready** but nothing matched" — **which is the wrong reason**.

Worse, this is not only cosmetic: `vector_search` **reuses the same function** to
decide whether to degrade, so the half-built vectors are **not rejected** and take
part in retrieval as if they were complete.

The cause is not two small slips: **one fact was judged independently by two functions
with different criteria.** `should_skip_index` requires "row count sufficient **and**
fingerprint matching"; the state predicate required only the latter. One says
"rebuild", the other says "ready".

The fix is two independent lines of defence:

- The predicate checks **row coverage first**: `n < total` yields a new fifth state
  `partial_index` ("index incomplete"). It must run before the fingerprint check,
  because "not finished" is more upstream than "wrong scheme".
- **No fingerprint is written on cancel** — a fingerprint means "this scheme is fully
  indexed", and writing one vouches for a build that never finished.

Half-built indexes were deliberately **not** folded into "stale index": that state's
reason ("not produced by the current encoding scheme") does not hold for a partial
build, whose real risk is "semantic search looks like it is working while only a few
entries are covered". **Two situations with different reasons must not share one state.**

The same round also removed three **unrecoverable dead states** (not flickers):

| Defect | Consequence |
|---|---|
| The running flag was cleared **after** broadcasting done | A window exists where the card stays "indexing" forever, and done is never sent a second time ⇒ the button stays disabled permanently |
| "Cancelling…" was only reset by the done event | If the event never arrives the button stays disabled with no way out |
| "Finished" relied entirely on one status round-trip whose failure was silently swallowed | If the refresh fails the card keeps claiming "indexing" |

"**Order is a contract, not a style choice**": state and announcement must have a
deterministic order, otherwise the two drift, and what they drift into is a dead state
rather than a flicker. The invariant now asserted is "**receiving done ⇒ running is
already false**".

Also removed a stretch of **dead code**: "new data not indexed" was gated on
`state === 'ready' && indexed < total`, and `ready` now implies `indexed >= total` by
definition ⇒ unreachable. It and its orphaned i18n key are gone (net key count
unchanged).

Finally, a third instance of "**a comment describing something the code does not do**":
both the state predicate and its call site claimed "reads the DB only, does not load
the model" — **which is not true**. Deciding `Ready`/`StaleIndex` needs the model
fingerprint, and the global encoder is a lazily-initialised `OnceLock`, so **the first
call really does read 91MB of weights** (about 1–2 seconds). The result is unaffected
(it is only slower once), but those two comments would lead someone to assume this path
is always free.

### Semantic search now defaults to off, behind an explicit switch (P-7 option ②)

The previous rounds made the semantic layer **correct**, but it was **still on by
default**. What the measurements support: exact identifiers — the dominant use in an
IDE — go through the keyword layer at top-1 **3/3** for free, while the vector layer
is top-1 1/15 with a median rank above 20, and costs a **178.9MB** vector table
(measured on the seed database: 91.9MB of net data, but SQLite page occupancy
nearly doubles it), 91MB shipped with the installer, and a 20–30 minute first build.
The **3/15 it does win on
Chinese queries is its only value** (the keyword layer returns nothing at all), so it
is not removed — it simply **does not pay that cost unless asked**.

The switch lives in the `settings` table and has three hard constraints:

- **Defaults to off** (fail-closed): a missing or unrecognised value counts as off.
  The reverse would make every upgrade enable it, which is exactly what ② removes.
- **The state check short-circuits first**: `semantic_layer_state`,
  `vector_search`, and — importantly — `api_kb_embed_status`, which **runs as soon as
  the page opens**, all check the switch before touching the vector table and only
  load the model last. With the switch off, **those 91MB of weights are never read**
  even once. Calling `global_embedder()` unconditionally in the status call would have
  made ② a decoration.
- **"Rebuild index" has a precondition**: with the switch off the backend refuses to
  build (otherwise a 20–30 minute build could be triggered for nothing, and the result
  would not even be used), and the frontend button is disabled to match.

A sixth state `disabled` was added: with the switch off the index may well be
complete, so reporting `ready` would be a lie (the feature is off) and reporting
`not_indexed` would be a different lie (it would send the user to build an index that
already exists). This state answers "**why we are not doing it**"; the other five
answer "**why it cannot be done**". Its badge is neutral grey — off is a deliberate
choice, not a fault, and a warning colour would read like a breakage.

**Negative verification caught a fake-green self-test of my own**: the first test for
the switch only asserted "the result is `Disabled`". Injecting a mutation that moved
the switch check to *after* the table read left the suite **fully green** — because
**behaviour and benefit are different things**: with the check moved later it still
returns `Disabled`, it just performs a few extra COUNTs. **Asserting a return value
cannot detect an early return.** The fix was a distinguishable check: create only the
`settings` table and deliberately omit the vector tables, so an early return yields
`Disabled` while a late one yields `NotIndexed` — "the table is missing" becomes a
probe for "was the table read at all". M1 then went red immediately. A second mutation
(M3) guards fail-closed, and a reverse check (M2, wording only must stay green) proves
the suite is not failing indiscriminately.

### Reviewing the switch: the same mistake I had just criticised, on the same card

The optimistic update was not reviewed after it was written. A reverse review found
two defects.

**① The switch and the state disagreed.** The optimistic update changes `enabled`
but not `state`, and the two are coupled (`state` is computed *given* `enabled`).
So the card simultaneously showed "switch: on" and "badge: off", **for one to two
seconds** — and that is not network latency: the status call that follows an enable
really does load 91MB of weights.

The **shape** is the same as "the card claimed readiness while the feature was off":
a conclusion is shown during a window where the state is still undetermined. Here
the two controls simply disagree instead of the backend lying.

### The fix above treated the symptom; this round removes the cause

Rendering no badge was **hiding** the contradiction. The cause was that the optimistic
update set `enabled` without being able to compute `state` — whether `state` becomes
`not_indexed` or `ready` depends on whether the backend has to load the model, and
**the frontend cannot know that in advance**. So "changed A but not B" was not a
mistake; it was a guaranteed consequence of that approach.

**Rule: optimistic updates only apply to fields whose true value the client can
determine locally.**

| Field | Can the client determine it? | Optimistic? |
|---|---|---|
| `running: false` (on receiving `done`) | **Yes** — the event *is* the evidence that the task ended | ✅ |
| `enabled: true` (on clicking the switch) | **No** — what `state` becomes afterwards is unknown | ❌ |

So the unjustified optimistic update is removed: the whole status is replaced only
after the response arrives, so `enabled` and `state` always come from **the same
response** and cannot disagree **structurally**. The cost is a button that spins for
one to two seconds (that is the model load — real and unavoidable), announced as
"Working…".

> **When a rule keeps causing problems and is hard to guard, first ask "can the
> offending shape be made to not exist", not "how can it be detected".** The earlier
> round spent a whole section on which predicate should guard it; the right move was
> to delete the cause — **zero maintenance, and breaking it becomes impossible**.

The badge guard added in the earlier round is kept, but its comment now says
**"redundant insurance, not a required mechanism"** — redundant things must be marked
as redundant, otherwise they block whoever later wants to fix the actual cause.

This round's predicate ("does an optimistic update touch `enabled`?") is statically
decidable and needs no JSX parsing, and was verified in the right order:
**first confirm it produces non-empty output on the real source** (which rules out
"always green"), then inject the old shape to confirm it goes red, then restore and
compare sha256. 6/6.

**② "Off by default" had no explanation.** The card said only "Off". The real
explanation appears only in search results, i.e. the user has to trip over it first
(a Chinese query returning nothing). Defaulting a feature to off is a **behaviour
change**, and one that is not announced becomes a support ticket. A one-line
explanation is now shown at the bottom of the card, **only while it is off**.

Together these give one rule: **whenever a capability or an optimistic update is
added, ask what the interface is saying during the window where the state is
undetermined.** This is by far the most frequent defect shape in this project — an
order of magnitude above a forgotten null check, because **it looks fine**.

### P-7 decision: ship ②, defer ①

P-7 had been sitting in the ledger as "awaiting a decision" with four options
(① switch models / ② default off behind a switch / ③ remove entirely /
④ repurpose as a size optimisation). All four are product judgements a gate
cannot make. Now that ② has landed, the entry is **closed**.

Every reason is a measurement, not a preference:

- **The dominant IDE use is already served**: exact identifiers go through the
  keyword layer at top-1 **3/3** for free; the vector layer is top-1 1/15.
- **② has already been paid for**: the fingerprint, partial-index refusal and
  short-circuit problems are all fixed. **① starts over from zero** — a new model
  means re-encoding all 44,856 rows, buying the model again, and passing the
  licence gate again.
- **The 3/15 it does win on Chinese is available on demand**: the switch turns it
  on; it does not have to ship enabled.
- **Stripping the seed vectors saves 178.9MB, while ① moves the other way**: it
  puts the vectors back into the installer and adds another model on top.

**Conditions to revisit ① (decidable; all three must hold)**:

1. A **real query** exists that the keyword layer scores 0 hits on while the
   semantic layer has the target in its top 10;
2. **≥3 of ≥15 real search terms** fall into that class;
3. A multilingual / code embedding model under **BSD-3, MIT or Apache-2** is
   available to vendor.

Also recorded is one **rejected** restart condition, because it looked the most
convincing of them all: "watch user feedback after release". But
`usage_events_enabled` is **off by default** (a missing key counts as off) and
the telemetry `EVENT_NAMES` allowlist contains **no search events at all** — so
**there is no data to watch**. A criterion that cannot be evaluated must not go
into the ledger, or six months later someone will read it as "someone already
measured this".

### Release prerequisite

The vectors inside the seed knowledge base
(`src-tauri/resources/seed/knowledge.db`) were produced by **a different machine under a
different encoding scheme**, and `api_docs_meta` has **no fingerprint key** — so after
import they are always judged `stale_index` and refused. **Not a single row is ever
usable**, and semantic search now defaults to off, which makes them a pure loss.

Measured: `DROP TABLE api_docs_embeddings` + `VACUUM` shrinks the seed from
**272.6 MB to 93.7 MB** (the vector table occupies **178.9 MB, 65.6%**).
> `44,856 × 2048B = 91.9MB` is the **net data**, not the disk footprint — SQLite page
> fragmentation and overflow pages nearly double it. The "91.8MB" figure in earlier
> documents was wrong and has been corrected everywhere.

**How to run it**: `python scripts/strip-seed-bloat.py --apply`
(reports only by default; `--apply` takes a backup first and rolls back if the
self-check fails. **`VACUUM` is mandatory** — `DROP TABLE` alone does not shrink the
file, so omitting it yields the false conclusion "nothing was saved").
The seed lives under `.gitignore`, so it is **not version-controlled**: this is a
release prerequisite rather than a code change.

**Verified end to end**: real stripped artifact → the same 85 production migrations →
`import_into` ⇒ 59,102 rows imported, `api_docs` 44,856, vectors **0**, no errors ⇒
**no code change is required**.

### Verification

Self-tests **net +10** (2 removed because they encoded the now-retracted contract,
12 added) · `cargo test` **1227 passed / 0 failed / 10 ignored** (baseline 1213) ·
`embedding` module self-tests **22** (+1: switch short-circuit / fail-closed) ·
clippy **no new warnings** · clean compile with the `embedding` feature ·
`tsc` clean · gates **36/36** · negative verification **21/21** (12 + 9) ·
IPC **319 → 321** (+2 switch commands) · i18n zh/en in sync.

This round's negative verification **checked both directions**: removing the row-count
criterion must turn the test red, **and it must be that specific test** that goes red
(otherwise "the gate works" and "it fails indiscriminately" are indistinguishable);
changing only the hint wording must stay green (the reverse proof).

**Negative verification caught a fake-green self-test of my own**: the
"ties are reproducible" test's input contained **no ties at all** (failing to
construct a tie is silent and green), so it still passed after removing the
tie-break key. The fix was to construct the tie correctly and to **assert that the
tie actually exists first**. A companion test asserting tie *order* only failed 50%
of the time, so the two were merged — **a test that only catches a mutation half the
time is itself a flake hazard**.

### Wrap-up: after signing off, two more findings in my own output (2026-10-08)

The previous batch's output was a **decision**. Re-checking the ledger afterwards found
two problems — **one of which I had written in the previous batch, the other far older
and considerably worse.**

#### 1. Signing off left **two `### P-6` headings, the older one with nothing under it

```markdown
### P-6 语义检索要不要**版本感知**（第二十三批提出，**数据已备好，等拍板**）

### P-6 语义检索要不要**版本感知**（第二十三批提出 → **第二十四批撤回重开**）

> **第二十四批结论：这条的前提被推翻了，原拍板已作废，不要按它实现。**
```

When the entry was reopened, the action was to **add a new heading** carrying the new
status, and the **old heading stayed where it was with nothing under it**.

1. The ledger header declares "**6** product decisions pending" while the file contains
   **7** `### P-` headings ⇒ **the stated number and the fact disagree, and nothing
   anywhere goes red**;
2. Someone scanning the ledger stops at "awaiting a decision" and **never reaches
   "the original decision is void — do not implement it"** — and that half is the
   dangerous part.

**An empty heading is not a formatting nit; it truncates a warning.** A ledger is
valuable because readers act on it, and **readers do not read twice**: the heading is
the entire basis for deciding whether to open the section. A heading that says
"awaiting a decision" tells the reader this is settled — just settled the wrong way.

#### 2. New rule "`##`/`###` must have a body", mutation-verified 5/5

What is judged is the **structure** (is there a body up to the next heading at the
same or a higher level), **not "duplicate heading"** — the latter lets "two headings
of the same name, each with a body" pass, which is just as bad.

| Boundary | Cost of getting it wrong | Mutation result (real corpus + synthetic) |
|---|---|---|
| A `####` sub-heading is not a body | becomes "empty if the next line is a heading" | **63 false positives on the real corpus** |
| `##` inside a fenced code block is not a heading | any pasted command containing `#` becomes a section | **4 false positives on the real corpus** + self-test goes red |
| H1 is excluded (`#` followed by `##` is normal structure) | CHANGELOG structure judged empty wholesale | real corpus **0**, **carried by the synthetic case only** |

The middle row is this batch's prettiest piece of **self-reference**: the four false
positives that appear once fence awareness is removed are **all example blocks that
document this very defect** — one each in `CHANGELOG.md`, `CHANGELOG.en.md`,
`docs/TECH_DEBT.md` and `docs/UI_REFINEMENT_PLAN.md`, each inside a ` ```markdown `
fence containing those two `### P-6` lines. **The habitual way of writing this defect
into a document is exactly the shape this rule is meant to catch.**

The third row is **labelled honestly as "carried by the synthetic case only"**: every
H1 in the current documents happens to have a body, so it guards nothing on the real
corpus today — it guards against the shape **appearing later**. Not described as
"also held by the real corpus".

#### Side note: **when an ad-hoc command reports "nothing changed", suspect the command first**

Those 4 false positives were **not detected the first time**. The mutation was run
as a PowerShell `python -c @"…"@`, and it printed "fence awareness removed: 0 items" —
while removing fence awareness **obviously cannot** have no effect. The real cause:
the `\"```\"` escaping inside the here-string **did not match**, so `.replace()` was a
**no-op** and both versions were byte-identical — hence 0 in both.

> **"The mutation did nothing" has two explanations: the guard is decorative, or my
> mutation never applied.** The second is far more common and far more dangerous — it
> leads to writing a **load-bearing** guard up as "measured: not load-bearing" and then
> deleting it. **Rule: an ad-hoc mutation must first assert that it actually changed
> something.** This is now a hard check in
> `scripts/verify-empty-sections-rule.py`: the anchor must occur exactly once and
> `mutated != original`, otherwise it fails outright — and it reports **both** the
> real corpus and the synthetic cases, because reporting only one of them yields a
> skewed conclusion.

`check-docs.py` self-tests **11 → 15**.

#### 3. Following that thread: **the gate has been red on Chinese Windows all along**

While updating the ledger's gate table (`check-docs.py` 11 → 15),
`check-self-test-counts.py` reported 6 failures. The first reaction was "I mistyped
the number" — but **`docs` was not among the failures**, so 15 had been accepted.
The real cause:

```python
r = subprocess.run([...], capture_output=True, cwd=REPO)          # no env
return r.returncode, (r.stdout + r.stderr).decode("utf-8", "replace")
```

The child gate's stdout is a **pipe**, and Python writes it using the locale —
**GBK** on Chinese Windows; the parent decodes as **UTF-8** ⇒ mojibake ⇒ the count
cannot be parsed ⇒ **6 gates go red at once**. A real A/B:

| Environment | Exit code |
|---|---:|
| `PYTHONIOENCODING` unset | **1** (false red, 6 items) |
| `PYTHONIOENCODING=utf-8` | **0** |

**Why nobody noticed**: CI is Linux/UTF-8 and always green; locally most people run it
through `deveco-verify-all.py`, and **that harness injects the variable into its
children** — its own comment reads "so an encoding problem can never masquerade as a
broken gate". **The same patch was applied to the harness, not to the gate.**

Fix: inject `env={**os.environ, "PYTHONIOENCODING": "utf-8"}` into the spawn.
Mutation check: removing that line drops this machine straight back to exit 1;
sha256 identical after restore. Self-tests **14 → 15** (one new end-to-end real run;
⚠️ that case **only has discriminating power on a non-UTF-8 locale**, stated as such).

> **Fixing "where the symptom appears" is not enough — ask "what made it appear".**
> Here the symptom is in the parse result; the cause is a spawn without an environment.
> And: **a defect that only reproduces on one machine most likely has its cause in that
> machine's environment.** `CI green` only means "no problem on the kind of machine CI is".

#### 4. Clearing numbers that were guaranteed to drift, plus three corrupted bytes

The Q-08 evidence lines in `docs/ROADMAP.md` / `ROADMAP.en.md` carry 8 concrete
numbers (tools 201 / migrations 77 / IPC 298 / …). Measured, **all eight are stale**
(real values 204 / 85 / 321 / 41 / 64 / 19 / 58 / 36), and `check-docs.py`
**does not check numbers inside ROADMAP**.

The handling is not "update them to the new values" but **delete the numbers and keep
only the category names** — updating merely postpones the same drift by one batch.

> **A number that no gate verifies should not appear in the form of a fact.**
> Either give it a criterion, or demote it to descriptive wording. The middle ground
> (an example that looks like a fact) is the worst option.

Also repaired three **corrupted UTF-8 Chinese characters** in
`docs/UI_REFINEMENT_PLAN.md` (`存档` / `发现` / `需要用正则`). **The Edit tool cannot
help here**: that character is not a stand-in produced by a failed decode but a **literal
`U+FFFD` character (`EF BF BD`)**, the file itself is **perfectly valid UTF-8 with not a
single invalid byte**, so **Edit can match and replace it directly**. The one time I
believed "Edit cannot match it" was because I had written the **same string into both
`old_string` and `new_string`**. **Verify "is the file actually valid UTF-8" before
concluding that a tool cannot do it.**

#### Verification for this batch

`check-docs.py` self-tests **15** (+4) · `check-self-test-counts.py` self-tests
**15** (+1) · mutation injection **5/5** · encoding-fix mutation **3/3** ·
gates **36/36**.

**Zero product-code changes** — two gate scripts and four documents. But one of those
affects **every Windows developer**: it left that gate permanently failing on their
own machine, and **a failing gate is the kind that gets turned off**.

### Deleting a write-only column: 19.7 MB saved, with zero functional change

The ledger recorded: "`api_details.members` and `api_members` differ on 9 rows;
**deleting `members` would leave those pages missing 49 members**". Going to find out
*where the page's member count actually comes from* showed **that premise was wrong** —
**no read path goes through that column at all**:

| Read at | Actually reads |
|---|---|
| The member-count column in the knowledge-base list | `SELECT COUNT(*) FROM api_members` |
| The member list on the detail page | `FROM api_members WHERE detail_slug=?1` |
| Sub-items matched by Agent search | `FROM api_members` |
| Frontend rendering | the backend responses from the three above |
| Diagnostic bundle export | **does not contain the `api_details` table at all** |

The manual "create detail" path is even more telling: it writes a literal `'[]'` into
`members` — **it never wrote any data there at all**.

⇒ Those 9 rows are a **symptom, not a disease**: two writers with different dedup rules,
and because nothing reads it, the divergence **can never surface** — type-check, lint,
tests, build and the size gate are **all green**.

| Item | Value |
|---|---:|
| `api_details` rows / of those `members` non-`'[]'` | 681 / **681 (all)** |
| `SUM(LENGTH(members))` net data | 15.2 MB |
| **Actual disk cost** (copy → DROP COLUMN → VACUUM → compare `page_count`) | **21.9 MB** |
| Recovered by the "VACUUM only, no drop" control | 2.1 MB |
| ⇒ **net gain** | **19.7 MB** |

The previous release was already bitten by this once (91.9 MB net vs **178.9 MB** actual),
so this time the measurement did **not** go through `SUM(LENGTH())`, and it added a
"VACUUM only" control group.

#### What actually blocked it was not the data — it was one `SELECT *`

`services/seed.rs` used `INSERT OR IGNORE INTO main.<t> SELECT * FROM seed.<t>`.
`SELECT *` expands on the **seed** side while the insert proceeds **positionally** ⇒
both sides must have identical column counts *and order*. An old seed database still
carrying that column while the main one no longer has it ⇒ first launch reports
`table main.api_details has N columns but M values were supplied`, inside a transaction
⇒ **the whole import rolls back** ⇒ **a new user's knowledge base comes up empty**.

**Measured first to confirm this was a latent landmine, not an already-firing one**:
comparing the four tables column by column, the migrated main database and the real seed
database have **identical column names and order**.

And **the same trap had already been hit in this very file**: `api_docs_embeddings` was
pulled out and hand-remapped, with a comment explaining why — but that **fixed the
symptom on that one table and left the `SELECT *` cause in place**, so it was free to
reappear on the next table.

The fix is not a hardcoded column list (that expires with the next migration) but a
**runtime intersection** (`shared_columns()`): a new seed missing a column inserts one
fewer; an old seed lacking a newly added column has it skipped.
**Neither side's schema changes can blow this up anymore.**

**End to end**: real seed (**with** the column) → real 86-migration main database
(**without** it) ⇒ import succeeds, `api_docs/api_details/api_members` =
**44,856 / 681 / 13,606**, second import **0** (idempotent). The temporary test was
removed byte-for-byte afterwards (this project adds no tests).

#### The release script now strips a second thing, and is renamed to match

`strip-seed-vectors.py` → **`strip-seed-bloat.py`** (it now strips the vector table *and*
the `members` column, so "vectors" in the old name became a lie). Measured on the factory
seed: **272.6 MB → 74.0 MB (198.6 MB / 73% saved)**; `--self-test` passes **4/4** on a
real copy.

Still **report-only by default**, requires `--apply`, backs up first, and rolls back if
the self-check fails — **that discipline does not relax just because it now strips one
more thing**. A `--vectors-only` escape hatch was added: on SQLite < 3.35 (no
`DROP COLUMN`) it strips only the vector table and **says so**, because the leftover
column affects nothing functionally — it only costs space.

**How to run it**: `python scripts/strip-seed-bloat.py --apply`

#### New gate 16: characters that got written as damage

Another corrupted character surfaced while writing the docs, so: is this a one-off or a
class? **A class**, and no criterion guarded it.

First, the conclusion recorded by the previous release is overturned: "that character is
a stand-in produced by a failed decode, so Edit cannot match it and it can only be
replaced at the byte level". **That is wrong**: what sits at the damaged position is a
**literal `U+FFFD` character (`EF BF BD`)**; the file itself is **perfectly valid, with
not one invalid byte**, and **Edit can match and replace it directly**. The one time I
believed "Edit cannot match it", the reason was that **I had written the same string into
both `old_string` and `new_string`**.

| | Behaviour |
|---|---|
| File size / line numbers | **unchanged** |
| `tsc` / `eslint` / `cargo` / `vitest` | **all green** |
| `decode("utf-8")` | **does not raise** |
| Actual consequence | one character is gone, usually from **the most semantically load-bearing position** |

New gate `scripts/check-encoding-damage.py` flags any line containing a literal `U+FFFD`;
self-tests **6/6**. One scan surfaced **8 historical corruptions** (docs, scripts and
source comments) — all fixed.

Two boundaries, both found by measurement: **deliberate use must be allowed**
(`MISSING = "<U+FFFD>缺键"` in `check-i18n-wording.py` is a **sentinel value** whose whole
job is to be unable to collide with a real translation — "fixing" it would break a working
mechanism); and **never write that character literally in docs** — always write the six
letters `U+FFFD`. Three explanatory notes I wrote myself were consequently flagged as
damage: **the gate classified its own documentation as a defect**, which only a convention
can prevent — not a bigger allowlist.

The first version used `rglob` and took **56 seconds** per run (`rglob` filters *after*
walking, so the vendored `references/` tree was traversed in full). Switching to
`os.walk` with in-traversal pruning brought it to **1 second**. The mutation was **re-run
after the change**: clean baseline → injection goes red and names the file → sha256
identical after restore → green again, **5/5**.

#### Verification for this batch

Migrations **85 → 86** · `cargo build` clean · `cargo test --lib services::seed` **5/5**
(including the temporary end-to-end, since removed) ·
`strip-seed-bloat.py --self-test` **4/4** · `check-encoding-damage.py` self-tests **6/6**
+ mutation **5/5** · document-drift gate and self-test-count gate both green ·
gates **36/36**.

**This is the project's first batch that deletes code**: the net effect is another
**19.7 MB** off the seed database, with **zero functional change** for the user — because
nothing ever read that column.

---

## Unreleased — Batch 23: the sample size of the census is itself a finding (2026-10-02)

This release does not change the scale: the `TOOL_SPECS` to **204**, the migration count reaches **85**.

This batch **changes no product behaviour** — one comment and one test that pins it.
The census process itself, however, produced the hardest data of this batch.

### First, a retraction: the entire database holds 1 conversation

| Table | Rows |
|---|---:|
| `conversations` | **1** |
| `tool_runs` | 25 |
| `api_docs_embeddings` | 44,856 |

The 273.8MB is almost entirely factory seed data; actual usage traces are
**1 conversation and 27 messages**. Every "historical statistic" from the previous
two batches must therefore be downgraded — "100% of `contribution_state` is
`no_direct_acceptance_evidence`", "the guard fires on 64% of calls", "placeholder
mistakes 3/3" — **all of them have their denominator inside the same conversation**.

**They are still real defects, but nothing shows they are systemic.**

> Print the denominator before quoting any ratio. "3/3 = 100%" and "3/1,000,000"
> look identical in wording and differ by six orders of magnitude in meaning.
> **Sample size is not a footnote; it is part of the conclusion.**

### The conclusion that does have a denominator: 72.2% of vectors are byte-identical

| Item | Value |
|---|---:|
| Vector rows | 44,856 |
| **Unique vectors** (sha256 dedup) | **18,506** |
| Rows in duplicate groups | **32,381 (72.2%)** |
| Duplicate groups / largest | 6,031 / **538** |

Root cause: `services/embedding.rs::doc_text` builds the encoded text from only four
fields; `version_label` / `dts_file` / `api_level` **do not participate**, while the
same declaration repeats across dozens of versions. Over 400 random queries,
**67.8% have a top-1/top-2 cosine gap of exactly 0**.

**"Which version of an API does the user get when they search?" has no answer today** —
they get an arbitrary member of the tied group.

### Quantisation rejected, with numbers (saving 65MB is tempting, and wrong)

Self-retrieval on the real vectors: int8 saves 75.0% and f16 50.0% in size, but
top-1 agreement with f32 is only **46.7% / 45.3%**. That looks unacceptable —
**but the f16 round-trip error is only `1e-4`, while adding `σ=1e-3` noise to the
query vector changes the #1 result `0/150` of the time.**

Put together, the conclusion changes: **the tied items were exactly equal to begin
with**; quantisation merely shuffles them. So the fix moves from "shrink the payload"
to "make tied items distinguishable" — which is a product decision (P-6), not debt.
**Recorded in the "rejected" table with every number**, so the next batch need not
redo the experiment over 44,856 vectors.

### Fixed: a comment describing something the code does not do

`doc_text`'s comment said "+ change notes", yet the function has **no `change_type`
parameter**. `cargo test` / `tsc` / clippy are all green — because a comment is not
code. And that is exactly what **hid the finding above**: `version_label` is likewise
absent from the encoded text.

**The fix is not just editing the comment** (that only means "right this time").
A new self-test pins "which four fields, in what order, how empty fields are
placeheld" with **exact string equality**; adding, removing or reordering any field
turns it red.

### Another one: `members` is not a redundant copy, it is a sync inconsistency

15.2MB of `api_details.members` matches `api_members` on 672/681 rows, but **9 rows
disagree and the JSON side always has more** (`js-apis-rpc` 265 vs 216, 49 missing
from the table). **It cannot be handled as "drop the redundant column"** — doing that
would leave those pages short of 49 members. Which side is authoritative has to be
decided first (logged as T-12).

### Verification

+1 self-test · `cargo test` **1213 passed / 0 failed / 10 ignored** (baseline 1212) ·
clippy **57/57 with no new warnings** · `tsc` clean · gates **36/36**.

---

## Unreleased — Batch 22: overturning two of my own entries from the previous batch (2026-10-02)

This release does not change the scale: the `TOOL_SPECS` to **204**, the migration count reaches **85**.

The previous batch closed with three ledger entries (T-8 / T-9 / T-10). This batch went
to act on them and found that **two of the three diagnoses were wrong** — which is
where the real value of this batch lies.

### "The arguments were discarded" was false: the audit layer faithfully recorded what the model sent

The previous batch saw `tool_runs.input_json` holding the literal string `"JSON参数"`
and recorded it as "failed calls discard their arguments — a forensic black hole".
Tracing it all the way to the write site:

`chat.rs:2556` → `tool_audit_payload` → tries `serde_json::from_str` first and only
falls back to `redact_text` (**verbatim**) — while the upstream `parse_tool_calls`
performs **no placeholder substitution whatsoever**.

**So `input_json` is exactly the string the model actually sent. Nothing was lost.**

> "Believed to be discarded" and "faithfully recorded" are identical at the exit code.
> Judgement-style reasoning flips here: a datum that *looks* like a bug can only be
> classified by **tracing to the write site and asking who writes it**.

The real defect lies elsewhere, and is **more worth fixing than the original claim**:
the system prompt writes the format as `【TOOL|tool|JSON参数】` in 5 places, and the
model **sent `JSON参数` verbatim as the arguments**; meanwhile the validator reported
`parse error at line 1 column 1` — **saying a row/column to "you sent a placeholder"
says nothing at all**. The consequence is worse than "wrong arguments":
**once the error message is useless, the model's self-diagnosis is necessarily wrong
too** (it concluded, in the observed run, "I wrote multi-line JSON / backslash escaping").

A placeholder now returns `placeholder_argument`, naming it explicitly ("that is the
placeholder from the prompt") with a copyable JSON example. **Parameter text is never
echoed back** (to avoid re-spreading sensitive data).

### Also fixed: path error messages no longer leak `\\?\`

The path `resolve_in_roots` returns was already normalised; **only the error message**
used a bare `display()`, so `\\?\I:\SNS\harmony-sns\entry\module.json5` landed in the
model's context verbatim. The same file already had `normalize_path` (with three
comments explaining why it is mandatory) — this one call site simply missed it.

### The session-metadata entry splits in two: half is not a defect, half lacks prerequisites

- **`conversations.model_id` is not a defect.** It carries **user-binding** semantics;
  NULL correctly means "the user never bound one", and the model actually used is
  **already recorded per message** in `messages.model`. Writing a "last used" value
  **would overwrite the user's explicit binding**.
- **`system_prompt_version` is a genuinely dead column** (read in 6 places, written in
  0), but **it must not be filled in ad hoc**: there is no version constant anywhere in
  the codebase, and the system prompt has no single assembly point. Inventing a version
  number now means **inventing a digit with no process behind it** — it would look
  authoritative yet stand for no real run, **which is worse than NULL**. The three
  questions that must be answered first are written into the ledger.

### Verification

- **+5 self-tests** (3 protocol, 2 tools), `cargo test` **1212 passed / 0 failed / 10 ignored**,
  `tsc` clean, clippy **57/57 with no new warnings**, gates **36/36**
- **Negative verification 6/6**, both ends checked for both defects
- One of those self-tests **binds the placeholder detector and the prompt's actual
  wording in both directions** — "the prompt changed but the word list did not" would
  silently drop that shape back to "line 1 column 1".
  **Comment conventions never fail, so a test has to guard this.**

> **The real output of this batch is not "two more bugs fixed" — it is that a ledger
> decays too.** A confidently written *wrong* entry is more dangerous than no entry,
> because the next batch **skips suspicion** and cites it. So ledger entries have to be
> able to carry "this one was later overturned, and why". T-9 / T-10 keep their
> corrections instead of being deleted, for exactly that reason.

---

## Unreleased — Batch 21: auditing the code through one of our own recorded sessions (2026-10-02)

This release does not change the scale: the `TOOL_SPECS` to **204**, the migration count reaches **85**.

This batch is not a UI batch. It started from reviewing a session the app recorded
**about itself** (`047424ea`, project `I:\SNS\harmony-sns`, whose entire question was
the 9 characters "现在项目什么情况？"). The conclusion: **the worst problem was not the
model — it was the execution kernel.**

### The quality score reported 100 on zero criteria, and rendered it green

That session produced `{"score":100,"acceptance_passed":true,"evidence_criteria":0}`,
and the Cost page's quality-score tile turned it **green** — while all 21 of its tool
calls carried `contribution_state=no_direct_acceptance_evidence`. **It was never
judged even once.**

Four layers, each individually "correct":

1. The goal contains none of change/build/test/deploy/commit/push ⇒ the contract
   generates **0 criteria**
2. `passed: blockers.is_empty()` over an **empty** set ⇒ always `true`
3. The score starts at 100 and only subtracts for `passed` / `exhausted` /
   `remediation` — **there is no term for "criteria is empty"** ⇒ `score = 100`
4. It is accumulated unconditionally into `average_quality_score`, and
   `scoreTone(>=90)` paints it green

The deepest layer: `false_completion_count`, the one metric dedicated to catching
false greens, tests for `passed == Some(false)` — and an unjudged run has
`passed == Some(true)`, so it **can structurally never reach that counter**. The
detector for false greens cannot see empty criteria, because to it empty criteria
and a real pass are **indistinguishable**.

**The fix is deliberately not flipping `passed` to false** — that would make **every
pure Q&A conversation unable to ever finish**, and "pure Q&A: give an accurate and
complete answer" is already the contract's own fallback for empty criteria. So the
kernel now **records "never judged" explicitly**: `RunQualitySnapshot` gains
`criteria_declared` / `unscored` (`schema_version` 1 → 2), an unjudged run's `score`
is pinned to 0 (**0 so that a downstream consumer who forgets to check the flag
displays "untrustworthy" rather than "perfect"**), the dashboard **excludes** those
runs from the average, and the Cost page shows `—` rather than `0.0` when nothing
was ever judged (the latter reads as "quality is terrible", the opposite of the
truth).

**No criterion was fabricated for pure Q&A** — inventing "the answer cites tool
evidence" would be building a tautology.

### `todo_write` silently dropped state changes: the documented usage was the broken one

The model marked 5 items `done`; the tool replied "0 done, 0 in progress, 9 pending"
— it said it would change, the tool said nothing changed, and the tool's own tally
said everything was still pending.

Two pieces of code contradict each other: `agent::todo::render_hint` injects a system
hint every turn telling the model to "mark items done with `todo_write`
(**merge=true, changed items only**)" — i.e. `{id, status}` with **no `content`** —
while the parser in `tools/mod.rs` **discards anything without content**.
**The documented correct usage was exactly the silently failing usage.**

Parsing is now a pure function: in merge mode `content` is inherited from the current
list by `id`; only genuinely unknown items are dropped, and dropped ids are now
**reported back to the model** — silent dropping is the cause of this defect.

### Three more things investigated but deliberately **not** fixed

Registered in `docs/TECH_DEBT.md` (T-8/T-9/T-10) so the next batch need not
rediscover them:

| | measured |
|---|---|
| **The stall guard only warns, it never escalates** | It fired **16 times** in one run (calls 10 → 25); the model changed strategy **zero** times |
| **Budget is denominated in rounds while cost is incurred in bytes** | A 9-character question burned **209825 input tokens / 167 s**; tools accounted for only **2.2%** of wall time |
| **Failed calls discard their arguments** | 3 schema failures stored the literal string `"JSON参数"` in `input_json` — a forensic black hole |

### Verification

- **+6 self-tests** (2 quality-score, 4 todo), full `cargo test` green, `tsc` clean,
  clippy **57/57 with no new warnings**
- **Negative verification 6/6**, both ends checked for both defects
- The negative verification surfaced a **third** same-shaped trap: `shutil.copy2`
  **preserves mtime**, so after restoring, the source looked *older* than the
  injected build and **cargo skipped recompilation** — byte-level restore verified,
  yet the injected binary was still what ran. Fixed with `os.utime(path, None)`:
  **byte-level clean ≠ behaviour-level clean**

---

## Unreleased — Batch 20: splitting the 2351-line device panels (2026-10-01)

This release does not change the scale: the `TOOL_SPECS` to **204**, the migration count reaches **85**.

The ledger entry "the device panels' internals have never been surveyed" sat unanswered
from §6.14 until this batch. Filling it in immediately surfaced something more important,
which then got fixed.

### Writing `lazy()` three times does not create three chunks

The Home page lazy-loads three panels, like this:

```ts
const DevicesPanel = lazy(() => import('.../devicePanels').then((m) => ({ default: m.DevicesPanel })))
const AnalyzePanel  = lazy(() => import('.../devicePanels').then((m) => ({ default: m.AnalyzePanel })))
const SymbolsPanel  = lazy(() => import('.../devicePanels').then((m) => ({ default: m.SymbolsPanel })))
```

All three point at **the same module**. And the bundler emits only **one** chunk for
repeated `import()` of the same module — the output was a single 63.6KB
`devicePanels-*.js`. So those three "lazy" declarations only deferred **one big blob**:
**opening the device panel downloaded the analysis panel and the symbols panel too.**

**Code splitting happens at module boundaries, not at call sites.**

### Survey result: the three panels have zero cross-references

| Inside the old file | Lines | Depends on |
|---|---:|---|
| Devices panel | 954 | the perf card and its own detail-row formatter only |
| Project analysis panel | 781 | fully self-contained |
| Symbols panel | 460 | its own kind table and colour helper only |
| Perf card | 47 | **nothing at all** (pure props + inline SVG) |

That answer was **counted**, not guessed — and it is itself the reason it was worth doing:
zero cross-references means the move **required changing not a single line of logic**.

### What the split bought

| | Before | After (bytes fetched when opening that panel) |
|---|---:|---|
| Devices | 63.6KB | **28.0KB** (-56%) |
| Project analysis | 63.6KB | **24.4KB** (-62%) |
| Symbols | 63.6KB | **13.0KB** (-80%) |

Chunk count **137 → 140**.

**The entry bundle did not shrink by a single byte** (564.4KB / 575KB) — these panels were
already lazy and never counted against the first-paint budget. What shrank is **the payload
fetched on interaction**. Do not read "split a big file" as "freed first-paint budget".

### A criterion that had been blocking two debts for several batches

Both the device-panels debt and the chat-state debt said "unlocked once we have a
component-level rendering test net". This batch showed that **the criterion conflates
two different things**:

| | Does it change **behaviour**? | Behaviour net needed? |
|---|---|---|
| What a component-level rendering test net catches | extracting a reducer / changing effect deps / changing stream timing | **yes** |
| Moving code verbatim into another file | **no** — the rendered result is unchanged by definition | **no** |

The only ways moving code can go wrong are **not moving it all / moving it into the wrong
file / dropping an import** — and all three are caught dead by a **segment-by-segment
content-equivalence proof**, without rendering anything. This batch's proof: all 6 original
code segments appear **verbatim** in their target files, and the overall line **multiset**
is identical.

**A green type-check only proves the types line up — it cannot prove a single line was not
dropped.** One silently-deleted JSX line can pass everything when there is no rendering
test at all. Hence two criteria that cover both directions: the type-check catches missing
imports, the linter catches extra ones, and the content-equivalence proof catches
"not everything moved".

### Fixes

- Devices panel, device detail-row formatter, project analysis panel, symbols panel and
  perf card are now separate files; `PerfCard` imports nothing
- **No change** to first-paint layout, interaction or visuals (a pure move, proven equivalent)

### New gate #15: lazy imports must actually be lazy

The "three `lazy()` calls pointing at one module" defect described above is **silent** —
type-check, lint, tests, build and the bundle-size gate are **all green**, because
"the same module" is neither a type nor a semantic error; it simply **does not split**.
So if someone merges the panels back into one file, or adds a fourth `lazy()` pointing
back at a shared module, **CI will not react at all** while the user quietly downloads more.

The new gate catches the two cases that are **objectively no-ops**:

1. The lazy target does not resolve to a real file → the panel **never appears**,
   and no existing gate reports anything
2. The lazy target is already reached by a **static** `import` → it is already in the
   first-paint bundle, so `lazy()` saves no bytes and just adds a Suspense layer

Current state: 24 lazy imports (19 pages + 5 panels), plus two occurrences of `lazy(`
that exist **only inside comments** — the gate tells the two apart.

**What this gate deliberately does not check:** "N `lazy()` calls should be N distinct
modules". Several components sharing one lazily-loaded module is a **legitimate** pattern.
And merging the three panels back into a single file is semantically correct — that is a
gap between *intent* and *fact*, which only code review can catch.

---

## Unreleased — Batch 19: closing out the remaining ledger (2026-10-01)

This release does not change the scale: the `TOOL_SPECS` to **204**, the migration count reaches **85**.

This batch finished the last four items on the ledger together: two technical debts
(T-6 / T-7), one content debt (C-3), and one half-doable technical debt (T-4).
**Exactly one user-visible interface change** — see "Website colour" below.

### i18n dead-key cleanup: 83 keys removed, but only after the evidence tool existed

Batch 18 found two keys with no literal reference, and **deliberately did not delete
them**: a literal search cannot prove a key is dead. This batch built the evidence
tool first (`scripts/audit_i18n_refs.py`), with **four criteria, none optional**:

1. Fully-qualified **literal** references
2. **Template literal prefixes** — unenumerated families are covered conservatively
   by prefix (better to miss a candidate than to wrongly declare one dead)
3. **String/variable concatenation** — must be registered, and reconciled with the
   code in **both** directions
4. i18next **plural forms** — `_one`/`_other` get separate protection, because they
   are *never* referenced by a literal

Criterion 3 is the reason this tool exists at all: one extra unregistered
`t(<bare variable>)` makes it **exit 2**. An unconditional "list of dead keys" is
**dangerous** once dynamic key lookup exists — it reports live keys as dead, and
**reporting a valid resource as dead is more misleading than missing one**.

The tool walked into four traps of its own (all written into its docstring):
Tailwind class names containing `0.5` / `1.5` broke the "any dot means an i18n prefix"
criterion and misjudged 248 keys; the single-letter prefix in `a${Date.now()}` is an ID
prefix, not an i18n prefix; ternaries and interpolation objects were misread as dynamic
outlets; and unprotected plural keys would have deleted four keys still in use.

Result: **2339 keys per file → 83 provably dead → 2256 keys, zero candidates**.
After the deletion `tsc` / `vitest` / all 34 acceptance checks stay green — nothing
referenced them. The audit also reports `dangling` (referenced by code, absent from
i18n) = **0**, which is the *positive* evidence that no live key was harmed.

### Contract 6 proved its own worth, in the wild

Re-running the i18n gate after the deletion reported **4 stale explanation entries**:
"关闭", "切换模型", "重试", "默认模型". When batch 18 wrote the "no stale entries"
contract these were still hypothetical; this batch they fired for real.
**Left for the next batch, they would have been four unread lines in a table.**

### Two mislabelled Chinese strings — one of them was *hiding* an English drift

- `cost.retry`: "重试" → "重试次数" (six other keys in this repo use "重试" as an **action**)
- `provider.models`: "默认模型" → "模型"

The second fix immediately **uncovered a real conflict group that it had been masking**:
`provider.models` is a **collection label** (`Models`), while `cost.model` /
`cost.evalModel` are **table column headers** (`Model`) — they are *supposed* to differ.
**A wrong Chinese string is not just "imprecise translation"; it keeps another
group's English drift invisible for as long as it stays there.**

### Website colour: one new token split out (the only interface change this batch)

`check-website.py` gained contracts 12–14: contrast (WCAG computed in pure Python),
focus must be visible, and focus order must not use a positive `tabindex`.
**axe-core was not introduced** — it is MPL-2.0 (file-level copyleft), outside the
licence gate's allow-list; and contrast on a static site is **pure arithmetic**.
Pulling in a file-level copyleft dependency for one square root is wildly
disproportionate to the benefit.

Contract 12 caught a real defect the moment it went live: white text on
`--site-accent` (#2f81f7) reaches only **3.75:1**; and darkening #2f81f7 enough for
white text drops it to 4.06:1 against the dark background, so links fail too.
**When one token has to serve both roles, one of them is guaranteed to fail** — so
`--site-accent-solid` (the surface that carries white text) was split out from
`--site-accent` (the colour drawn on top), and the primary button plus the skip link
now use the former.

### Motion tokens: only the half that is numerically identical

`--app-motion-fast` *is* `0.12s`, so `0.12s → var(--app-motion-fast)` is pure
convergence with **zero visual change**; five declarations were converted. Everything
else was left alone — which bucket `0.15s` belongs to is a **visual decision**, and the
38 `animation` lines (`spin 0.8s` / `shimmer 1.5s`) are **second-scale** loops: swapping
`0.8s` for `--app-motion-slow` (0.25s) would spin the spinner 3× faster.
**New finding:** what is needed is a **second token family** (loop / emphasis), not a
wider version of the existing three.

### A flaky test failure, fixed

Four `services::seed` tests intermittently failed 3/4 (and passed when run alone) with
`table api_docs already exists`. The temp directory name contains the pid, while the
`seed.db` cleanup was written **only at the end** — so as soon as the process is
interrupted that cleanup line never runs, and pids get reused. All four tests now
delete first, *then* create. Negative verification is 19/19 with a real control group:
with the fix removed the run must fail; with it restored it must pass three times running.

---

## Unreleased — i18n wording convergence: "reviewed by hand" becomes "zero unexplained" (2026-10-01)

The last remaining content debt from §6.30 was "review those 49 i18n wording groups".
This batch actually finished it.

This release does not change the scale: the `TOOL_SPECS` to **204**, the migration count reaches **85**.

**The criterion changed: not "same Chinese must mean same English", but
"do these two keys refer to the same thing in the same screen"**

Button vs state, diff legend vs action, panel vs table column, collection vs item —
these **are supposed to differ**. Chinese over-sharing is a **data-organisation**
problem, not an English-wording one; flattening them only puts two identical
labels on one screen pointing at two different numbers.

Three changes in the first draft were **overturned by evidence** once the call sites
were dug out:

- `repro.title` should **not** be shortened — the established pattern for page titles
  across this repo is "short nav, longer page title" (Cost / Cost Tracking,
  MCP / MCP Servers are the same shape)
- `config.telemetry` should **not** become Title Case — it is a settings row label
  (sentence case), while `usage.title` is a page title (Title Case).
  Changing it would contradict the rule unified in batch 16
- `home.detailRam` should **not** become "Memory" — `home.devicePerfMem` is a
  **percentage** PerfCard, `home.detailRam` is a **GB capacity** row. Two different quantities

**chat vs conversation: originally a product decision, the evidence answered it**

The two words sit in the same sentence — `"No conversations yet. Click New Chat to start"` —
while the sidebar header says `Conversations` and the same context menu says
`Pin chat` / `Archive chat` / `Delete chat`. Repo-wide `conversation` : `chat` = 50 : 22.

But most of those 22 `chat` keys are **not drift** and are left alone:
`Send to chat` (verb phrase), `Chat settings`, `in the chat area`,
`neither chat nor tools` — those are the "chat area / chatting" sense, and
"Send to conversation" is ungrammatical.

**36 keys changed**, all by targeted per-key replacement
(**top-level section + leaf key + old value, anchored uniquely inside the section**):

| Segment | Keys | Content |
|---|---:|---|
| A real drift | 6 | Confirm delete? / Deleted / Save failed / success rate / call count / referenced file |
| B chat → conversation | 7 | Conversation menu + reproduction-bundle page |
| C audit family consistency | 3 | Three `home.auditLabel*` keys that lacked an object |
| D ellipsis | 13 | The busy-state semantic cluster's `...` → `…` (U+2026); `sk-...` is literal dots and is kept |
| E second round | 7 | **Groups missed in the first round**: current / switch model / refresh / retry / read failed |

Segment E is the most instructive: after fixing 29 keys, re-running the audit found
7 more real drift. The miss was treating "casing is inconsistent" as a settled
conclusion from a previous batch — which substitutes **an already-false summary for
per-key checking**. Summaries expire; per-key checks do not.

**Gate 13, `check-i18n-wording.py`, grows from 4 contracts to 7**

Batch 16's gate **admitted in its own docstring** that the 49 reworded groups still
slipped through and the list was "reviewed by hand". The new contracts here are:
**every difference must be able to state why it differs.**

| Contract | Content |
|---|---|
| 5 | Every group with differing English must have a written reason in `scripts/i18n-wording-explained.json` |
| 6 | The table must have **no stale entries** (an entry left after a group is fixed pretends it was explained) |
| 7 | Ellipsis must be U+2026; the allowlist only takes literal dots, and **a stale allowlist entry is also red** |

All 38 groups carry a written reason — **0 unexplained, 0 stale**
(`different-slot` 26 / `different-thing` 10 / `dead-key` 1 / `zh-side` 1).

This does **not** contradict batch 16's "rejected: hard-gate the reworded groups" —
what was rejected was **hard-coding a single unified answer**, not gating at all.
Hard-coding forces out the answer that is wrong for its position; requiring a reason
only surfaces "this hasn't been thought through yet".

**Three pitfalls hit along the way**

1. **Two classification implementations each skipping differently**: the audit reported
   50 groups, the gate 49; the difference was a group containing an i18next plural key.
   The root cause was not a miscount — the **gate skipped it, the audit did not**.
   The audit now imports the gate's `classify_groups()`; the numbers no longer diverge
2. **An anchor can silently not exist**: the negative validation injected into
   `'"common.loading": "Loading…"'`, but en.json is nested and the top-level section
   name is on another line, so it matched **0 places** — the injection became a no-op
   while the **exit code stayed red**, which looks like a passing validation
3. **Broken config ≠ broken gate**: when the explained table was malformed the gate
   raised `JSONDecodeError` — the exit code was still red (CI would block) but the
   message named neither the file nor the fix. It is now `ExplainedTableError` with
   **exit 2**, distinct from "wording drift" (exit 1)

**Validation**: negative validation **31/31** (5 groups) · gate self-test **19/19** ·
full acceptance **34/34** (14 gates + 14 self-tests + tsc + eslint + vitest 211 +
cargo 1201 + build).

**Still needs a product decision** (not guessed, logged in `docs/TECH_DEBT.md`):
the zh/en wording of `cost.retry` · a likely mislabelled Chinese string on
`provider.models` · cleanup of two dead keys.

---

## Unreleased — Wrap-up: website semantics, gate 14, debt ledger (2026-10-01)

This batch is about **wrapping up**, not opening a new front. Every P0/P1/P2 item
in §2 is now either shipped or explicitly rejected.

This release does not change the scale: the `TOOL_SPECS` to **204**, the migration count reaches **85**.

**Website semantic accessibility (folded into `check-website.py`, contract 11)**

- Five rules: `<html lang>` required · **exactly one `<h1>`** · no heading-level
  jumps · icon-only `<a>`/`<button>` need an accessible name · `<a>`/`<button>`
  may not be entirely empty
- **No second HTML parser was written**: `check-a11y.py`'s criteria are
  React-specific regexes, while `check-website.py` already has an `html.parser`
  instance. A second one would be a second implementation — and two
  implementations always diverge the moment only one of them is edited, with no
  check catching the half that drifted
- Self-test 24 → **33**, negative verification 10 → **15/15**. The real site passes first try

**The 14th gate: `check-self-test-counts.py` (a gate about gates)**

- The ledger table "how many self-test cases does each of the 14 gates have"
  **can only be written by hand**. Measured: **9 of 13 produce no readable
  number** — they only print "all passed". Hands err, and the error is invisible
  in the exit code
- The fix is **not** to rewrite 13 working self-tests to satisfy a document;
  it is to automate the cross-check. This gate runs every self-test on each CI
  pass and compares line by line against the ledger — and **fails if a new gate
  was never registered there** ("ledger is missing N gates wired into CI")
- While writing the parser I hit two "it looks like it's reading, but it reads
  less" bugs: `m.groups() == 2` compares a tuple to an int and is always false
  (`ui-states`' 9 became 5), and `\S*?` cannot cross a space (`licenses`' 26
  became 7). **Neither turns the exit code red**
- Self-test 14/14 + negative verification 4/4

**New `docs/TECH_DEBT.md`: the single registry of what is still owed**

- Items scattered across a dozen "next batch" sections are collected into one
  table: **6 tech debts / 2 content debts / 3 product decisions / 5 rejected
  items** (including the condition under which each rejected item may be reopened)
- Along the way, several long-copied stale numbers were corrected:
  `ConfigPage.tsx` 1148 → **665** lines; the `chatSlice.ts` path moved from
  `src/stores/` to **`src/stores/slices/`** (the old path **does not exist** and
  was referenced five times); i18n duplicate keys "zh 5 / en 4" → **120 / 0 / 49**

**One methodology note**

- **"Green again after restore" does not mean "the file is back to normal"** —
  a gate only judges what it cares about. A dedicated residue check was added:
  16 probes (injection markers plus the content that should be present), each asserted
- Same family as "the injection must actually take effect": **both ends of a
  negative run need checking** — the start ("did the injection land?") and the
  end ("did the restore clean up?")

---

## Unreleased — Gates 12 & 13: website boundary and i18n wording consistency (2026-10-01)

This batch adds no features. It closes two outstanding gaps: **places where
something can break without any check noticing.**

This release does not change the scale: the `TOOL_SPECS` to **204**, the migration count reaches **85**.

**The 12th gate: `check-website.py`**

- All 11 previous gates scan **only `src/` and `docs/`**, while the website is the
  first thing a user sees: a dead link, a missing image, an anchor pointing nowhere,
  or somebody adding a CDN reference — none of it trips a single check
- The most important of the 10 contracts is that **resources must not be remote**:
  the site has to open on intranet machines, a CDN outage means a blank page, and a
  blank page is worse than an ugly one. **`<a href>` is exempt** — outbound links
  are what a web page is for
- Self-test 24/24 + negative verification 10/10

**Real gap it caught in the site**

- `meta.description` had an **empty `content`** (bad for SEO, and it was marked as a
  slot awaiting text). The gate caught it; a placeholder is now in place

**The 13th gate: `check-i18n-wording.py`**

- i18n wording drift is **purely visual**: no compile error, no test failure, nothing
  reports it. A user just sees `Copy Path` in one place and `Copy path` in another
- A fresh survey splits it three ways: 120 groups identical (normal — a nav item and
  a page title *should* share a sentence) / **14 groups differing only in case,
  31 keys** (mechanically decidable) / **49 groups using a different word, 149 keys**
  (needs human judgment)
- The 14 groups are now unified to sentence case. The choice came from **two
  independent rules that agree**: file-wide word frequency settled 6 of them (every
  winner came out sentence case), and the remaining 8 were settled by scope — they
  are in-widget labels, not panel titles
- The 49 different-word groups are **not gated**: `Delete` (a button) versus `Removed`
  (a diff statistic) should differ by position, and a hard rule would only force a
  single "right answer" that is wrong half the time. Full list in
  `docs/I18N_WORDING_AUDIT.md`
- Self-test 8/8 + negative verification 6/6

**How the i18n file was changed**

Per-key string replacement, **not** a full JSON rewrite. The diff is **14 lines**;
2325 keys are byte-for-byte untouched. Two traps along the way: the file stores
*nested* keys, so matching against a flattened path never lands; and one leaf key
appears twice, so anchoring on the leaf alone edits the wrong place. The correct
approach anchors on leaf **plus** old value, and reports rather than edits when the
match count is not exactly one.

**A new way a self-test can silently fail**

- The self-test once reported "**24/25**" while printing **no FAIL line at all** —
  red exit code, no visible cause. The total had been over-counted by one
- **A counting criterion that miscounts does not error**; it just prints a number
  that looks like the gate is broken. The fix is a guard assertion that makes the
  tally report itself, and the guard is then itself covered by the negative run
- The same mistake happened twice in one day, which makes it the **default failure
  mode of this kind of script**: any script printing "N/N" needs that guard

---

## Unreleased — Website skeleton (P2-5, content pending) (2026-10-01)

The last of the three commercialization items. This batch ships **structure only,
not a single word of copy** — the text is filled in later, and every slot is left
as a placeholder carrying a `data-fill` key.

This release does not change the scale: the `TOOL_SPECS` to **204**, the migration count reaches **85**.

**New: `website/` standalone static site**

- Single-page landing skeleton: header nav / hero / feature cards / screenshots /
  downloads / FAQ / footer, with responsive layout and light-dark support in place
- **No build step, no third-party CDN.** System font stack, inline SVG icons — the
  site has to open on offline and intranet machines, and a CDN outage means a
  blank page
- Assets wired up: `screenshots/01` and `07` are copied into the site directory and
  used in the hero and the screenshot section
- Every fill-in slot carries a `data-fill="section.field"` key; the conventions live
  in `website/README.md`. Attributes rather than Chinese placeholder text, so that
  filling content in bulk never has to parse prose — and never silently breaks
  when the prose is edited

**Fully isolated from the desktop app**

- The site does **not** take part in `npm run build`. `src/index.css` gained
  `@source not "../website"`: Tailwind v4's automatic scanning works per file
  rather than per extension, so without it the site's class names end up in the
  desktop bundle
- Styles do not share the app's `--app-*` tokens and use `--site-*` instead. A theme
  change on the desktop side silently rewriting the website is pure downside when
  the two have completely different audiences

**Fixed along the way**

- Two "currently **68** migrations" counts in the Chinese and English READMEs — the
  gate's pattern is "个迁" while the text says "个**顺序**迁移", so the check never
  matched and the number has drifted since the second batch. **Numbers outside the
  criteria expire silently**
- `check-docs.py` now derives its self-test case count by scanning its own source
  (the docs previously said 12 cases while the real number was 11, and nothing
  reported it)

---

## Unreleased — License tier model (P2-2) and the 11th entitlement-boundary gate (2026-10-01)

Positioning: write down *what the free tier gets and what Pro adds* as a **model**
first, payment second. This batch does not touch payments — the outermost shell of
the commercialization path is built now, checkout goes in last.

This release does not change the scale: the `TOOL_SPECS` to **204**, the migration count reaches **85**.

**New: license tiers (local tiering, no payment)**

- Three tiers Free / Pro / Team, five capability switches: multi-provider pool,
  team sharing, eval reports, LAN collaboration, audit trail
- The free tier is **fully functional locally**: sessions, builds, deploys, indexing,
  and diagnostic bundles all work. Only collaboration features and the Agent quota
  are limited (CNY 50/month budget · 200 runs · 2000 tool calls)
- Pro/Team set **no local ceiling**. Real cost belongs on the server; hardcoding a
  number in the client only gives people the false impression that a limit is in
  effect, which is worse than no limit at all
- The tier is stored in the local `settings` table and `set_license_tier` has no
  payment step. To go production, swap `current_tier` to read a server-signed
  entitlement — no other code has to change

**Capability gating governs "who may switch a feature on", never "who may switch it off"**

- All 11 gated entry points go through the `licensing::check()` gateway.
  `auto_pool_mode == 0` is **always allowed** — blocking the "off" path would trap
  a downgraded user with the pool stuck on and no in-app way out
- Three deliberate exemptions: `get_lan_server_status` and `get_lan_ips` are
  read-only local state that the license and LAN pages need in order to show
  "this feature requires Team" instead of an error; `revoke_lan_token` is a
  **closing action, not an opening one** — gating it would mean tokens already
  handed out can never be taken back after a downgrade
- Quotas reuse the existing `agent_quota_usage` counter. A second counter is bound
  to drift from real usage, and a drifted quota system lets over-limit users through

**New: license page `/license`**

- Shows the current tier's entitlements, quota usage (used / limit / breached) and history
- All six UI states are covered: loading, empty, partial, failed, retry, permission

**The 11th gate: `check-entitlement-gate.py`**

- 7 contracts: gated commands must pass the gateway, gated services must not be
  called directly, tier comparisons may only appear in `licensing.rs`, every
  capability enum member must be present in the matrix, the free tier must not
  contain collaboration features, the downgrade direction must not be blocked,
  and the frontend must not keep its own copy of the matrix
- Self-test 8/8 + negative verification 14/14 (including one end-to-end case:
  making the gateway always allow makes the Rust tests fail immediately)
- "The free tier must not contain collaboration features" is a product decision that
  **only a gate can pin down** — if code changes it quietly there is no compile
  error, just complaints on the first day of commercialization

**Fixed along the way**

- 16 count drifts across the Chinese and English README / ARCHITECTURE. Two of them
  (`68 sequential migrations`, `309 IPC registration entry points`) were outside the
  gate's pattern coverage entirely — **numbers outside the criteria expire silently**,
  and "ran the gate and everything was green" would never have caught them

---

## Unreleased — One-click diagnostic bundle export (P2-3) and the 10th privacy-boundary gate (2026-10-01)

Positioning: a support path users are **willing to send to a stranger**. It is the
exact opposite of the existing "Export backup" — a backup *is* your data; a
diagnostic bundle contains no user content at all.

No scale change in this batch: the `TOOL_SPECS` to **204**, the
migration count reaches **85**.

**New: one-click diagnostic bundle**

- Settings → Data gains a dedicated "Diagnostic support" block with a
  **preview → export** two-step flow. The preview lists every file that will be
  written, its size, and how many records each error source contributed.
  On export the backend verifies the digest of "what you previewed" against
  "what gets written" and refuses on mismatch
- Contents: app version / host / database schema version and row **counts only**
  + recent task, model-request, tool, agent-run failures and SLO alerts
  + a tail of device-side hilog. The bundle's own `README.txt` states what is
  **not** included
- Errors come from 5 sources (`task_runs` / `request_logs` / `tool_runs` /
  `agent_runs` / `agent_alerts`) plus the device runtime-log ring buffer. Each
  source is capped; when a cap is hit the preview **says so explicitly**

**Privacy boundary (the point of this batch)**

- The bundle never SELECTs user-content columns: conversation titles and message
  bodies, task goals, tool inputs/outputs, project names, conversation IDs and
  provider names are all excluded. `provider_id` survives only as a boolean
- Error text is the single free-text exit and passes three stages: secret and
  identity-pattern redaction → path redaction → **user-content landmark
  comparison**. Landmarks come from task goals / conversation titles / project
  names / recent user messages; a hit replaces the whole field with a placeholder
- **The landmark stage was forced in by the end-to-end tests.** Pattern redaction
  stops secrets and paths, but it cannot stop "the runtime copied the user's
  text into the error message" — arbitrary user text matches no sensitive
  pattern. That is a boundary of redaction, not a sloppy implementation
- Gate and tests cover different halves: the **gate guards outlet structure**
  (which columns are selected, through which accessor) and the **tests guard
  predicate effectiveness** (whether redaction and comparison actually hold).
  The boundary is written into the script's docstring rather than glossed over

**10th gate: `check-diagnostic-boundary.py`**

- Three tiers: **A forbidden** (user-content columns, never selected in any
  scope) / **B restricted** (error-text columns only via `row.clean(...)`,
  `provider_id` only via `row.present(...)`) / **C free**
- Judged **per function scope**: reading `messages.content` inside
  `collect_landmarks` is the defence, reading the same column inside `collect`
  is the hole — the criteria must tell those two apart for an identical SELECT
- 44/44 self-tests + 12/12 negative validations (including 1 end-to-end: neutering
  the landmark predicate makes the Rust test report the leak)
- Wired into CI with the same blocking power as the other nine gates

**Also fixed along the way**

- The second, private copy of path redaction in `reproduction_bundle.rs` moved
  into `utils::redact`; both sides now share one implementation (a copy drifts:
  missing one regex is a silent leak)
- Fixed 1 undefined CSS token introduced in this batch's UI (`--bg-subtle`, which
  the browser silently discards), 15 count drifts across README/ARCHITECTURE,
  and 2 new clippy warnings

---

---

## v2.3.0 — Verdict-Blindness Fixes, Tighter Acceptance and Eval Rules, Rebuilt Huawei Docs Fetch (2026-10-01)

Positioning: the core of this release is a dedicated audit of "**the report says everything is
fine, but it never actually looked at everything — or looked at the wrong thing**". This is not
a feature release but a systematic cleanup of verdict and gate defects, two of which are
deliberate tightenings: **some task outcomes change after upgrading** (see *Behavior changes*
below). Also included: the rebuilt Huawei docs fetch pipeline and backfilled API 26 data.

**Verdict blindness (the focus of this release)**

- The static-check gate no longer treats "ran successfully" as "passed". `check_code` and
  `run_lint` returned Ok no matter how many rules they hit, so only the fact that the tool ran
  was being checked. They now require an explicit clean verdict, and require the rule severity
  filter to cover errors — with a warning-only filter, "错误 (error)：0" is an artifact of the
  counting scope, not a clean result.
- The execution loop no longer treats "ran a command" as "verified the change". `run_command`
  always carried a `Command` label, so `echo hi` / `ls` / `cat` got one too, letting "a command
  was executed" impersonate "the change was verified".
- "No API/permission/capability inconsistencies found" was previously established on an
  **incomplete walk** (the third copy of the same directory traversal): inconsistencies on files
  that could not be read are simply invisible, yet the report still printed "0 error". Incomplete
  coverage is now disclosed, and the acceptance gate no longer accepts such a "nothing found".
- `check_code` reported a clean result for files it could not read, so the gate could not tell
  how much was actually covered; a truncated hit list can drop the high-severity group entirely,
  so "no high-severity hits seen" was not "no high-severity hits". Both now block with a reason.
- `vuln_scan` put a ✅ on security scans that **never ran** (the second instance of this family);
  `license_check` read "cannot read" as "none found" and reported a failed check as
  **compliance passed**.
- An empty read on the device no longer counts as a confirmed deploy; a `verify_ui` that reports
  a black screen no longer confirms a deploy.

**Behavior changes (may affect existing task outcomes after upgrading)**

- Acceptance gate: "which commands count as verification" used **substring containment** over the
  command head, so `cat test_notes.md` contained "test" and satisfied *tests passed* — `ls tests`
  likewise. A goal mentioning tests always generates that required criterion, so one `cat` could
  clear the entire test requirement. Matching is now **token equality** (compound commands such as
  `cargo test` require adjacent tokens in order; `npm run test:watch` matches on the script-name
  part). Real test/build commands behave exactly as before (`npm test`, `cargo test`,
  `python -m pytest`, `npx vitest run`, `cd frontend && npm run test`, …); what is now rejected is
  reading a file whose name happens to contain "test".
- Eval harness: artifacts declared in a task are now a **commitment**. They used to be a collection
  log, so a passing `grader` alone meant `resolved` — a fixture declaring `test-results/**` that
  produced nothing was waved through. A run that declares artifacts but matches none is now
  `unresolved`; the report names them in `missing_artifacts` and the CLI prints the missing
  patterns to stderr. Fixtures that only want best-effort capture should not declare the pattern.

**One predicate, two implementations — all converged**

- Parsing the XY prefix of `git status --short` had two implementations that had already diverged;
  and `review_changes`' scope filter was **always false** — the verdict depends on the leading
  space that distinguishes staged-only from unstaged-only, and the collection step `trim()`ed it
  away one line earlier. Both now resolve to a single source of truth (`git_tools::StatusEntry` /
  `parse_status_line`).
- The change-tool set, the visual-closure marker list, and the `secret_scan` traversal config had
  the same kind of second copy; all converged.
- The acceptance gate and the structured-result envelope used to disagree about whether an
  execution counted as verification (the latter was looser, matching substrings anywhere in the raw
  argument JSON). They now share one implementation.

**Destructive writes and undo**

- A file that exists but **cannot be read** was silently overwritten: the conflict check was
  skipped entirely, no undo snapshot was taken, and it still reported success — destructive,
  unrecorded, and falsely successful at once. Writing is now refused outright.
- `undo_edit` popped the stack before range-checking, permanently destroying undo capability while
  reporting "no write record"; a mid-way failure discarded the remaining snapshots, which were the
  only copies of the old content. Both fixed; the remaining snapshots are now restored on failure.
- Cross-file LSP edits were not atomic (a reported failure left the disk half-modified), and the
  write invariant only covered the fs side — the entire LSP write path bypassed it.

**"Cannot read" used to be reported as something else**

- An unreadable docs directory was reported as "not downloaded", prompting users to re-download an
  image they already had (which would not even fix the read failure). This is now a distinct state
  that explicitly says not to re-download.
- State-snapshot listing reported "does not exist" for "cannot read", and listed other people's
  json files from the system temp directory as this tool's snapshots.
- A failed UI recording read was reported as "device output format mismatch (0 rows)", and corrupt
  timestamps were treated as operations at time 0, zeroing the total duration. Signature self-check
  escalated "cannot read the material library" into "log into your Huawei account and regenerate".
- The build content fingerprint treated "cannot read" as "content unchanged" — it hashed paths but
  not content, so **stale artifacts were passed off as fresh ones**. The fingerprint now carries a
  completeness flag and refuses to resume from cache when incomplete.
- An unavailable sandbox was reported as "command timed out, try smaller pieces".

**Change detection, memory, and observability**

- `lsp_rename` changed the workspace without invalidating facts or project memory; `first_write`
  approval mode never prompted for `multi_edit` / `lsp_rename`; `multi_edit` changing files was
  judged as "no progress" and injected a stall warning.
- The change list shown to the user and the verification scope now derive from one tool set
  (previously "acceptance saw 4 files changed, the delivery list showed 1").
- Context compaction is rejected when the summary is not smaller than the content it replaces.
- `mcp_list` reported "N tools" without saying how many were blocked by the allowlist (a freshly
  created MCP server defaults to an empty `allowed_tools`, producing a count followed by nothing).
- Approval bypasses caused by a missing app handle are now logged instead of happening silently.

**Engineering and gates**

- The main loop was collapsed into a single round function.
- Cleared 6 mechanical clippy warnings (`needless_borrow` ×3, `trim_split_whitespace`, `dead_code`,
  `useless_format`), restoring the Q-07 warning baseline gate (57/57).
- Eval grader tests are now cross-platform (Windows uses the built-in `findstr` instead of `grep`,
  which has no standalone executable there); the local backend suite reaches 0 failures for the
  first time.

**Fixed: the API knowledge base had been idle since Huawei retired the `.md` endpoint**

- Version diffs and API reference bodies relied on appending `.md` to any doc page URL for Markdown. Huawei retired that endpoint (404 site-wide), so both feeds silently returned nothing. Content now comes from the doc center's own `documentPortal/getDocumentById` API (HTML), with anchor/table extraction and an HTML→Markdown conversion added on top.
- Release discovery now walks by content: `apidiff-2600` only lists the release/beta sub-entries (26.0.0 Release lives at `apidiff-7003`), so kit diff pages are collected level by level (max 3). The same walk also backfilled pages the old discovery had missed for older versions.
- Reference fallback slugs were re-verified against live objectIds (13 camelCase slugs were 404).
- Fixed a fetch regression: the HTML→Markdown tag branch treated a relative offset as absolute and looped forever.

**Versions: SemVer from 26.0.0 on**

- Huawei switched API versions to `X.Y.Z` from 26.0.0 (replacing `X.Y.Z(N)`), ordered `26.0.0 > 6.1.1(24) > … > 5.0.5(17)`. Parsing, comparison and format validation now live in one place, used by project parsing, SDK alignment, API availability, the create-project `sdk_version` check and `sdk-pkg.json` combination.
- Prompt and built-in knowledge entries follow: `6.1.1(24)` before 26.0.0, `26.0.0` from 26.0.0 on; `26.0.0(26)` is rejected as an invented combination.

**Data**

- API reference bodies are now resolved against the documentation catalog tree (`getCatalogTree`, 4,762 documents): coverage grew from 310 to **681 pages** (members 7,436 → 13,705). Documents whose title omits the module prefix (`@hms.ai.face.faceDetector` is titled just `faceDetector（人脸检测）`) are matched by a unique last segment plus parent-segment disambiguation; short or ambiguous segments are refused rather than guessed. Hardening: minimum segment length 4 → 3, parent-segment disambiguation when a segment collides (`@hms.core.map.map` picks `map-map`, not the Bluetooth MAP page), `errorcode-*` pages excluded, and ties broken **by namespace** (`@hms.*` gets the kit document, `@ohos.*` gets the `js-apis-*` one, so the two surfaces stop fighting over one row).
- Seventeen modules the rules cannot reach (e.g. `@hms.core.atomicserviceComponent.atomicservice`, `@kit.AREngine`, `@hms.health.store`, `@hms.core.weather`, `@hms.ai.AICaption`, `@ohos.arkui.WithEnv`) are pinned in an explicit mapping table after reading each candidate page: the criteria are the kit a page imports, the `-api`/`-capi-` suffix, and whether the page's capabilities match the module's d.ts, cross-checked through the doc site's own search API (the catalog tree only sees titles, and 809 pages there have no ASCII module name). Coverage reaches **681 pages** and module-level misses drop to **25**, every one of them verified to have no dedicated reference page - ArkUI internals, the retired `@ohos.multimedia.mediaLibrary` ArkTS page, HMS component pages, and capability modules such as `bluetooth.opp`, `file.keyManager` and `resourceschedule.deviceStandby`.
- Two stale aliases in the fallback slug table (`@ohos.preferences`, `@ohos.relationalStore`) now carry their real module names (`@ohos.data.*`): alias and real name point at the same document, and since `api_details.slug` is unique the alias would claim the row and leave the real module looking unresolved.
- The shipped seed database was **rebuilt from an empty database** instead of stacking onto the previous crawl: 14 versions / 1,014 pages / 47,410 changes written, deduplicated to **44,856** `api_docs` rows with 44,856 vectors, **280MB** (the stacked state was 505MB). 26.0.0 carries the full `apidiff-7003` set added by the August 29 Release.
- Fixed the duplication introduced by the previous refresh: HTML bodies produce different declaration strings than the Markdown era, so the conflict key never matched and every row was inserted rather than updated (stale `.d.ets` module names and two generations of the same API side by side). The upsert now refreshes the derived `module` column and the seed was rebuilt empty; stale names are gone and vectors match `api_docs` 1:1.
- Seed import learned to ship same-version refreshes: when a version label keeps its name but gains rows (26.0.0 Beta → Release), the new rows are backfilled additively, keyed on a seed revision.

**Compatibility and rollback**

- No database migration and no schema change; the seed database is a local build artifact (not tracked) and can be regenerated with `cargo run --bin full_fetch --features full-fetch,embedding --release -- --seed` (~6 minutes: 1 minute of crawling, 4 minutes of vector indexing).
- The old `.md` fetch path is gone; if Huawei restores that endpoint, revert this fetch rework.
- **The two tightenings to acceptance and eval rules need to be rolled back separately**: they
  turn some previously passing tasks into failing ones, which is a correction rather than a
  regression. To return to the old behavior temporarily, revert the token matching in
  `agent/acceptance.rs` (`is_command` / `declared_validator`) and the `missing` check in
  `eval_workspace::collect_artifacts` — no need to revert the whole release.
- The tool protocol (`ToolResultV2`) is unchanged in this release. The new `missing_artifacts` field
  on `OutcomeInfo` is an optional addition that older readers ignore, so it is not a breaking
  change.

**UI refinement and commercialization push (this round)**

- Fixed 6 CSS variables that were **never defined** (`--bg-muted` / `--bg-tertiary` / `--info` /
  `--muted` / `--danger-border`, 16 references failing silently), and added `check-css-vars.py`
  to block them permanently.
- Collapsed 15 arbitrary font sizes into a 7-step ladder — and **actually wired the ladder up**:
  1238 places had been written as literals, so the ladder was decoration. Interface text size
  is now adjustable from Config, scaling 1299 token references and 375 Tailwind named sizes together.
- Rewrote the settings page from a bare JSON textarea into three sections. Investigation showed
  `deveco.jsonc` is **not** this app's settings (they live in localStorage), so edits there did
  nothing — a more fundamental problem than being hard to use.
- Added a three-step first-run guide, unified empty states onto `EmptyState`, brought the command
  palette up to combobox semantics, and added three-tier `prefers-reduced-motion` degradation.
- `Home` chunk 703.0 → **416.3KB**: productizing the sourcemap attribution showed the real weight
  was the pinyin dictionaries (286KB packaged, 30.7% of the chunk), which are now lazy-loaded.
  The bundle budget is tightened from 750KB to **440KB** accordingly.
- **Added local usage stats** (P2-1): off by default, local SQLite only, no upload path anywhere,
  event names restricted to a whitelist. Adds the `usage_events` table, bringing the database
  migration count reaches **85**, with 39 `commands/` modules, 313 Tauri IPC entry points and
  17 React pages. The tool count remains `TOOL_SPECS` to **204** (no Agent tools added or removed
  in this round).
- **Added About / license page** (P2-4): ships `LICENSE` (MIT) and establishes the license
  boundary of all 724 components (692 permissive / 26 notice / 5 copyleft, all MPL-2.0
  introduced by Tauri / 1 unknown) — **no GPL, AGPL or SSPL**. `/about` can export
  `NOTICE.txt`.
- **Density and typography now finished** (P1-4): at the default 1100×720 window the
  conversation list shows **7 rows instead of 6 (+16.7%)**, and row height goes
  50.09px → **45.09px, uniform across all rows**. The half that previously could not be
  verified (spacing) now has a measurement net and a gate.
- **Ninth gate, `check-density.py`**: the judge is "the number of fully visible rows must
  not fall below the committed value", split into a contract layer (both platforms) and a
  measurement layer (windows-latest re-measures on the spot). **Zero new dependencies** —
  it drives the system Edge over CDP using Node 22's built-in `WebSocket`, deliberately
  avoiding Playwright/Puppeteer.
- **The CSS-variable gate gained its other half**: it previously only checked "referenced
  but not defined"; it now also checks "**defined but never consumed**" — equally silent,
  since the line sits in `index.css` in plain sight and the next reader assumes it works.
  Six dead tokens were deleted, three of them **duplicate names with byte-identical values**
  (`--success-500` = `--success`, in both dark and light), and one had an **invalid CSS value**.
  Another 16 are recorded in an allowlist that carries a reason per entry, and **an entry
  goes red once it is no longer needed**.
- 8 Python gates added in total (CSS variables / font sizes / empty states / a11y /
  telemetry contract / licenses / density / gate self-tests in CI).

**Several "the number was right but meaningless" problems fixed in this round** (none of
them raise an error, and neither grep nor a screenshot can see them):

- The message list is virtualized, so the number of entries in the DOM is **not** the
  total (30 messages mount only 5). The total is now reported by the app itself, and the
  probe only checks "total ≥ mounted".
- A session row's short-ID badge is `opacity-0` by default (completely invisible) yet
  still reserved about 29px of horizontal space. As soon as an archive badge appeared it
  pushed that line to wrap, doubling the row height from 50px to 66.5px.
- Every one of the 40 fake conversations in the measurement fixture was stamped a year
  ago, so all of them triggered the "cleanup?" badge — the baseline was measuring the
  **worst case, not the typical case**. The measurement clock is now frozen, and a gate
  requires the fixture to cover 4 time groups plus both archive-badge states.

## v2.2.0 — Structure-First Navigation, Multi-Language Write Gates, and a Trustworthy Execution Kernel (2026-09-15)

Positioning: moving from "the agent can edit code" to "edits are language-checked and verifiable" — writes pass language-level gates, host operations carry revocable approval receipts, and resource isolation has an explicit support matrix. 259 commits.

**Structure-first navigation and large-repo indexing**

- New structure-first code navigation with a persistent repository catalog: whole-tree file registration, incremental file deltas, git-checkpoint reconciliation, and a resilient index watcher; progressive indexing at million-file scale has its own baseline (see [Index Scale Baseline](docs/INDEX_SCALE_BASELINE.md)).
- Structure queries support stable cursors, paginated structure nodes, and persistent graph edges; results are bound to safe file-read windows.

**Pre-write gates (headline of this release)**

- Real per-language gates: Java (Tree-sitter declaration/annotation checks plus a `javac` diagnostic diff that also compiles **unedited callers** in the same source root), Dart (`dart analyze` diff), Go (`go vet` diff in a temp module that carries same-package sibling files), Python (`pyflakes` diff), SQL (`sqlite3` in-memory execution diff with side-effect statement protection).
- Syntax-level coverage for ArkTS/TS/JS/Go/Python/Rust/Dart/Kotlin/C++/Swift; when an external tool is missing or times out the gate degrades to "not checked" and records an event — writes are never blocked and never falsely reported as verified.
- Multi-file transactions: baselines re-verified before batch and per-file writes, conditional reverse-order rollback, external edits preserved, and "rollback incomplete" reported honestly.

**Host capability approvals and revocable receipts**

- Capability-agnostic v4 approval receipts (OTA content digests are just one scope kind) with self-describing revocation; agent-initiated mutating host capabilities must hold a valid receipt, which expires on stop or restart.
- The approval card now shows an impact contract (reversibility / blast radius / targets / consequence), and the same wording flows into pending-confirmation restore and the audit timeline.

**Sandbox and resource limits**

- Native sandboxed children get enforceable rlimits (CPU time verified locally; memory and process count are reported honestly as unavailable/unmapped per platform); host-direct commands can opt into limits via environment variables.

**UI and quality gates**

- Design tokens converged to an IDE feel (radius/shadow/type scale); static inline styles moved to utility classes.
- Documentation-drift and clippy warning-baseline gates are green again; a new [Current Status](docs/CURRENT_STATUS.md) page is the single source of truth for capability state.

## v2.1.1 — Interaction and Quality-Gate Hardening (2026-08-25)

- Provider Token fields now preserve and prefill full values with K/M suffix guidance; reply-language and reasoning-language configuration is also refined.
- File drag-and-drop now supports external files, batch imports, and browser fallback paths, while in-project references remain portable and relative; SVG MIME detection, duplicate/failure feedback, and regression coverage are included.
- Improved bilingual README, development, and release documentation; repaired documentation-drift validation and added Node.js version preflight plus frontend bundle-size enforcement.
- Fixed Windows/macOS toolchain and packaging-path compatibility, with stricter ESLint, Rust test, Clippy, and cross-platform prerelease checks.

## v2.1.0 — Evidence-Driven Governance & Dual Execution Kernel (2026-08-22)

Positioning: upgrading "the model can call many tools" to "tasks and tools are durably scheduled, acceptable, recoverable, and observable". The reliability & governance batch adds migrations `057`—`062`, `069`—`075`; work continues on long-session Context V2, and the total migration count reaches **84**. Subsequent capability work brings `TOOL_SPECS` to **204**.

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

- The tool total is authoritative at the length of the `TOOL_SPECS` array in `src-tauri/src/agent/tools/mod.rs` (currently 204).
- Task groups are authoritative at the `TASK_GROUPS` constant (currently 8: `build` / `fix` / `explore` / `deploy` / `refactor` / `test` / `debug` / `other`).
- `quality_tools::*` is exposed via the facade; **importing the 4 sub-files directly (`quality_metrics`, etc.) is forbidden** — they are internal modules; external coupling goes through the facade.
- Any "split by line count" of tools is forbidden. **Must slice completely by method**, with signature + body in the same file. Script aid: `scripts/legacy/_split_quality.py`.
- Any CHANGELOG change should be committed with `docs(changelog): <one-liner>` in the commit message; do not edit this file and then commit with plain `docs:`.
