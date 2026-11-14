# Teams execution replacement implementation

## Current acceptance status — 2026-11-06

M0/M1 remain in progress. The exact newly authorized Market archive `bd754f3d…cefa5` built successfully and API revision `market-api-00009-nk6` is live. Bot v3 uses the existing direct gateway; real browser cold Range/If-Range/auth checks passed. Both frontends are deployed, including the reviewed account-switch metadata/intent isolation fix, and all served files match their recorded manifests.

The exact approved production2 old bindings/4 Bot conversations and obsolete trigger index were removed after fresh guards. Actual Desktop userData retirement also completed: the reviewed old `team-bot-host` contained1 file/22 bytes; it was removed with no active writers, CAS checks and hashes proving all25 personal Bot files unchanged. No personal Bot record was rewritten; remaining cleanup actions0. Private receipt: `.omx/private/teams-m1-qa/local-retired-cleanup-receipt.json` in the workspace.

Fresh Android direct-root package passed1,894 tests and is installed on Samsung under the isolated QA identity; member account has zero configured providers. Real human messaging and Bot download handoff were observed, but full publication→download→installation ACK→sharing→member answer is still pending. Thirty meaningful real-model cases have not run. The independent iPhone package is signed; physical acceptance remains pending.

The new signed Desktop ZIP `4bc25b9115c4568316c0f21d91f411f372d4b91e832ead1a088e54409efd29f8` (383,564,474 bytes) passed the full official Flare signing, static and runtime gates. The original Broker registration was restored. Actual checks include provider persistence, cold session restoration, Local App delivery/Modify and no-reload draft round trips. The provider lock/shutdown and fresh-owned replay corrections have focused regressions and source review. Artifact inputs are baseline `19658a3` plus recorded uncommitted source hashes; no clean-commit provenance is implied. Receipt: `.omx/plans/evidence/2026-11-05-m0-m1/desktop-replay-runtime-pass.json`. Actual business UI publication is now starting; install ACK, shared deployment and thirty real-model cases remain pending. The concurrent Desktop task remains paused under the authorized coordination window.

The reviewed Web join fix is deployed;34 served files and the custom-domain index/AASA passed36 hash checks. A real first-admission response-loss retest passed9 checks: production200/alreadyMember:false at generation2 was followed by deliberately aborted delivery; refresh did not auto-join; explicit retry returned200/alreadyMember:true and closed the dialog with a verified selected Team workspace. Owner removal restored the outsider negative role; original four member documents and seven business documents were unchanged. That snapshot initially retained loading indicators because cleanup followed immediately. A separate bounded generation3 retest passed10 checks: real list/detail identity, selected Team and MemberHome became fully ready without loading indicators or alerts in1,413ms and remained stable for1,010ms under a30-second limit. Normal owner removal restored the outsider; original four member documents and seven business documents remained unchanged. Epoch3, invite use7 and exact join receipts are retained for audit/cleanup. The earlier failed run remains retained. Fresh OAuth and physical iPhone acceptance remain open.

The new family Bot was published through the real five-file review UI to self/hidden, then approved through the normal exact-digest review API. All five source files matched. The temporary QA reviewer role was restored and its revocation fence retained in the registered immutable journal. Samsung is currently physically disconnected, before owner login or a business download intent; the user has been asked to reconnect it. Business install/ACK, sharing and thirty real-model cases remain pending.

Main merge is authorized only after M0/M1 acceptance. No merge-to-main or complete-QA-cleanup claim is made. The checkpoints below are historical and may describe earlier pending states.


## M2 / Q02 — durable human waits, continue permits and cancellation (Firebase backend)

Implemented in `apps/firebase/src/shared-bots.mjs` and `shared-bot-commands.mjs`; no scheduler, no UI, no compatibility shim. Records changed in place (there is no deployed data).

- Task record: `epoch` (0 until `startTask`, which sets 1), `attemptStartedAt`, `activeBudgetMs` (300000), `activeSpentMs`, `physicalDispatchesSpent`, and optional `currentWaitId`, `checkpointDigest`, `resume {waitId, answerMessageId, answerSeq}`, `cancelRequestedAt`. `deadline` is the end of the current phase: queued = pick-up limit (created + 300000, or `max(now, lock.until) + 300000` when the Bot×Chat lock is live at submit), running = `attemptStartedAt + (activeBudgetMs − activeSpentMs)`, waiting = wait expiry. Stored states add `waiting` and `cancelled`; a waiting task past its deadline reads as `expired` (derived, like the other states). The member view adds `epoch`, `activeSpentMs`, `cancelRequestedAt?` and, while waiting, `wait {waitId, kind, recipient, expiresAt, messageId}`.
- `deploymentRecord.capabilities.durableWait` is a boolean; `pauseTask` needs it (409 `DURABLE_WAIT_UNSUPPORTED`).
- `pauseTask` (host): requires the running task, its current permit and `attemptStartedAt <= pausedAt <= min(now, deadline)`. It applies the same output-audience gate as `completeTask` (`outputGate`, shared code). Refused: the generic notice is published exactly as a refused result, the task ends `authorization_required`/`failed`, no question and no wait exist. Allowed: one Bot question message (id from `[taskId, waitId, 'question']`, `waitId`, `replyTo` the request), `teams/{team}/waits/{waitId}` (server-only; also stores a private `pauseDigest` so an identical replay returns the same response and a different body for the same `waitId` is `WAIT_CONFLICT`), task `waiting` with the permit invalidated, active time charged up to `pausedAt` only, and the Bot×Chat lock kept with `until = deadline`.
- `continueTask` (host): one unrepeatable capability shared with `startTask` (receipt `granted`); a replay of a granted call returns `{execute: false}`. `reason: 'answer'` needs a queued task with `resume`, the stored `checkpointDigest` (`CHECKPOINT_MISMATCH`) and an answer message that still exists, unrecalled and unedited (`SOURCE_CHANGED`). `reason: 'recovery'` needs the running attempt on the same host before its deadline, charges `now − attemptStartedAt` and keeps the absolute deadline. Both run the same revalidation as `startTask` (`admitRun`: deployment readiness and epoch, revocation, source watermark `inputLastChangeSeq`, Bot×Chat lock ownership, message order). Remaining budget under 1000 ms fails the task and releases the lock. `startTask` refuses a task whose `epoch >= 1`.
- `answerWait` (member): only the recorded recipient at the same membership generation; wait pending, unexpired and the task's current wait (`WAIT_CLOSED`); writes the human answer and its change with the `submit` idempotency scheme; no quota, no new task, no `mentions`/`taskIds`, so the answer is never admitted as a request. The task returns to `queued` with `resume` and a fresh 300000 ms pick-up limit.
- `cancelTask` (member): requester at the current generation or the Bot maintainer (`TASK_CANCEL_FORBIDDEN`). Queued/waiting end `cancelled` (wait `revoked`, lock released); running sets `cancelRequestedAt` once, after which `validateTask` is `TASK_CANCELLED`, and the one accepted `completeTask` ends `cancelled` with no reply or artifact but with usage and `resultDigest` recorded. A cancelled run the host never closes reads as `cancelled` once its deadline passes. Terminal tasks (including derived `expired`/`interrupted`/`cancelled`) are `TASK_FINISHED`.
- Tests: `apps/firebase/test/shared-bots.emulator.test.mjs` (`wait:` cases), `team-execution-contract.test.mjs`, `teams.rules.emulator.test.mjs` (members cannot read or write `waits`), `apps/web/test/teams-bots.test.ts` and `packages/teams/contracts/execution-vectors.json` (vectors added only; the iOS and Android member-command validators accept `answerWait`/`cancelTask` because they read the same vector file).
- Not done here: native UI. The Web/Desktop UI, the Electron allowlist for the two member actions and `chat-source.ts` surfacing `waitId`/`sourceTaskId` followed in `43116521c`; the host side is the next section.

## M2 / Q04 — operation ledger (Firebase backend)

Implemented in `apps/firebase/src/shared-bots.mjs` and `shared-bot-commands.mjs`; tests in `apps/firebase/test/shared-bots.emulator.test.mjs` ("operations: …"). Nothing is deployed. No host or runtime calls these commands yet: the runtime gate and the Desktop broker are the second half of Q04, and the first real write tool arrives with Q06.

- Identity: `operationId = sha256(JSON [rootTaskId, taskId, logicalStepId, category])`, recomputed by the server (`OPERATION_ID_MISMATCH`). `logicalStepId` is the runtime's durable step (`<taskId>/t<group>/c<index>`), never a provider call id. A task is its own root until delegation exists. `digest` (tool version, normalised parameters, connection, resource version, target, policy) and `approvalRef` are fixed by the first record: a different value is `OPERATION_CONFLICT`.
- Categories: `read`, `store_idempotent`, `provider_idempotent` (safe to send twice), `queryable`, `unqueryable`. Every category but `read` needs `deployment.capabilities.writeOperations` (`WRITE_OPERATIONS_UNSUPPORTED`); the contract now accepts `true`, the Desktop host still publishes `false`.
- `recordOperation {deploymentId, hostId, accountEpoch, taskId, permitId, operationId, logicalStepId, category, tool {name, version}, digest, approvalRef?, dispatchedAt}` is the durable prepared → dispatched record and must be accepted before the host calls out. It needs the live permit of a running, not cancelled, still authorised task (`EXECUTION_PERMIT_REQUIRED`, `TASK_CANCELLED`, `TASK_EXPIRED`, `TASK_REVOKED`), at most 64 operations per task (`OPERATION_LIMIT`). Response `{operationId, state, dispatch, resultDigest?, reference?}`: `dispatch: true` when this call created the record, or when a replay-safe operation is asked again under the current permit (a recovered attempt); otherwise `dispatch: false` with the recorded state, and the receipt when it is settled. The same command id replays its first response.
- `settleOperation {…host, taskId, permitId, operationId, outcome: succeeded|failed|unknown, resultDigest, reference?, settledAt}` records the outcome under the task's current permit, also after the task ended. Identical replays return the same; a different outcome is `OPERATION_CONFLICT`. Only a `queryable` operation may leave `unknown` through the host.
- Unverified operations: a `queryable` or `unqueryable` operation in `dispatched` or `unknown`. The task carries their count (`unverifiedOperations`, also in the task view when above zero).
  - `continueTask` grants nothing while one exists. If all of them are `queryable` and `dispatched`, the response is `{execute: false, taskId, reconcile: [operationId…]}` and the task is unchanged: the host asks the outside system, settles under the permit it still holds, and asks again. Otherwise the task becomes `needs_reconciliation`, the Chat lock is released and the response carries that state.
  - `completeTask` over an unverified operation publishes the reply but stores `needs_reconciliation` (the earned state is kept and applied when the last operation is verified). A cancelled task stays `cancelled` and keeps showing `unverifiedOperations`. A running task past its deadline reads `needs_reconciliation` instead of `interrupted`/`cancelled` while one exists.
- `resolveOperation {taskId, operationId, resolution: succeeded|failed}` (member command, Bot maintainer only: `MAINTAINER_REQUIRED`) states what happened to an unverified operation. Refused while the run could still report (`TASK_RUNNING`) and for an operation that is not unverified (`OPERATION_CONFLICT`). Response `{taskId, state, operationId, operationState}`.
- `listOperations {taskId}` (requester and maintainer, both must still read the Chat): `{taskId, operations: [{operationId, logicalStepId, category, tool, state, epoch, dispatchedAt, settledAt?, resolvedBy?, resolvedAt?}]}`. Parameters, digests and results are not returned.
- New task state `needs_reconciliation`. All four clients label it and offer the review (see "M2 / Q07–Q08 clients").

## M2 / Q07 — shared documents: revisions, conflicts and history (Firebase backend)

Implemented in `apps/firebase/src/shared-bots.mjs` and `shared-bot-commands.mjs`; tests "documents: …" in the emulator suite. Not deployed. The clients do not offer editing yet, and no host sends `revises` yet.

- Storage: `artifacts/{artifactId}` stays the current revision (`revision`, `digest`, `document`, `updatedAt`). Every revision, including the first, is also kept in `artifacts/{artifactId}/revisions/{n}`: `{artifactId, revision, parentRevision, operationId, digest, document, author, sourceTaskId?, createdAt}` with `author` either `{kind: 'shared_bot', sharedBotId}` or `{kind: 'user', uid, generation}`. An artifact has at most 200 revisions.
- `reviseArtifact {artifactId, expectedRevision, operationId, document {title, body}}` (member command): the member must read the document (same rule as `readArtifact`) and take part in its Chat. The edit becomes revision `expectedRevision + 1` only if `expectedRevision` is current: otherwise `ARTIFACT_REVISION_CONFLICT` and nothing is written, so the client reads the current revision and saves onto it. `ARTIFACT_UNCHANGED` for an identical document. The operation id is written in the same transaction: the same operation with the same content returns `{artifactId, revision, digest}` of the revision it made, also under a new command id; with other content or another base it is `OPERATION_CONFLICT`.
- `readArtifact {artifactId, revision?}` returns the current revision, or the named one (`ARTIFACT_REVISION_REQUIRED` if it does not exist), and now also `currentRevision`.
- `listArtifactRevisions {artifactId}` → `{artifactId, currentRevision, revisions: [{revision, parentRevision, digest, title, author, createdAt, sourceTaskId?}]}`, newest first, without bodies.
- `completeTask` accepts `revises {artifactId, expectedRevision}` next to `document`: the result is saved as the next revision of that artifact instead of a new one. It must be a document of the same Bot revision in the same Chat, with the Chat's current readers, at exactly that revision, and changed. Otherwise nothing is saved, the reply is a fixed notice instead of the Bot's text, the task ends `failed` with `artifactId: null`. A repeated completion returns the same response.

## M2 / Desktop host — questions, answers, recovery and cancellation

Implemented in `apps/electron/src/main/teams/connector.ts`, `process.ts`, `privateStore.ts` and `service.ts`. It needs the runtime of SDK branch `claude/teams-m2-runtime-20261111` (`inspect_task`, `continue`, `askRequester`, `status: "waiting"`; contract in the SDK's `docs/teams/shared-chat-runtime-m2.md`). The product `Cargo.toml` still pins the earlier runtime: the pin must move before this host is packaged, because the earlier host binary rejects the new bootstrap fields and the earlier worker rejects `waitId` in source messages.

- Capability: a publication sets `capabilities.durableWait` to true only when the Bot's model route is one the runtime can continue (`open_ai_chat` today). A Bot published before this change keeps `false` until it is published again. The worker is offered the question tool (`askRequester`) only when the deployment the permit came from has `durableWait` and the route qualifies.
- Execution record (`executions/<taskId>`, schema 2, replaced atomically): the current permit and attempt number, the source watermark, the admitted input batch (kept until the task ends so a continuing worker gets the same bytes; removed with the record), the server's dispatch count, usage of earlier attempts, the number of recoveries, and one of `pause` (a question not yet accepted), `wait` (accepted; continues only through its answer) or `answered` (the answer the current attempt continues with). Records of the retired single-attempt format are removed when met.
- Question: a `waiting` worker result becomes one saved `pauseTask` request (24 h expiry) that is sent unchanged until the server accepts it. A transient failure or `TASK_EXPIRED` (host clock ahead) keeps it for the next pass; `TASK_CANCELLED`, `TASK_REVOKED`, `WAIT_CONFLICT`, a missing permit or an unsupported deployment end the attempt with a failed completion under its own permit (a cancelled task then reads `cancelled`). While waiting there is no worker and no concurrency slot. A wait that is never answered is forgotten after its expiry plus the pick-up limit.
- Answer: a queued task with `resume` is not started again. The host inspects the checkpoint (it must still name that wait), reads the answer message by id (same wait, same seq, not recalled), asks `continueTask(reason: 'answer')`, saves the new permit and runs `continue` with `continuation.answer`. Only the host that asked holds the session, so another host leaves the task to expire.
- Recovery: an attempt that ended without a verdict (the host is closing, the host process died, the worker died) is no longer failed on the spot. Discovery reads the result if one was recorded, otherwise the checkpoint: an unreported question is reported; a continuable attempt gets `continueTask(reason: 'recovery')` and a `continue` worker, at most twice per task; anything else, or a refused continue, ends with a failed completion under the current permit. Lost authority, a cancellation (`validateTask` answers `TASK_CANCELLED`, which stops the worker) and a refusal by the worker itself still end the task at once. Past the attempt's deadline a task without a result is failed as before.
- Budgets: each attempt gets the server's deadline for that permit and what is left of 32 model requests over the whole task. Usage is the sum of the attempts that reported one and is `partial` whenever more than one attempt ran.
- Delivery: a discovery pass no longer sends the saved result of a run that is still sending it itself.
- Tests: `apps/electron/test/teams-host.test.ts` (question/answer round trip with the same input batch, route without continuation, recovery and its limit, refused continue, cancellation, question retry and refusal, unreported question, answer recovered after a second interruption, forgotten wait, process frames for `task_checkpoint` and `waiting`, permit hand-back for `continueTask`). All fixtures stand in for the host binary; no real worker ran against this host yet.

## M2 / iOS and Android — waiting tasks, answering and cancelling

Implemented in `apps/ios/native/Sources/Teams/` (`TeamBots.swift`, `TeamBotViews.swift`, `TeamMessageProjection.swift`, `TeamMessagingModel.swift`, `TeamMessageViews.swift`, `TeamConversationView.swift`, `TeamsListView.swift`, `TeamsBotText.swift`, `TeamsStrings.swift`, `TeamsErrors.swift`) and `apps/android/native/app/src/main/java/com/lingxi/code/teams/` (`TeamBots.kt`, `TeamBotComponents.kt`, `TeamMessageProjection.kt`, `TeamMessagingModel.kt`, `TeamConversationScreen.kt`, `TeamsListScreen.kt`, `TeamsTab.kt`, `TeamsUi.kt`). Both clients follow the Web/Desktop behaviour.

- Task: `wait` (`waitId`, `kind`, `recipient`, `expiresAt`, `messageId`) and `cancelRequestedAt` are read when present. `waiting` is not a final state; `cancelled` is. Only `queued` and `running` tasks are polled every 3 s; a waiting task is refreshed at its `deadline`, which is the wait's expiry.
- Card: the state line reads "Waiting for a reply" / "Cancelled", or "Cancelling…" while a running task has `cancelRequestedAt`. A waiting task adds "Waiting for you · expires in 4 minutes" or "Waiting for <member> · expires …" (minutes under an hour, hours under 48, then days, rounded up, in the UI language). A task cancelled before any model call shows no usage line.
- Answer: the Bot's question message carries `waitId`. While its task waits for the viewer (same uid and generation) and no answer to that wait is in the outbox, the card on that message shows Reply, which starts an ordinary reply to the question. Sending a reply whose target is such a question queues `answerWait {conversationId, waitId, messageId, changeId, text}` in the outbox instead of a message or a `submit`: it asks no Bot and starts no task. `WAIT_REQUIRED`, `WAIT_RECIPIENT_REQUIRED`, `WAIT_CLOSED` and `TASK_REVOKED` are final: the draft stays as a failed row the member can discard.
- Cancel: the requester and the Bot's maintainer see "Cancel task" on a task that has not ended and is not already being cancelled. It sends `cancelTask {taskId}` directly (not through the outbox), treats `TASK_FINISHED` as done, and reads the task again.
- Tests: iOS `Tests/TeamTaskUsageTests.swift` and `Tests/TeamsTests.swift` (states, who may cancel, who may answer, expiry rounding, the `answerWait` body and its survival of a restart, catalogue keys); Android `TeamsBotTextTest.kt` (the same rules) and `TeamMessagingQueueTest.kt` (an answer is durable before one `answerWait` request, a closed wait is not retried, cancel sends `cancelTask` then `getTask`). Not run on a device or simulator: the card layout, the Reply/Cancel buttons and the expiry wording are checked by compilation only.

Target: detailed design 0.5 and development plan 1.4, M0 then M1a/M1b.

## M2 / Q06 — the shared list (Firebase backend) and its runtime tools

**Deviation from the detailed design, to be confirmed.** Design §14.3 puts the list in the maintainer's Local App
instance and reaches it through `serviceBroker`/`remoteGateway` with a new TeamAppGrant. That gateway work is not built.
The list is instead one server-owned document per shared Bot (`teams/{team}/lists/{sharedBotId}`), so members see the same
data on every client without the maintainer's Desktop being online, and so the write can be verified in the emulator. The
runtime and host only know a "list service" with read and operation-id write; a Local App adapter can replace the store later.

- Publication: `publishSharedBot.sharedList {title, description}` (optional). The first publication with a list creates it
  empty; later publications keep its items; a publication without it stops sharing but deletes nothing. The Bot card carries
  `sharedList.title`. `startTask`/`continueTask` return `sharedList` only when the deployment has `capabilities.writeOperations`.
- Members (`readList`, `writeList {sharedBotId, operationId, expectedDataRevision, changes}`): whoever may use the Bot, and
  its maintainer. A write is 1–20 changes (`add`, `update`, `remove`) applied together on the revision they were made from;
  otherwise `LIST_REVISION_CONFLICT` and nothing is saved. `operationId` makes a repeated request return what it saved.
  Other refusals: `LIST_REQUIRED`, `LIST_ITEM_REQUIRED`, `LIST_LIMIT` (500 items, or 48 KiB for all items as JSON, so the
  whole list is always one tool result for the Bot), `OPERATION_CONFLICT`.
- Tasks (`readTaskList`, `writeTaskList {…host, taskId, permitId, operationId, expectedDataRevision, changes}`): the write
  must already be a `store_idempotent` ledger operation of the task for tool `TeamListWrite` whose digest is the hash of
  `{expectedDataRevision, changes}`. The list change, the stored answer and the ledger outcome (`succeeded`, or `failed` for
  a conflict or a missing item) are one transaction, so the store never holds a dispatched-but-unsettled write and a repeat
  returns the first answer. A cancelled task writes nothing (`TASK_CANCELLED`).
- Runtime (SDK branch `claude/teams-m2-runtime-20261111`): `TeamListRead` / `TeamListWrite` tools, registered when the host
  start frame carries `sharedList`. The write's operation is named by the durable logical step of its call and is sent to
  the host only after that call is on the transcript; a killed worker repeats the same operation on continuation; an
  outcome the host cannot establish ends the process with the call unfinished.

## M2 / Q06 — Desktop host and the list on Web and Desktop

- Publication (`apps/electron/src/main/teams/connector.ts`): `PublishLocalBotInput.sharedList {title, description}` is
  optional. A deployment publishes `capabilities.writeOperations` exactly when its model route can be continued from a
  checkpoint (today `open_ai_chat`), because a list write is recorded before it is sent and repeated after an interruption.
  A list on any other route is refused with `SHARED_LIST_UNSUPPORTED` before anything is published.
- A run: the `sharedList` the service returns from `startTask`/`continueTask` is kept on the saved execution (`list`) and
  passed in the `run`/`continue` bootstrap. `runTeamHost` (`process.ts`) accepts `operation` frames only from a bootstrap
  that carried the list, checks that a write names a step of this task, authorizes, and answers `operation_result`
  `{established, succeeded, result}`.
- A write (`TeamConnector.operate`): arguments are checked with the shared validator first (`INVALID_LIST_CHANGE` is told to
  the Bot and nothing is recorded). Then `recordOperation` with `operationId = sha256([taskId, taskId, logicalStepId,
  "store_idempotent"])` and the digest of `{expectedDataRevision, changes}`; only then `writeTaskList` under the same id.
  - `OPERATION_LIMIT`, `OPERATION_CONFLICT`, `WRITE_OPERATIONS_UNSUPPORTED` from the record: nothing was sent, the Bot is told.
  - Any other definitive refusal of the record ends the run like every refused worker frame (a cancelled task, a lost permit).
  - A record or a write whose answer never arrives, a record that may not be sent again, a refused write: `established: false`.
    The worker stops with the call unfinished (`WORKER`, an interruption), and the recovery attempt repeats the same step
    under the same operation id with its new permit; the service returns what the first one saved.
- No operation of the list is ever "unverified" (`store_idempotent` is replay-safe), so `continueTask` answers `reconcile`
  and `needs_reconciliation` are not reachable through it. The host does not act on `reconcile` yet: there is no queryable
  tool to ask.
- Clients: `readList` / `writeList` are in the shared member contract (TypeScript, Swift and Kotlin validators, shared
  vectors) and in the Desktop allowlist. Web and Desktop: the Bot card has a "List: <title>" button for whoever may use the
  Bot; `SharedListDialog` adds, ticks, renames and removes items, one change per save on top of the revision that was read.
  A refused save (`LIST_REVISION_CONFLICT`, `LIST_ITEM_REQUIRED`) reads the list again and keeps what was typed; a repeated
  save reuses its operation id. The publish wizard has the list option, names the list in the review, and carries an existing
  list into the next publication unless the maintainer switches it off.
- iOS and Android: the Bot card (Team details, and the Bots of a friend chat) has "Shared list" for whoever may use the Bot
  and for its maintainer, while the Bot is published and the client is online. The screen (`TeamSharedListView.swift`,
  `TeamSharedListScreen.kt`) follows `SharedListDialog`: tick, rename, remove and add, one change per save on top of the
  revision that was read; a refused save reads the list again, says the list changed and keeps what was typed; a repeated
  save of the same change on the same revision reuses its operation id (`TeamSharedListOperation`); an item over 500 bytes
  is refused before it is sent. The list is read when the screen opens and on Refresh: it is not live. Tests:
  `Tests/TeamBotTests.swift`, `TeamBotsTest.kt`. Checked by compilation and unit tests only, not on a device or simulator.
- Known limit: `dispatchedAt` is the host clock (as `pausedAt` is). A host clock ahead of the service, or behind the start of
  the attempt, makes the record answer `TASK_EXPIRED`, which ends the run.

## M2 / Q05 — approving a list change

Scope: the controlled write is the Bot's own write to its shared list, and the approver is the member whose request the task
serves (B3, C7). Connection waits (B8) and budget waits are not built: no M2 tool needs a connection or a budget grant.

- Publication: `sharedList.approval` is `none` (default, absent) or `requester`. The card shows it; a list that asks needs a
  deployment with both `writeOperations` and `durableWait` (the Desktop publishes both exactly on a continuable route).
- Runtime (`harness-runtime`, `team_process`): a list published with `requester` offers `TeamListProposeChange` instead of
  `TeamListWrite`; the two names never share a transcript meaning. A well-formed proposal that is on the transcript file is the
  task's wait: `kind: "tool_approval"`, `waitId` derived from its logical step, `question` the proposed write as JSON, and a
  request digest under its own domain. The worker ends as `waiting` exactly as for a question and nothing reaches the host.
  One wait is open at a time, whichever tool opens it.
- Continuation: a waiting task continues only with the reply to that wait and of that kind (`answer.approved` present exactly
  for an approval). Approved: the write is sent with `approvalRef` = the wait of its own step; the parent process refuses any
  write whose `approvalRef` does not match the list's mode. Declined: the call returns `DECLINED` to the model and nothing is
  sent. An approved write that is interrupted repeats in the next attempt with the same step, operation id and approval.
- Desktop host: `pauseTask {kind: "tool_approval", approval: {operationId, logicalStepId, expectedDataRevision, changes}}`
  with the operation id the write will have and no free text. A proposal that is not a valid list change, or one on a list
  that does not ask, ends the task instead of asking anyone. `resume.decision` becomes `answer.approved`; a reply in words
  to an approval (or a decision on a question) continues nothing. `recordOperation` carries `approvalRef`;
  `APPROVAL_REQUIRED` from the ledger is told to the Bot as a refusal and nothing is sent.
- Service (already in the Q05 backend commit): the request message carries `waitId` and `approval {title, changes}` with each
  touched item as it read then; `decideWait {decision}` is accepted only from the wait's recipient and only for an approval
  wait (`answerWait` only for a question: `WAIT_KIND_MISMATCH`); `recordOperation` for a list that asks accepts the write only
  under an approved wait of the same task, operation id and parameter digest. A change of parameters, a declined, expired,
  cancelled or revoked wait all leave the list untouched. The list revision is still checked when the write is applied, so an
  approval of a change to a list that moved on saves nothing and the Bot is told.
- Clients (Web, Desktop, iOS, Android): `TaskWait.kind` is `human_input | tool_approval`. An approval request is never
  answered in words: the Reply entry is offered only for a question. Under the Bot's request a card lists each change in
  words (add, rename, tick, untick, remove; an item that was already gone is said so) with Approve and Decline for the
  recipient, "waiting for <member>" for everyone else, and afterwards how it ended (approved by, declined by, or ended
  without a decision). A decision keeps its command, message and change ids across retries. The request text and the reply
  (`Approved` / `Declined`) are stored in English and shown in the viewer's language; a decision cannot be edited or recalled
  from the clients. The publish wizard on Desktop has "Ask before the assistant changes the list".
- Not built: approvers other than the requester (design §10 allows an authorized approver set), approval of anything but the
  list write, and a member-side view of pending approvals outside the chat.

## M2 / Q07–Q08 clients — document editing, history and tasks that need checking

Member commands added to the shared contract (`packages/teams/contracts/execution-vectors.json`, validated identically by the
TypeScript, Swift and Kotlin validators): `readArtifact {artifactId, revision?}`, `listArtifactRevisions`, `reviseArtifact`,
`listOperations`, `resolveOperation`. `recordOperation` and `settleOperation` stay host-only and are rejected as member commands.
The Desktop main process allows the new member commands through `TeamsService.messages`.

Web and Desktop (`packages/teams`):

- `DocumentDialog.tsx` reads a shared document, edits its title and Markdown body, and lists its history. An edit is sent as
  `reviseArtifact` with the revision it was made from. One operation id belongs to one exact edit: a save whose answer was lost
  is repeated with the same id and creates no second revision; changed text or a changed base gets a new id.
- When someone saved first (`ARTIFACT_REVISION_CONFLICT`) nothing is saved. The person's text stays in the editor, the newer
  revision is shown under it, and the choice is "Save over revision N" or "Discard my changes". Closing with unsaved text asks first.
- History names the author of each revision (member or Bot) and opens any revision read-only; only the latest can be edited.
- A task in `needs_reconciliation` is labelled "Needs checking". Its requester and the Bot's maintainer can open the list of
  outside actions (`OperationsDialog`); only the maintainer gets "It happened" / "It did not happen" (`resolveOperation`).

iOS and Android:

- The document screen (`TeamArtifactViewer.swift`, `TeamArtifactScreen.kt`) follows the same rules as `DocumentDialog`: read,
  edit the title and Markdown body, history with the author and date of each revision, any revision opens read-only and only
  the latest can be edited. A refused save keeps the person's text above the newer revision with "Save over revision N" /
  "Discard my changes"; leaving with unsaved text asks first (the iOS sheet cannot be swiped away, Android Back asks).
  The edit identity and validity rules are `TeamDocumentOperation` / `TeamDocumentDraft` in `TeamDocuments.swift` and
  `TeamDocuments.kt`. Checked by compile and unit tests only, not on a device.
- A friend chat does not host the document screen on iOS or Android (it did not before either): "Open document" there shows nothing.
- A task in `needs_reconciliation` is labelled "Needs checking". Its requester and the Bot's maintainer get "View actions" /
  "Review actions" on the task card, which opens the list of outside actions (`TeamOperationsView.swift`,
  `TeamOperationsScreen.kt`); only the maintainer gets "It happened" / "It did not happen". After a decision, accepted or
  refused, the list and the task are read again. Who may look and who may decide is `TeamOperationRules` in
  `TeamOperations.swift` / `TeamOperations.kt`. Checked by compile and unit tests only, not on a device.

`resources/translations/*.json` now contains the 22 `settings_browser_*` keys that had been added to the generated iOS and
Android files only, so `generate.py` is lossless again.

## M2 / Q09 — fault injection against the service (emulator)

`apps/firebase/test/fault-matrix.emulator.test.mjs`, part of `npm run test:teams` (185 tests with it). A scripted host runs one
task with one tool step against the real service code and stops at each of the seven boundaries of acceptance C3: creating
the request, obtaining the permit, saving the checkpoint, dispatching the tool, receiving the tool result, writing the final
result, and the service's acknowledgement. A restarted host then does what the Desktop host does on start (`continueTask`
with `reason: 'recovery'`, settling a queryable operation first). Each boundary runs ten rounds over the four write
categories.

The tool reaches `Outside`, a stand-in system with its own ledger that the service cannot change. After every round the test
compares that ledger with the service's: a keyed write has exactly one effect however often it was called; a write that
cannot be repeated is called at most once and never by a recovery; an operation the ledger calls `succeeded` happened and
one it calls `failed` did not; the task is in the expected final state with no unverified operation, the Bot chat is free,
the host has nothing left to do, and there is at most one reply.

| Stopped at | What the rounds establish |
| --- | --- |
| Request | The same `submit` sent again, also concurrently, is one message, one task and one charge. |
| Permit | A granted `startTask` whose answer was lost is not granted again; recovery gets a new permit and the lost one is void. |
| Checkpoint | The replaced attempt's permit can neither record an operation nor deliver a result. |
| Tool dispatch | Nothing is called before the record is accepted. Recorded but not sent: a keyed write is sent by the next attempt; a queryable one is closed as not done after asking; an unqueryable one sends the task to the maintainer with zero calls. |
| Tool result | Called but not settled: a keyed write is repeated with one effect; a queryable one is confirmed and not called again; an unqueryable one is never called again and the maintainer closes it. |
| Final result | The next attempt gets the tool's receipt instead of a licence and delivers the result. |
| Acknowledgement | A delivered result answers a repeated delivery identically, refuses a different one, and grants no new attempt. |

Three further ten-round tests: two processes of one host racing for the permit, the recovery, the licence and the delivery
(exactly one of each); authority ending mid-run (the Bot paused, the requester removed, the requester's use withdrawn, the
host account changed), after which no tool call is admitted, no attempt is granted and the call already sent stays on record
until the maintainer says what happened (B5, C8); and a result racing a new Chat reader, which is published whole or not at
all.

What this does not cover: the Desktop host process and the runtime worker are scripted here, not killed. Their own recovery
is tested in `apps/electron/test/teams-host.test.ts` and the SDK's `team_continuation_process` tests, each against a stand-in
for the other side. A run with a signed Desktop package, real process kills and real devices has not been done: it needs the
SDK branch pushed and pinned, and nothing here is deployed.

## M3-1 — keeping, replacing and no longer sharing a Bot's files

A published Bot's files used to be all-or-nothing: publishing again meant picking every file again, and each pick became a new resource. A maintainer can now publish again and decide file by file.

**The rule (service).** `publish` compares the `inputs` it is given with the grants of the current publication:

| Input | Accepted when |
|---|---|
| a resource of the current publication, same digest | same `kind`, same `revision` |
| a resource of the current publication, different digest | same `kind`, `revision` + 1 |
| a resource the current publication does not have | `revision` 1 |
| a current resource that is left out | always: it is no longer shared |

Anything else is `INPUT_REVISION_CONFLICT` (409). A resource therefore keeps its identity for as long as it is published, its revision counts its contents, and a revision cannot be reused for other contents or skipped.

**Stopping (E1/E4).** Leaving a file out is a new Bot revision like any other publication. Tasks admitted under the earlier revision are refused at their next step (`TASK_REVOKED`), their result is not delivered (`authorization_required`), and a new task is prepared from the new revision's grants only, so the file is not among its sources. Nothing is indexed apart from the grants, so there is no second path to the file. Answers people already received stay in their chats.

**Desktop host.** `publishedKnowledge(teamId, sharedBotId)` returns what this Desktop published the Bot with: the role and, per file, `resourceId`, `revision`, `kind`, `name`, `sha256`, `bytes`. Texts stay in the private snapshot and are not returned. `publishLocalBot` takes `keptResources` (ids carried unchanged) and `replacedResources` (id → newly picked file). With either, the binding must be the Bot's current revision on this Desktop (`BOT_REVISION_CONFLICT`), each id must be in the current snapshot (`SHARED_RESOURCE_REQUIRED`), and a replacement must be of the same kind (`SHARED_RESOURCE_KIND`). A replacement with identical contents keeps its revision; otherwise it is the same resource at the next revision, and it keeps the package binding of the file it replaces. Carried and new texts together are limited as before (`SHARED_FILE_LIMIT`).

**Publish dialog (Web and Desktop share it; only Desktop can publish).** Choosing a Bot that is already published fills in its role and lists "Shared now" with Replace…, Undo replace, Stop sharing and Keep sharing per file. The review names each file as unchanged, replaced (with its next revision and digest) or no longer shared. A publication this Desktop does not hold at the Bot's current revision carries nothing, and the role and files are entered again.

**Tests.** Emulator `shared-bots.emulator.test.mjs`: two `knowledge:` tests (identity and revision rule; a task that began with a stopped source cannot continue or deliver, and a new task is not given the source). Electron `teams-host.test.ts`: keep, replace and stop sharing one by one. Web `teams-bots-ui.test.ts`: the dialog flow, the kind check, undoing a choice, and the unknown-revision case.

**Not done.** No signed Desktop package or device run. iOS and Android cannot publish, so they have no change. A file is replaced by picking a new one; editing a published file in place and a per-file history view are not offered.

## M3-2 — the Mac as a resident host (code part)

No new daemon. The host of published Bots already lives in the Desktop main process and checks for work every five seconds whether or not a window is open; the existing login item opens the signed app in the background. What was missing is that the two existing switches (Bots page → "Running in the background") did nothing for published Bots, that sleep and quit were only noticed when the heartbeat ran out, and that the maintainer could not see any of it.

**Keep awake.** `TeamConnector` takes the same `KeepAwake` as local Bots. It holds while a task runs and, on mains power, for as long as one of the Bots it publishes is online. On battery only a running task holds. Nothing holds when the switch is off (that is `KeepAwake`'s rule), after an account change, or once the host rests or closes. Closing the lid still puts the Mac to sleep.

**The five states.**

| What happens | Host | What people see |
|---|---|---|
| Last window closed | keeps serving | nothing changes |
| Quit | `rest()` (at most 1.5 s), then Teams stops; running tasks are interrupted and continued from their checkpoint at the next start | unavailable at once |
| Sleep (`suspend`) | `rest()`; running tasks are left alone | unavailable at once |
| Wake (`resume`) | `awake()`: a pass at once, heartbeat first | available again |
| Sign-out or another account | `revoke()` as before | unavailable when the heartbeat ends |
| Network lost | passes fail and are repeated; the state is `unreachable` | unavailable when the heartbeat ends (90 s) |

`rest()` sends `heartbeatDeployment {ready: false}` for each online Bot, so a request is refused (`HOST_UNAVAILABLE`) instead of being accepted and left waiting. It is best effort: without an answer the heartbeat ends by itself. No pass runs and no Bot is said to be online while resting.

**Status (`hostStatus`).** Per Bot this Mac publishes: `checking`, `ready`, `not_ready` with what its maintainer repairs (`bot`: the local Bot is gone, `runtime`: this app cannot run it, `model`: its model account), `unreachable`, `stopped` (paused, published again elsewhere, team left), and the number of requests in progress. With it: the login item as the system has it, the keep-awake switch and whether the Mac is on battery. Memory only.

**"This Mac" in the Bot directory (Desktop, team and friendship).** Shown to a person who maintains a published Bot there: one line per Bot, what continues and what stops the service, and the two switches with what is wrong when one is off (not opened after a restart; may sleep when idle; on battery; the system wants the login item allowed, with a button to Login Items). A Bot that another computer publishes is named as such and offers no switches. The switches are the ones of the Bots page; the keep-awake hint there now mentions shared Bots.

**Tests.** Electron `teams-host.test.ts` (three `M3-2` tests: holds for a task and for an online Bot on mains, battery, account change; rest/awake and an unanswering service; every status and reason), `bot-login-item.test.ts` (hint). Web `teams-bots-ui.test.ts` (the section, both switches, approval, a reason, a Bot served elsewhere, a person who maintains nothing).

**Not done — needs a signed package and a real Mac.** F1 on a signed app after login with no window; the sleep, quit, sign-out and offline rows observed on a device; F2, the twenty-four hour run with a fixture request every ten minutes; F3 after a real restart; F5. A Mac that is asleep, shut down or has LingXi quit still answers nobody, and the lid still wins over keep-awake.

## M3-3 — handing a Bot to another member, and handing over the team

Two separate things, each moved by its own command. Maintaining a Bot, owning the team, paying for the model and the audience of the data never move together (E5).

**What a Bot handover moves.** The maintainer, and with it the deployment that answers and the model account that pays. The published role and the files of the current publication go to the new maintainer as a package. The Bot keeps its `sharedBotId`, its name, who may use it, its daily limits and its shared list; its revision goes up by one.

**What it does not move.** The earlier maintainer's local Bot, model account and anything else on their Mac. Conversations: each keeps the readers it was created with, so the new maintainer reads none of the earlier ones and the earlier maintainer reads none of the new ones. Results and documents already delivered stay readable to the people who could read them. What a task did outside the Chat stays with the maintainer it ran under (`task.maintainer`): only they see and resolve its operations.

**Steps and states.** `offered → accepted → completed`, or one of `cancelled` (taken back), `declined`, `expired` (seven days), `void` (a new publication, a pause, or either party leaving the team). Until `completed` nothing changes for the people who use the Bot. Every step has one answer that can be asked for again.

| Step | Who | Command | Notes |
|---|---|---|---|
| Upload the package | maintainer's Desktop | `putTransferPart` | JSON `{schemaVersion, documents}` in base64 parts of 700 KiB, at most 8; sealed by the offer |
| Offer | maintainer | `offerTransfer` (`commandId` = `transferId`) | only to a member who may use the Bot (`INPUT_GRANT_REQUIRED` otherwise); one open offer per Bot (`TRANSFER_PENDING`) |
| Accept | recipient's Desktop | `acceptTransfer`, then `readTransferPart` | parts are readable by the accepted recipient only |
| Complete | recipient's Desktop | `completeTransfer` | new deployment, old one disabled, parts deleted |
| End | either party | `endTransfer` | taken back by the maintainer, declined by the recipient |

**After it.** The earlier deployment gets `DEPLOYMENT_REQUIRED` for everything, and a run it had in progress is not carried over. A request in a Chat from before the handover is refused with `BOT_MAINTAINER_CHANGED`: the Chat's readers were fixed when it was made, and the new maintainer is not one of them. People start a new chat with the Bot.

**Desktop (recipient).** Accept → read the parts → check the package against the manifest (same documents, same digests, exactly one role; `TRANSFER_PACKAGE_MISMATCH` otherwise) → take the model route and accounts of the local Bot the recipient chose → check that the model answers → save a pending publication → `completeTransfer` → adopt it. The chosen Bot's model answers and the published role is used in place of its own. A lost answer at any step is repeated with the same command, by the same call or at the next start. A tampered package, a list the chosen model route cannot continue (`SHARED_LIST_UNSUPPORTED`) or a model that does not answer leave the Bot where it is.

**Clients.**

| | Maintainer | Recipient | Everyone else |
|---|---|---|---|
| Desktop | "Hand over…" on the Bot's card: who, what goes, what stays, a confirmation; then "Offered to … / Take back" | the offer with "Take over…" (choose a local Bot, costs, confirmation) and "Decline" | nothing |
| Web | "Offered to … / Take back" (the offer itself needs the Desktop that publishes the Bot) | the offer, "Decline", and where to take it over | nothing |
| iOS, Android | nothing | nothing | nothing |

On all four, a Bot chat from before the handover shows why it no longer answers, and the Bot list opens a new chat instead of that one. Web and Desktop add a "Start a new chat" button there. The rule is the server's: the Chat is a Bot DM whose readers do not include the current maintainer, or include someone who is neither the maintainer nor the person in it. Handover is not offered in a friendship.

**Handing over the team.** `handOver {teamId, uid, generation}` on the team service: the owner gives the team to a current member and stays as an admin. It takes effect at once, has no offer step, and changes two roles and nothing else — no Bot, deployment, Chat reader, grant or quota. Only the owner can (`OWNER_REQUIRED`); not to oneself (`ROLE_REQUIRED`); a member whose membership changed is refused (`STALE_MEMBERSHIP`); asked again after a lost answer it changes nothing. Friendships refuse it like every member command (`PAIR_TEAM`). "Make owner" is on each member for the owner, with what does and does not change in the confirmation: Web and Desktop in the details drawer, iOS in the member's menu, Android in a new per-member menu that also holds the role change and removal.

**Tests.** Emulator `handover.emulator.test.mjs` (7: E2 and RV-06, E3 repeats, each way an offer ends, a party leaving, E5 including the team's owner changing, operations, documents) and `teams.members.emulator.test.mjs` (the owner's handover); rules deny clients `transfers` and `transferParts`. Electron `teams-host.test.ts` (three `M3-3` tests). Web `teams-bots-ui.test.ts` (four `M3-3` scenarios) and `teams-workspace-interaction.test.ts` (the owner's confirmation). iOS `TeamsListLogicTests`, `TeamsPointerParserTests`; Android `TeamsListLogicTest`.

**Not done.** Nothing here has run on a device or against the deployed project, and nothing is deployed. Native clients show no offer: the maintainer and the recipient use Desktop or Web. The owner control on iOS and Android is compiled and type-checked but has no UI test. A handover does not move the task history of the earlier deployment, by design.

## M3-4 — files in a Team message, and voice in the phone composer (service, Web, Desktop, iOS, Android, the Bot's model)

A member attaches pictures and documents to a Team message. Everyone who reads the conversation can open them, and a Bot that is asked gets them as real model input. On a phone a member can also speak a message into the box. What is not done is at the end.

**Limits.** PNG, JPEG, PDF and plain text. 10 MiB a file, 5 files and 20 MiB a message. What a member has uploaded and not yet sent: at most 10 files and 40 MiB, kept for 24 hours.

**Where the bytes are.** In Firestore, as base64 parts of 700 KiB under `teams/{team}/uploads/{uploadId}/parts/{index}`; the rules deny every client read and write there. The design names a private object store; the app has no Cloud Storage bucket today, so this uses the database that is already there. Only `team-attachments.mjs` puts, reads and deletes parts, so moving them to a bucket later changes that one file. **This is a deviation to confirm.**

**The upload record** `teams/{team}/uploads/{uploadId}`: conversation, uploader, name, type, length, SHA-256, and a state.

| State | Meaning |
|---|---|
| `uploading` | parts are arriving |
| `verified` | the service read every part and the whole matches the declared length, digest and type (by its first bytes); only this state can go into a message |
| `attached` | a message carries it; the id is now the `attachmentId` |
| `failed` | it was not what it said; its parts are deleted |
| `revoked` | its sender took it back, or recalled the message; its parts are deleted |

**Commands** (all through the member command endpoint; each can be asked again and gives the same answer).

| Command | Who | Notes |
|---|---|---|
| `createUpload` | a participant of the conversation | `UPLOAD_QUOTA_EXCEEDED`, `UPLOAD_ID_CONFLICT` |
| `putUploadPart` | the uploader | `UPLOAD_PART_INVALID`, `UPLOAD_SEALED`, `UPLOAD_EXPIRED` |
| `finalizeUpload` | the uploader | answers `verified`, or `failed` with `UPLOAD_INCOMPLETE`, `UPLOAD_DIGEST_MISMATCH` or `UPLOAD_TYPE_MISMATCH` |
| `cancelUpload` | the uploader | a file taken out of a draft is gone at once |
| `submit` with `attachments` | the uploader | 1–5 verified uploads of this sender for this conversation; the text and the Bot targets may be empty (`ATTACHMENT_REQUIRED`, `ATTACHMENT_LIMIT`) |
| `readAttachmentPart` | whoever reads the conversation now | `ATTACHMENT_REVOKED` after a take-back or a recall; a member who left gets `MEMBERSHIP_REQUIRED` |
| `revokeAttachment` | the sender | one change to the message: the file stays listed as taken back |

A message stores `attachments: [{attachmentId, name, mime, byteLength, sha256, revoked?}]`. There are no download links: every read goes through the service and is checked against the conversation's readers at that moment (L4). A daily job removes uploads no message took.

**Web and Desktop.** The composer has an attach button and takes dropped and pasted files. Each file shows its progress, can be removed, and a refusal says why in words (type, empty, too large, too many, too much together). Sending waits for every file to be checked. A draft and its uploads belong to one conversation: switching conversations keeps them there and never sends them elsewhere (L3). In a message, pictures show inline, text opens in place, and any file can be saved; the sender can take a file back after a confirmation. Desktop reuses the Web components; its main process allows the six commands.

**The Bot's model (Desktop host and runtime).**

1. When a task starts, the host chooses files: those of the message that asked, then of the message it answers, then of the other messages the Bot has not seen, newest first — together at most 5 files and 20 MiB. Files taken back are not chosen.
2. The host reads each one through `readAttachmentPart` as the Bot's maintainer (who reads the conversation), and checks length and digest. A file that was taken back in the meantime is left out. Any other failure ends the try before a permit is spent, and the task is tried again.
3. The batch handed to the worker lists the files with their bytes (`files`, in the private transport file). Each message still names its own files in `attachments`.
4. The worker checks every file again, takes the bytes out of the batch before anything is journaled, and writes them into the transcript row of the asking message: a PNG or JPEG as an image block, a PDF as a document block, a text file as text (at most 256 KiB of it, with `shownBytes` saying where it was cut). Each file is preceded by a label with its message, name, type, length and digest.
5. A batch whose files do not match what it says is refused before anything is kept.
6. A finished run sends `readFiles: [{attachmentId, sha256}]` with `completeTask`. The service keeps the entries that are files of that conversation with that digest, writes them on the task, and puts `readFiles: [{attachmentId, name}]` on the Bot's reply. Web and Desktop show "Read ticket.png, itinerary.pdf" over the reply. A failed run records none.

Things that follow from existing rules:

- Taking a file back, like editing, changes the asking message. A run in progress for that message publishes nothing (`TASK_REVOKED`).
- After a message changes, the Bot's history is rebuilt from what the messages say now. The rebuilt history names earlier files but no longer contains their bytes.
- The bytes of files a Bot was given stay in that Bot's transcript on the maintainer's Mac, as recalled text already does.
- A Team Bot's transcript now accepts rows up to 48 MiB (other transcripts keep 2 MiB).

**Tests.** Emulator `attachments.emulator.test.mjs` (9: upload and read back by each reader, wrong or cut files, limits, repeats, quota and cleanup, take-back, recall and leaving, a Bot request with files, the read-files record) and the rules test. Firebase unit contract test. Web `teams-attachments.test.ts` (11). Electron `teams-host.test.ts` (3 for files: order and evidence, limits, failures) and `teams-chat-source.test.ts` (1). Runtime `team_invocation_process` (3: files in the real model request and across later tasks, a row over 2 MiB, refused batches).

**iOS and Android.** Both have the same rules as `packages/teams/attachments.ts` (`TeamAttachments.swift`, `TeamAttachments.kt`) and the same command checks.

- Choosing: iOS offers the photo library and Files; Android the system file chooser. A picture that is not PNG or JPEG (HEIC, WebP), or one over 10 MiB, is made into a JPEG of at most 4096 pixels a side before it is sent. Anything else that is not one of the four types is refused with the reason.
- The workspace owns the uploads. A draft's files are kept by account, team, generation and conversation, next to its text. An upload reports to the draft it was started for only; it ends when its team is closed, and its file then leaves the draft (checked files stay). Everything is dropped on sign-out or an account change.
- Sending reads the files of the open conversation's draft at that moment. An edit and an answer to a Bot's question carry no files; the files wait in the draft. A message refused for good gives its uploads up when it is discarded.
- A message's pictures are shown small in the stream; the small copies are held in memory only while the team (iOS) or the conversation (Android) is open. Opening a file asks the service for it each time. iOS shows it in Quick Look and deletes its copy when the preview closes. Android writes it under `cache/team-files` for the app that shows it and deletes it before the next file is opened and on an account change.
- The sender can take a file back after a confirmation. A Bot's reply names the files its model was given.

Tests: iOS `TeamAttachmentsTests` (12, on macOS) and the whole-app type check; Android `TeamAttachmentsTest` (10) and two model tests in `TeamMessagingQueueTest` (upload in parts with a busy service, send, read back, take back; refusals and files never queued for another conversation, an edit, an answer or unchecked).

**Voice on iOS and Android.** The Team composer has a microphone button. It uses the app's existing voice input and adds no recorder or recognizer: on iOS `VoiceInteractionController` with its panel and its setup banner, on Android `rememberVoiceCapture` with its setup notice. No audio is uploaded or kept in the team's history; only the text the member sends is.

- What is heard is added to the text in the box, after what is already there. It is never sent from there: the member reads it, changes it and presses send. On iOS the words arrive when the member ends the recording; on Android what has been heard so far is shown above the box while the microphone listens, and goes into the box at the end.
- Cancelling (closing the panel, leaving the conversation, the app going to the background on Android) adds nothing and sends nothing.
- When the microphone cannot be used (permission refused, voice not set up, a failure), a notice says so and links to the voice settings, which return to the Teams or Friends tab. The box stays usable for typing the whole time.
- Words belong to the box they were spoken into: the account, the team as the member is in it, the conversation, and whether a message is being changed. If any of these changes, the recording is cancelled and a transcript that still arrives is dropped. Starting or ending an edit cancels it too.
- Android: the app's held-capture registry now keeps every capture that is attached rather than the last one, so opening a Team conversation does not unregister the chat's capture. The chat screen no longer shows a failure notice for a capture another tab was using.

Tests: the joining rule on both phones (`TeamsComposeLogic.appendingTranscript`), and on Android the rules for which box words reach and for how a recording that ends by itself is treated (`TeamVoiceLogic`). iOS logic tests 68, Android Teams tests 105, and the iOS whole-app type check pass.

**Not done.**

- Voice has not been used on a phone: L1 (record, change the text, send, cancel, refuse the permission) and L5 (accuracy on voice samples) need real devices. No test drives the microphone or the recognizer.
- The phone UI has no UI test and has not run on a device or simulator.
- Nothing ran in a real browser, the Desktop app, on a phone, or against a real model. No real provider has been sent a Team file.
- Files at the full 20 MiB were not run through the worker. The test uses one 1.6 MiB file, just over the old row limit: the unoptimized worker needs about a minute and a half per run for 3.5 MiB of files, and a 13 MiB run did not finish in five minutes. The time grows with the size of the rows in the transcript and is paid again by every later task of that conversation. It has not been measured in an optimized build.
- Nothing is deployed. The runtime change is on its own branch and the app does not pin it yet.

## M3-5 — moving an installed Bot to a newer release (Desktop)

A Bot installed from the market stays on the release it was installed as. Its owner can now move it to the newest release of the same listing, and what they changed locally is kept (design 16.4, I8 and I13). Only Desktop installs market Bots, so only Desktop has this.

**What counts as the local change.** The Bot keeps the manifest of its installed release in `market-package.json`. The role (`home/LINGXI.md`) and each packaged Skill (`home/skills/<name>/SKILL.md`) are compared by SHA-256 with that manifest and with the new release. Per file:

| The release | The owner | Result |
|---|---|---|
| did not change it | did not change it | nothing |
| did not change it | changed or deleted it | the owner's stays (`kept`) |
| changed, added or removed it | left it as installed | follows the release (`update`, `add`, `remove`) |
| changed it | made the same change | nothing |
| changed, added or removed it | changed it differently | `conflict` |

A Skill is the same Skill across releases by its name. A file in conflict is shown with both texts and the owner picks "Keep mine" or "Use the new release's" for each one. The merge is per file; lines inside a file are not merged.

**Looking and deciding.** "Check for a new release" (Bot, Market tab) downloads and verifies the newest release exactly as an install does (`marketRelease.ts`, now shared by both) and returns a plan: the files above, a license change, and the tools and resources added, removed or newly required. Looking writes nothing. The plan carries a digest of everything it was made from: both releases, the Bot's files, its record revision and the outcome per file.

**Carrying it out.** `upgrade` rebuilds the plan and refuses, with nothing written, when:

- the plan's digest or the Bot's revision is not the reviewed one (`BOT_UPGRADE_CHANGED`): the owner looks again;
- a conflict has no choice (`BOT_UPGRADE_CHOICE_REQUIRED`);
- the release adds a tool or a resource and the owner did not agree (`BOT_UPGRADE_APPROVAL_REQUIRED`). A resource that only becomes required, and anything removed, needs no agreement;
- the signed-in account or the market is not the one the Bot was installed with.

Then it writes the files that follow the release, the new `market-package.json`, and the record (`marketOrigin.releaseId` and `digest`, revision + 1). The name and the description follow the release only while they are still the ones the installed release gave. If the app quits between the files and the record, the next check finds the files already in place and finishes.

**What is never touched.** Skills the owner added, files beside a packaged Skill, memory, conversations, settings, the model, workspaces. Declining ("Stay on …") is doing nothing. An upgrade is never a second copy of the Bot.

**Teams (I13).** A Team runs the snapshot a Bot was published with, not the Bot's folder. An upgrade does not stop running Team tasks and does not change what a Team is served; the new release reaches a Team when the maintainer publishes the Bot again, which is a new Bot revision with its own confirmation. Updating authorizes nothing by itself: the owner still binds their own resources when sharing.

**Tests.** Electron `bot-market-upgrade.test.ts` (7): the per-file rule; an upgrade that updates, adds and keeps, with the owner's Skill, memory, model and name untouched and no Team task stopped; conflicts refused until each is chosen, then carried out, and kept choices surviving the next release; added access refused without agreement, removed access not asking; a stale plan, another account, a corrupt download and a Bot not from the market; removal and an interrupted upgrade; the review's text and its disabled button. `bot-market-package.test.ts` (9) still passes on the shared release code.

**Not done.**

- Nothing ran against the real market or in a signed Desktop app. The upgrade uses the market's existing routes; nothing on the service changed.
- The app does not look for new releases by itself and shows no badge: the owner presses the button.
- Lines inside one file are not merged. Cost conditions are not part of a Bot package, so they are not listed.
- A Team is not told that a Bot it uses has a newer release.
- The review's text is English only, like the rest of the Market tab.

## M3-6 — a Bot offered outside Teams: the hosted Bot service (service, Desktop host, screens on all four clients)

A publisher can offer one of their Bots to people who are in none of their Teams (design 17, HostedBotService; I7 and I11, and the online parts of I2 and I9). This is not the local BotService: that one manages Bot instances on a device, and a hosted service reuses it only for that. Done so far: the service layer in `apps/firebase`, the Desktop host, the screens on Web and Desktop, and the caller's screens on iOS and Android.

**A scope of its own.** A service is `services/{serviceId}`. Callers are never added to a Team and no Team generation is made up for them. The root has the shape the execution ledger works in, so `shared-bots.mjs` runs inside it unchanged: admission, daily quotas, permits, waits, operations and task states are the same code as in a Team.

| Under `services/{serviceId}` | What it is |
|---|---|
| the service record | publisher, name, description, audience, limits, retention text, `online`, `cost: 'publisher'`, `visibility: 'requester_and_publisher'` |
| `members/{uid}` | the publisher (`owner`) and each caller's entitlement (`member`, with a generation) |
| `entitlements/{uid}` | the last generation a caller held |
| `active/{uid}`, `active/_service` | the tasks counted against the concurrency limits |
| `sharedBots`, `deployments`, `conversations`, `tasks`, `requestQuotas`, … | what the ledger writes; a conversation is one session |

Beside it: `serviceUsers/{uid}/entered/{serviceId}` (what a person entered), `serviceUsers/{uid}/reportDays/{day}` and `serviceReports/{id}`. The Firestore rules deny all of this to clients; everything goes through the API.

**Ids.** A service id starts with `s_`. `createService` takes no other id and no Team can be created with one (`RESERVED_TEAM_ID`), so an id alone says which scope it names. This is what lets the Desktop host keep one kind of record for both.

**Commands.** `POST /v1/services` (its own hosting rewrite) takes `createService`, `updateService`, `setServiceOnline`, `withdrawService`, `getService`, `enterService`, `leaveService`, `reportService` and `listServices`. Sessions and tasks use the existing ledger and message routes with `serviceId` in place of `teamId`. A command names one scope: one with both is refused, a service id used as a Team id is `MEMBERSHIP_REQUIRED`, and a Team id used as a service id is `NOT_FOUND`.

**Only what the scope lists.** A caller has `createConversation`, `submit`, `answerWait`, `decideWait`, `cancelTask`, `getTask`, `listTasks`, and reads with `list`, `sync` and `read`. The publisher has the publishing and host commands, `listSharedBots` (its host checks the one Bot it serves) and reads. Everything else is `SERVICE_SCOPE_UNSUPPORTED`: the Bot directory, shared lists, documents, handover, conversation policy, attachments, writing a plain message. No command falls back to the publisher's authority when it is not listed. The publication must be the service's one Bot, usable by everyone in the scope, with no shared list (`INVALID_SERVICE_PUBLICATION`). A session is a `bot_dm` with that Bot (`INVALID_SERVICE_SESSION`), at most 50 per caller.

**The gates, in the order a request meets them.**

1. Signed in (the API's token check).
2. Audience: `public`, or up to 200 named accounts. To anyone the audience does not name, the service answers `NOT_FOUND`, the same as an id that does not exist.
3. Entry: `enterService` needs `acknowledged: true`. Before that the caller is shown who pays (the publisher), who can read a session (the caller and the publisher) and what is kept (the publisher's retention text); `getService` returns these. Without entry: `SERVICE_ENTRY_REQUIRED`.
4. The off switch: a new service is off. While off, `submit`, `startTask` and `continueTask` are `SERVICE_DISABLED`; reading stays.
5. Daily quotas: the ledger's own counters in this scope. `quotas.team` is the service's day, `quotas.member` each caller's (`REQUEST_QUOTA_EXCEEDED`).
6. Concurrency: `limits.userConcurrent` (1–5) per caller across their sessions and `limits.serviceConcurrent` (1–50) for the service (`SERVICE_USER_BUSY`, `SERVICE_BUSY`, both 429). A refused request leaves no message, no task and no quota use. Sending the same message again is not a second request.

`getService` gives a caller one `availability`: `withdrawn`, `disabled`, `not_ready` (no published Bot), `host_offline`, `service_quota_exhausted`, `user_quota_exhausted` or `available`, and their daily allowance left. It gives the publisher's display name and nothing else of the publisher.

**Isolation.** A caller lists and reads their own sessions only. Another caller's session is `CONVERSATION_REQUIRED` and another caller's task answers as a task that does not exist. The publisher reads every session, as each caller was told, and cannot write in one.

**Losing access.** Narrowing the audience refuses the people it leaves out at once, then removes their entitlements; a task of theirs that had not started is `TASK_REVOKED`. `leaveService` is the caller's own way out. Entering again is a new generation and the old one is `STALE_MEMBERSHIP`; the caller's earlier sessions are theirs again. `withdrawService` is final: every command is `SERVICE_WITHDRAWN` (410) and the service leaves every caller's list; the publisher still sees it, as withdrawn.

**Reports.** `reportService` writes to `serviceReports` with the reporter, the publisher, a reason and optional text and session (only the reporter's own session). Five per person per day (`REPORT_LIMIT`), idempotent by `reportId`. A report is not in anything the publisher can ask for, and can be made after a service is withdrawn.

**The host (Desktop).** The Bot is published into the service's scope by the same connector that serves Teams. A service binding is a Team binding whose scope id is a service id:

- every host command is sent with `serviceId` in place of `teamId`;
- the binding is not looked for among the account's Teams: the publisher is the service's owner at generation 1 for as long as it exists;
- the session is read through the API (`createServiceChatReader` in `packages/teams/hosted-service.ts`), with the same requests and the same checks as the Team reader. It follows the session's change log from the start, keeps it in memory, and afterwards asks only for what changed. A read that does not add up (a gap in the log, a missing message, an author that is neither a person nor the Bot) is refused and the next read starts over;
- nothing of a service can be watched, so new requests are found on the connector's own timer (5 s);
- a withdrawn service answers `SERVICE_WITHDRAWN` once; its running request is stopped and it is asked nothing more until the account starts again.

**Screens (Web and Desktop).** One component, `packages/teams/HostedServices.tsx`, on both. Web shows it at `/services` and `/services/<id>` (the link a publisher hands out), in the Teams security zone: the same content policy, never cached, no referrer. Desktop shows it in the Teams page, from "Bot services" at the foot of the Teams sidebar.

- *Using one.* A link or an id is looked up and its terms are shown: who offers it, that it is not a Team, who can read a session, who pays, what the publisher says is kept, how many requests a day and at once, and that it can be reported and left. Nothing is entered by looking; "Agree and open" sends the acknowledgement. A publisher who opens their own link is told it is theirs and shown no terms. Then the person has their sessions with the Bot: a new session, a request, the answer (the Bot's text through the host's Markdown renderer, the same one the Team chat uses; a person's text as typed), the Bot's question answered in the same box, approve or decline, cancel. A refusal is said in words, with "Nothing was sent" where that is so, and the text stays in the box. The page reads an open session every 2 s while the Bot is answering and every 10 s otherwise.
- *Reporting and leaving.* The report dialog says it goes to LingXi and not to the publisher, and names the open session only when the person ticks that. Leaving is confirmed.
- *Offering one (Desktop only).* The publisher picks a local Bot, the name and description people see, the published role, daily requests per person and in all, requests at once per person and in all, and their retention statement, and accepts in a checkbox that this computer answers, that they pay, and that they can read every session. The service is created off, the Bot is published into it, and the publisher turns it on. If the publication fails, the service is withdrawn again so that nothing is left that can answer nobody. Each service shows whether it is on, whether this computer is answering, its link, and has "Turn on/off" and "Withdraw" (confirmed, final).

**Screens (iOS and Android).** "Bot services" in the Team switcher and on the no-teams screen opens a full-screen sheet with the caller's side only: the services the person uses, a link or id looked up with the same terms, "Agree and open", sessions (ask, the Bot's answer through the chat's Markdown renderer with web links only, answer its question, approve or decline, cancel), report and leave from the menu. A phone hosts and offers nothing; a publisher who opens their own link is told it is theirs. The open session is read on the same 2 s / 10 s timer and not at all while the app is in the background. A page that does not continue the change log, or carries a message of anyone but the person and the service's Bot, is refused whole and the session is read again from the start. Another account closes everything (iOS: the auth listener; Android: the screen is keyed by the account). The words are in `resources/translations` (`services_*`, five languages).

A page's commands are checked before they leave: Desktop main and the Web API send a session or task command only when it is one of the caller's seven actions or three reads, in a service scope, with no Team named (`isServiceMessageCommand`).

**Tests.**

- Service: `hosted-bot-service.emulator.test.mjs` (7), part of `npm run test:teams`: off by default and each availability; entry, a full request and answer, and who reads a session; the scope's refusals, the two id spaces and the Team/service mix-ups; the switch, quota and both concurrency limits; narrowing, leaving and re-entry; withdrawal and reports; the HTTP route. The whole suite: 211 + 10 emulator tests and 64 unit tests pass.
- Desktop: `teams-host.test.ts` (4 new, 96 in all): a service Bot is published and served with `serviceId` on every command and never a Team; the Desktop service reads a session through the API and watches nothing; a withdrawn service stops its request and is asked nothing more; a page's command leaves only when it is a caller's. `hosted-service.test.ts` (5): the id space, the session reader (every request kind, paging, edits, refusals, starting over), the transport.
- iOS: `HostedServiceTests` (7, macOS): links, the words for every availability, refusal and ended request in all five languages, telling a service from the publisher's own, the change log and its refusals, commands built and checked like their Team counterparts with `serviceId`, report and the acknowledgement. Whole-app differential type-check: no new diagnostics.
- Android: `HostedServiceTest` (9): the same rules, and the screen's model against a fake service (looking enters nothing; agreeing opens it; a refused first request is sent again into the same session; the Bot's question is answered; withdrawal closes the sessions; another account sees nothing of the first; a report carries the session only when chosen; leaving).
- Web: `hosted-services-ui.test.ts` (7): links; the terms and that looking enters nothing; asking, a refusal, the answer, the Bot's question, cancelling; a service that cannot answer; report and leave; offering, a failed publication, the switch and withdrawal. `remoteHosting.test.ts` covers the new route and its policy.

**Not done.**

- Nothing is deployed, and nothing ran against the real project, in a signed Desktop app, in a real browser or with a real model. Every check above is a fake or the emulator.
- iOS and Android have no publisher side and are not reached by a `/services/<id>` link yet (no app link or universal link): the person pastes the link.
- No directory: a person reaches a service by its link. The list shows what they entered and what they offer.
- The server can offer a service to named accounts only; the screen offers that choice only when it is handed a list of people to pick from, and neither host passes one yet. A service made in the screen is open to anyone with the link.
- A service cannot be edited after it is made (name, limits, audience, retention): the server takes `updateService`, the screen does not send it.
- A service Bot carries its role only. Files and Skills, and Bots that need files bound, cannot be offered.
- Nobody reads reports through the product; there is no review tool, and no automatic action on a reported service.
- Files in a service session are refused.
- Cost is always the publisher's. A caller paying, or a split, is not modelled.
- Retention is the publisher's statement shown to the caller; nothing deletes sessions on a schedule.
- A caller who was removed cannot read or export their earlier sessions while they are out.
- The publisher has no screen for reading callers' sessions, although the service lets them and callers are told so.
- A session is polled, not pushed: an answer shows up to 2 s late, and a new request reaches the publisher's computer up to 5 s late.
- The screen's text is in the component (English and Chinese), like the other Teams screens, not in the translation files.

## Current integration status, 2026-11-05

The sections below record earlier increments and their failures; this table supersedes their pending source-work statements. M0/M1 release acceptance remains open.

| Area | Current evidence | Remaining gate |
| --- | --- | --- |
| Backend | Execution 2 only; old claim/start/finish module removed. Strict commands, atomic admission/quotas, single-use start, durable host fence, source-edit fence, grants, artifact/current-reader authorization and generic pushes implemented. Contract/gateway/eligibility unit suite: 15 passed. Latest full Emulator run after review corrections and cleanup: 85 backend plus eight SDK/UI tests passed, zero skipped (`/tmp/teams-root-all-final.log`). | Source review passed; deployed full-business integration remains. |
| Shared Chat source | Existing UID/project SDK cache reused; exact latest-200 bootstrap, complete deltas/history and edits/recalls. Nine reader tests cover cache sharing and appends during paged reads. Final real-worker suite: 15 passed; Desktop host→worker proved 200→450 input, reopen and exact result recovery with two model calls. | Signed release/current-device and real-provider acceptance. |
| Shared UI/Web | Member home, publisher wizard, task/artifact views and invitation continuation use new contracts. 135 focused tests passed; full Web 395 passed plus three Emulator-only cases separately passed. Typecheck/build passed. | Real new-account/model/device acceptance. |
| Native | New Teams contracts/cards/history/artifacts; pure Swift 57 and Kotlin 48 tests passed. Final current iOS simulator source build passed; actual simulator XCTest ran 75 cases, zero failures/skips (`/tmp/native-teams-ios-xctest.log`). | Current Android JNI+Kotlin generated together with Page; DirectDebug APK and1889unit tests pass, installed fresh onSamsung with empty model providers. Samsung member login is paused at user request while phone is in use. Fresh iOSFFI/package and physical full business acceptance remain. |
| Bot market/install | Strict manifests, authenticated v3 content, four audience policies, version-pinned account-bound install receipts and post-commit Desktop ACK implemented. Market 85 unit tests, six Emulator tests and two browser integrations passed. | Authorized cloud candidate passed30 grouped content/Range/revocation checks with dedicated service identity; actual Desktop installation/handoff and final promotion remain. |
| Production account domain | AUTH-DOMAIN-01 passed with explicitly authorized temporary account: real signed Firebase token, Teams custom-token exchange and Market identify the same UID. Project/audience `lingxi-5e4b2`, issuer `https://securetoken.google.com/lingxi-5e4b2`, tenant null. Temporary Auth account deletion succeeded; receipt stores no token/password/UID. | This proves account domain only. It does not establish Team service identity or content authorization. |
| Deletion | Product and worker source scans have zero retired matches. Old shell guardian and five runner branches removed. Cloud cleanup tests passed; explicit local inspect/apply has 15 passing CAS/link/ownership/preservation tests, with no startup reader/adapter. | Final package/export audit and actual coordinated cutover. No existing production/user data has been deleted. |

AUTH-DOMAIN receipt: `/Users/luolingfeng/lingxi/.omx/plans/evidence/2026-11-05-m0-m1/auth-domain-01-attempt1.json`. The experiment deletes the temporary Auth account; it does not claim deletion of every possible Market profile side effect.

After a separate explicit user authorization, the read-only production cleanup inventory found three Teams and six retired roots: two old bindings and four conversations containing old Bot principals. Candidate `/Users/luolingfeng/lingxi/.omx/plans/evidence/2026-11-05-m0-m1/production-cleanup-candidate.json` is mode 0600, stores paths/update stamps only, and has digest `a32df10c721646aaae31d931419f72a2107c23edeeac6476f0eb356eaf97afd0`. This authorization covers inventory only. No production deletion was performed; the reviewed candidate must be revalidated after stopping old writers before any cutover.

Disk exhaustion interrupted the newest Rust build and initially prevented the final Emulator run. Bounded inactive compiler-library caches were later reclaimed under explicit file manifests, CAS and the Cargo lock; source, executables/dylibs, packages, evidence and user data were preserved. The final host/worker digests were checked unchanged. Failed checks must be rerun; no acceptance is inferred from earlier builds or local sibling source while product dependency pins still name an older SDK revision.

Independent architect review identified four P1 and one P2; all source findings were re-reviewed and closed. The later busy-Chat watermark refinement also passed source review. Host regressions cover 26 cases; installer concurrency/recovery nine. Actual worker testing found and fixed stack overflow, append-once replay identity and read-tool metadata/permission issues; synthetic provider fixtures now issue unique response/tool-call IDs rather than accidentally merging rounds.

Final worker suite: 15/15 passed in 303.22s (`/tmp/teams-runtime-process-final.log`); journal7, private12, quota4, writer27 and compaction301 also passed. One unchanged native-295 summary oracle fails before the modified writer/protection branches; exact baseline call-path evidence is retained in SDK `docs/teams/shared-chat-runtime-cleanup.md`, not reported as passing. Final Desktop integration receipt `shared-bot-v3-integration-f9r9WX/receipt.json` records host SHA `d260a1888ece9e6cadf0ffebd54bae7ebf73a23a149af0f6558b4f0db16c819b`, worker SHA `2979258ac744520ea991a81646513e3acd21b8a6acf86094888c8c6b2de8d481`, two provider calls and six authorization checks. Debug whole-worker elapsed times were 25.876s for 200 and 141.771s for 450; these are not inference-only or release performance measurements.

SDK candidate `1a222074a968894286eee45aa1c3620b904f6ff1` is published on `codex/teams-bots-m1-runtime-20261105`. It preserves the concurrent create-bot feature and captures 38 task-owned paths. The isolated product has a locked canonical Git dependency graph with `development:false`; canonical local path overlays are preserved, while its SDK revision is updated. Current signed/native packages remain pending.

The user explicitly authorized the exact frozen Market archive upload/build and scoped cloud QA. Cloud Build `8f6a86bc-1990-45ac-996a-b042bb5bee93` succeeded with image digest `sha256:5f7b3e4788192435fbbc386357157b280adcb234f3b8ad35b7ccfe7343205d2a`. Existing jobs service is revision `market-jobs-00005-fbs`; no-traffic API candidate is `market-api-00008-riw`, while live API remains `00004-qzb` at 100%. Private eligibility is configured at `https://marketteameligibility-pacws5zsoa-uc.a.run.app` (`marketteameligibility-00002-9vx`); only the dedicated caller is an invoker. Nonsecret runtime configuration is recorded in `infra/firebase/teams-market-eligibility.env.json`; preserve/apply those fields when redeploying the private function. Five approved temporary QA accounts were registered through the actual candidate Web UI and real production Auth; two invitation flows joined only after confirmation. Private credentials/UID cleanup inventory stays outside the repositories. This is local candidate UI plus a test-only same-origin proxy, not proof of live Web deployment or fresh third-party OAuth signup.

Source upload initially failed automatic approval review; the exact payload/destination was subsequently approved and the same archived input was submitted. No bypass was used. Cloud source/IAM/content QA, actual model work and temporary-account cleanup must be tracked separately from the earlier production account-domain experiment.

## Current package verification

The isolated product candidate includes integration `8c3c1f069`, concurrent create-bot fix `225035844`, packaged worker entry cleanup `80c01dff5`, and billing-label fix `aa17959d6`. The latter reuses billing groups without changing model references;61 related tests passed. Independent source review passed these follow-ups and the current SDK preparer changes. This is not artifact acceptance.

The official Flare wrapper caught a previously omitted obsolete Rust worker entry, now replaced by the pinned SDK worker entry. A later packaging failure came from the candidate dependency directory being a symlink; it was replaced by an APFS clone while leaving canonical dependencies untouched. Signing/static/runtime checks are still pending. Fresh Android JNI plus Kotlin bindings are building together; no old generated FFI is accepted. SDK433 native support assets were added under six new release names without overwriting existing assets; empty-cache public download verification remains in progress.

Authorized cloud QA passed30 grouped checks, including all four audiences, current membership/role/generation, content and Range revocation, dedicated service identity, and existing App/Visualization reads. Evidence: `/Users/luolingfeng/lingxi/.omx/plans/evidence/2026-11-05-m0-m1/market-cloud-qa-result.md`. Existing public API/gateway/frontend traffic is unchanged. Server install receipts were manually exercised and do not establish physical installation. QA fixtures remain until device/model acceptance and must then be cleaned up.

### Actual follow-up acceptance

Android: fresh pinned-source JNI, symbols and all three Kotlin bindings were produced together. Normal DirectDebug build, native/rootfs/APK guards and1,889unit tests passed with zero failures/skips; package SHA `548d8308` is only an abbreviated identifier here, with the full digest retained in `.buildcache/teams-m1-candidate/native-artifacts/android-direct-arm64/build-receipt.json`. The independent QA app installed on Samsung and showed zero personal model providers. The user then confirmed the device is in use; all Samsung interaction stopped before QA credential input. This is not member-answer acceptance.

Desktop: the official Flare wrapper completed signing/static verification; ZIP SHA `3e9f4cd0e16ba4eea02e82b7a38cf252487aa65a323f072fc20c3e0a11925a58`. Runtime verification correctly refused to run against an already-registered Broker. Read-only inspection confirmed an active concurrent signed create-bot app and Broker, so neither was stopped. A scoped normal-exit/service-restore request is pending. No Desktop QA login or real-model business call has occurred.

Web A4: actual closed-tab and cross-tab failures were fixed using the existing bounded intent helper with localStorage, original expiry and matching-intent cleanup. There is no old-storage fallback or migration. Focused15tests/typecheck/build, real browser13checks and final cross-tab5checks passed; independent review closed the follow-up P2. Fresh OAuth and successful-join response-loss remain unverified. The frozen final Web artifact records the separately authorized temporary QA-only AASA override, preserving both production app IDs.

SDK input preparation: two real252MB iOS ZIP downloads were truncated by transport and correctly rejected by digest validation. Independent bytes matched the exact published archive prefix. The new128MB runtime-only archive removes only the duplicate source tar; all8runtime files remain byte-identical. The source archive remains mandatory through the existing fetcher and official profile verifier.24tests and local official profile verification passed; new public download acceptance remains in progress. Old release assets are unchanged, and no custom download protocol, fallback, source pin change or expected-hash substitution was introduced. Automatic upload review initially rejected the three new filenames; the user then explicitly authorized those exact files/destination and upload succeeded.

The user separately authorized isolated iPhone QA provisioning. Flare profiles preserve all required capabilities, and genuine Firebase config was obtained without changing CLI login. Only the Firebase MCP connector reported expired credentials; Firebase CLI was confirmed working. The authorized AASA-only Hosting version `6164a4662851face` changed exactly one path;35other file hashes/configuration match the prior release, and the served body was verified. Physical Apple routing remains unobserved.

## Historical implementation increments

The following sections retain earlier checkpoints and failures. Their pending-source statements are superseded by the current integration/package status above.

## Current delivery

- P00: source baseline and read-only inventory generated with `node scripts/teams-execution-inventory.mjs`. The inventory names retired symbol readers/writers and protected modules; no production or local data has been removed.
- P01, in progress: executable strict backend contracts in `apps/firebase/src/team-execution-contract.mjs`. SharedBot identity is independent of host/owner/instance. M1 accepts execution 2 with later capabilities disabled; listed members bind generation. Chat batches preserve each source identity/time and require a complete ordered interval, with a 200-message bootstrap and no incremental byte/frame truncation.
- Seven behavior tests cover membership rejoin, identity rejection, execution versions, stable admission, bootstrap, a 450-message delta above the old frame limit, and missing/reordered records.

P09: email registration reuses Firebase Auth. A same-tab invitation intent now survives auth interruption and refresh; showing the confirmation dialog does not clear it. Explicit cancel/success clears it; expiry is bounded and refresh does not extend it. Three intent tests and Web typecheck pass. Real-account registration and the member home remain pending.

## Replacement and preservation

Retire the old backend `team-bots.mjs` execution actions and `prepareBotTriggers`, old client actions/DTO/key decoding, Desktop connector execution, Harness team invocation/connector/process formats, and their old storage readers. New equivalents must replace each corresponding call site before integrated cutover. No alias, old-request translation, compatibility decoder or data conversion is allowed.

Retain the existing account/membership/invitation system, human conversations, shared Firestore cache, Bot definition/session infrastructure, UI components and valid business data. Change their new Teams contract call sites directly. The source inventory's protected-module entries are ownership boundaries, not permission to retain retired execution inside those files.

`node scripts/teams-execution-inventory.mjs --assert-removed` is a cutover gate. It currently fails intentionally because replacement is unfinished. Symbol scans are a necessary check, not sufficient proof: inspect exports, call graphs, package artifacts, rules, private worker protocol, and unknown retired aliases as part of P18. The worker replacement scope must have all retired contracts removed; file names alone do not decide compliance; valid isolation mechanisms should be extracted into the new implementation before deleting old modules.

The cloud cleanup selector must identify retired Bot principals before deleting their conversations and descendants. Preserve all human-only conversations, accounts, credentials, Team members, personal Bot definitions and Local App/Visualization data. Local cleanup requires verified actual scope directories. Stop old admission/workers and reconcile already-dispatched unknown effects before any cleanup. No cleanup has run.

## Remaining M0 gates

Freeze the command/session/grant/artifact and worker contracts across TypeScript, Rust, Swift and Kotlin, add common boundary vectors, inventory exact cloud/local formats and exports, implement isolated dry-run cleanup with preservation sentinels, and establish the new emulator/worker fixtures. P01 currently covers backend domain/input validation only; it is not a complete four-language contract freeze.

## Remaining M1

P03–P08 backend admission/quotas, grants/result gate, shared-cache reads, durable Bot×Chat session/worker, artifacts and readiness; P09–P11 member/publisher UI and native paths; P12–P17 Bot package/market/installation/identity/distribution; P18 deletion, integration, signed packages and physical devices. A pure validator is not an execution endpoint or authorization grant.

No M0/M1 completion or release acceptance is claimed yet. Backend old execution actions and their module/fixture/script have now been removed, and new execution commands are connected to the existing authenticated messages endpoint. Desktop/native/shared UI and the private host still need their replacement call sites. This intermediate workspace must not be deployed as a mixed-client release.

## Verification for the first increment

- `node --test apps/firebase/test/team-execution-contract.test.mjs`: 7 passed.
- `npm --prefix apps/web run typecheck`: passed (both application and Node tsconfigs).
- Web `teams-web-view.test.ts`: 4 skipped because react-test-renderer is unavailable in this environment; no UI pass claimed.
- `git diff --check` and Node syntax checks: passed.
- `--assert-removed`: exit 1 as expected while old execution call sites remain; this is a release blocker, not a completed deletion test.

Real Firebase registration/invitation, crash persistence, current signed Desktop and native devices have not been verified in this increment.

## Ongoing implementation, second batch

- P03/P04/P07: new `SharedBotsStore` reuses existing membership/conversation transactions and auth. Implements explicit publication/grants, random SharedBot principals, atomic message/admission/task/quota, one-shot start, per-Chat ordering, result audience gate before text/preview/artifact, immutable document delivery and current read ACL. No trigger/old-run reader is used. Additional host preparation/revalidation and pause/readiness signals are in progress.
- Removed backend old `team-bots.mjs`, its old runtime fixture and old protocol tests; rules tests are rewritten for SharedBot server ownership. Old Bot authorship can no longer be forged through SDK writes; private execution collections are denied. New task indexes replace the trigger index.
- P05: `createChatSourceReader` is wired into the same existing Desktop UID/project data page and private MessagePort. It provides recent/range/before/by-ID, current server ACL, range completeness, shared in-flight reads, bounded watermark proofs and SDK refill after eviction. Six tests and Desktop full typecheck pass. Actual worker use is still pending.
- P05/P06: sibling Harness `session_state/team_chat.rs` adds ChatInput to the existing canonical journal/coordinator/queue. One durable event contains source batch and dual-cursor fact; replay rebuilds metadata. Four Rust tests pass for full 450-message delta, restart, exact duplicate ACK, CAS race, identity/time and scope rejection. Runtime worker/model/transcript integration remains pending; this is not CTX completion.
- P02: explicit Team-scoped cloud dry-run/apply cleanup and preservation-sentinel tests added, not run in production. Exact local cleanup remains pending.
- Verified backend replacement regression before the latest small additions: 61 Emulator tests passed, zero skipped (15 new execution + membership + human messaging + rewritten rules). First invocation lacked rules-unit-testing; locked dev dependencies were installed offline, then the suite passed. A contention test initially failed due to SDK transaction retry exhaustion; maxAttempts=20 fixed it, and subsequent race suites passed. These failures remain in tool history.
- Latest HTTP retired-action race, generic shared-Bot push and cleanup fixtures await the next Emulator run. No deployment, signed package, real-token market experiment, native acceptance or production/local cleanup has occurred.
