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
- New task state `needs_reconciliation`. Web, Desktop, iOS and Android do not label it yet (they show the unknown-state text); that is part of Q08.

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

iOS and Android label the state and explain that the maintainer confirms it on Desktop or the web. They do not edit documents
or resolve operations yet.

`resources/translations/*.json` now contains the 22 `settings_browser_*` keys that had been added to the generated iOS and
Android files only, so `generate.py` is lossless again.

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
