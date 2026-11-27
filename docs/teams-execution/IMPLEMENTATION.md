# Teams execution replacement implementation

## Current acceptance status — 2026-11-06

M0/M1 remain in progress. The exact newly authorized Market archive `bd754f3d…cefa5` built successfully and API revision `market-api-00009-nk6` is live. Bot v3 uses the existing direct gateway; real browser cold Range/If-Range/auth checks passed. Both frontends are deployed, including the reviewed account-switch metadata/intent isolation fix, and all served files match their recorded manifests.

The exact approved production2 old bindings/4 Bot conversations and obsolete trigger index were removed after fresh guards. Actual Desktop userData retirement also completed: the reviewed old `team-bot-host` contained1 file/22 bytes; it was removed with no active writers, CAS checks and hashes proving all25 personal Bot files unchanged. No personal Bot record was rewritten; remaining cleanup actions0. Private receipt: `.omx/private/teams-m1-qa/local-retired-cleanup-receipt.json` in the workspace.

Fresh Android direct-root package passed1,894 tests and is installed on Samsung under the isolated QA identity; member account has zero configured providers. Real human messaging and Bot download handoff were observed, but full publication→download→installation ACK→sharing→member answer is still pending. Thirty meaningful real-model cases have not run. The independent iPhone package is signed; physical acceptance remains pending.

The new signed Desktop ZIP `4bc25b9115c4568316c0f21d91f411f372d4b91e832ead1a088e54409efd29f8` (383,564,474 bytes) passed the full official Flare signing, static and runtime gates. The original Broker registration was restored. Actual checks include provider persistence, cold session restoration, Local App delivery/Modify and no-reload draft round trips. The provider lock/shutdown and fresh-owned replay corrections have focused regressions and source review. Artifact inputs are baseline `19658a3` plus recorded uncommitted source hashes; no clean-commit provenance is implied. Receipt: `.omx/plans/evidence/2026-11-05-m0-m1/desktop-replay-runtime-pass.json`. Actual business UI publication is now starting; install ACK, shared deployment and thirty real-model cases remain pending. The concurrent Desktop task remains paused under the authorized coordination window.

The reviewed Web join fix is deployed;34 served files and the custom-domain index/AASA passed36 hash checks. A real first-admission response-loss retest passed9 checks: production200/alreadyMember:false at generation2 was followed by deliberately aborted delivery; refresh did not auto-join; explicit retry returned200/alreadyMember:true and closed the dialog with a verified selected Team workspace. Owner removal restored the outsider negative role; original four member documents and seven business documents were unchanged. That snapshot initially retained loading indicators because cleanup followed immediately. A separate bounded generation3 retest passed10 checks: real list/detail identity, selected Team and MemberHome became fully ready without loading indicators or alerts in1,413ms and remained stable for1,010ms under a30-second limit. Normal owner removal restored the outsider; original four member documents and seven business documents remained unchanged. Epoch3, invite use7 and exact join receipts are retained for audit/cleanup. The earlier failed run remains retained. Fresh OAuth and physical iPhone acceptance remain open.

The new family Bot was published through the real five-file review UI to self/hidden, then approved through the normal exact-digest review API. All five source files matched. The temporary QA reviewer role was restored and its revocation fence retained in the registered immutable journal. Samsung is currently physically disconnected, before owner login or a business download intent; the user has been asked to reconnect it. Business install/ACK, sharing and thirty real-model cases remain pending.

Main merge is authorized only after M0/M1 acceptance. No merge-to-main or complete-QA-cleanup claim is made. The checkpoints below are historical and may describe earlier pending states.


## M2/M3 runtime pin and deploy — 2026-11-13

- Runtime: the SDK branch `claude/teams-m2-runtime-20261111` is pushed at `d51c2fec` (merged with harness-runtime main). Its own test suite did not run for that merge: SDK CI runs only on pull requests and main, and this machine had no room to build it. The app pins every harness-runtime package to `d51c2fec`.
- Deployed to the existing project from branch `claude/teams-m2-20261111` (not pushed): functions `teamsApi` and `prunePushRegistrations`, the `(default)` Firestore rules (unchanged against main) and its four indexes, and Hosting (the Web build, with `/services` and the `/v1/services` route). The other functions were not redeployed; their code is the same as on main. Checked after deploy: `/`, `/services` and `/services/<id>` load, and `/v1/services` and `/v1/teams/messages` refuse a request without sign-in with `SIGN_IN_REQUIRED`.
- Team file bytes stay in Firestore as base64 parts (decided 2026-11-13); no Cloud Storage bucket is added.
- Not done after deploy: nothing was run with a real account, a signed Desktop app or a phone against the deployed service. The Q06 list placement was decided on 2026-11-14 (see Q06 below); its history and live lists came after this deploy.

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

**Decided 2026-11-14: the design is revised; the list stays server-owned.** Design §14.3 put the list in the maintainer's
Local App instance, reached through `serviceBroker`/`remoteGateway` with a new TeamAppGrant. That no longer fits the
project: the remote gateway needs a tunnel per maintainer and a wildcard TLS origin (`LINGXI_LOCAL_APP_REMOTE_ORIGIN`, off
by default), and a Local App has no revision or compare-and-set data API, so a list there could not give the conflict and
idempotency guarantees below. The list is one server-owned document per shared Bot (`teams/{team}/lists/{sharedBotId}`):
members see the same data on every client without the maintainer's Desktop being online, and every write is verified in
the emulator. TeamAppGrant is not built. What the design asks of the list and the old placement did not provide is added:
revision history with its source (J5) and open lists that follow changes (J4), below.

- History (J5): every applied write, by a member or by a Bot's task, stores `lists/{sharedBotId}/revisions/{dataRevision}`
  `{dataRevision, parentRevision, operationId, author, changes, createdAt}` in the same transaction. `author` is
  `{kind: 'user', uid, generation}` or `{kind: 'shared_bot', sharedBotId, taskId, requester}`; each change keeps the item
  as it was before (`before {text, done}`), so a removal still says what was removed. The newest 200 revisions are kept: the
  write of revision r deletes r − 200. A refused or repeated write records nothing. Members read it with
  `listHistory {sharedBotId, beforeRevision?}` (who may read the list), newest first, 20 per page, with `hasMore`.
- Live lists (J4): a list write also writes the Bot's change signal (`teams/{team}/signals/{sharedBotId}`), which clients
  already watch for Bot changes. An open list (and its history, once opened) is read again on that signal, so a change by
  another member or by the Bot shows without Refresh. Reads that finish out of order never replace a newer revision.

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
  is refused before it is sent. Tests: `Tests/TeamBotTests.swift`, `TeamBotsTest.kt`. Checked by compilation and unit
  tests only, not on a device or simulator.
- History and live lists on all four clients: the list screen has a collapsed History section that lists each revision with
  who made it ("You", the member's name, "a former member" for an earlier membership, or "<Bot>, for <requester>") and what
  changed (added, removed, renamed, ticked, unticked), with "Show older changes" while there is more, and says that the
  latest 200 changes are kept. Reading the newest page again keeps the older pages already shown. The list and the opened
  history refresh on the Bot's change signal (Web/Desktop `onMessagesChanged` with `signal.bots`; iOS and Android the
  messaging model's `botSignals` counter). `listHistory` is in the shared contract (validators and vectors) and in the
  Desktop allowlist. Tests: the emulator suite (authors, `before`, paging, pruning at 200, a removed audience), the Web
  scenario in `apps/web/test/teams-bots-ui.test.ts`, `TeamBotTests.swift` and `TeamBotsTest.kt` (lines, authors, merge).
  The new backend is not deployed.
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
for the other side. A run with a signed Desktop package, real process kills and real devices has not been done. The SDK branch
is now pushed and pinned, and the service is deployed (see the 2026-11-13 section).

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

**Where the bytes are.** In Firestore, as base64 parts of 700 KiB under `teams/{team}/uploads/{uploadId}/parts/{index}`; the rules deny every client read and write there. The design names a private object store; the app has no Cloud Storage bucket today, so this uses the database that is already there. This was kept as the decision on 2026-11-13. Only `team-attachments.mjs` puts, reads and deletes parts, so moving them to a bucket later changes that one file. **This is a deviation to confirm.**

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
- Deployed 2026-11-13, and the app pins the runtime change (see the 2026-11-13 section); nothing has run against the deployed service yet.

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

- Deployed 2026-11-13 (see that section), but nothing ran against the real project, in a signed Desktop app, in a real browser or with a real model. Every check above is a fake or the emulator.
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

## Bots that answer on their own (auto-reply)

A Bot can judge for itself whether to answer a Chat message nobody asked it about. It takes two switches, both off by default: the
maintainer publishes the Bot with `autoReply: { daily }` (Desktop only, refused with `DECISION_REQUIRED` when that Desktop has no
Decision service; hosted services are refused with `AUTO_REPLY_UNSUPPORTED`), and any person in a group, channel or Bot DM turns it on
for that Chat with the member command `setConversationAutoReply { conversationId, expectedRevision, targets }`.

- Service (`shared-bots.mjs`, `team-messages.mjs`): an ordinary message (`mutate` create, or `submit` that does not name the Bot) admits
  an `optional: true` task for each switched-on Bot in the same transaction, after the same checks as an asked task (published, author
  permitted, maintainer current, deployment online, grants). It writes no `taskIds`/`mentions` and charges no request quota; a group or
  channel task is charged at `startTask` against the Bot's own daily `auto` quota (over it: `declined`, `declineReason: 'quota'`). In a
  Bot DM the task is the usual one, charged as usual, with `optional` added. New host command `declineTask { reason: 'triage' |
  'superseded' }` (queued optional tasks only); `completeTask` takes `outcome: 'declined'` (optional tasks only, empty text, nothing
  posted). An unasked task that fails or is refused posts nothing, and members' `listTasks` hides unasked tasks without a reply.
- Desktop host (`teams/replyTriage.ts`, `teams/connector.ts`): an optional task waits 2.5 s so a burst is judged once; earlier queued
  optional tasks of the same Bot and Chat are declined as `superseded`. Before `startTask` each Bot is judged by its own settings
  (`autoReply` of its `bot.json`: `policy`, `keywords`, `quietHours`, `cooldownSec`, `thresholds`; `approval` does not judge: it holds a finished reply, see "Replies the maintainer reads first"),
  and the first step that fits decides:
  1. the service marked the task `addressed` (a reply to the Bot's message, or its name in the text: decided once by the service when it admits
     the message, not by the host; see "Several Bots answering one message") → run, source `rule`, reason `addressed`: a person talking to the
     Bot is answered even in quiet hours or the cooldown;
  2. quiet hours: the window is read in the zone the maintainer stored, at the message's service time → `declineTask`, source `quiet`;
  3. cooldown: the Bot posted in the Chat less than `cooldownSec` before the message, both times the service's `createdAt` and never this
     computer's clock (`0` turns it off; the 13 messages read are widened once, by at most 200, when they begin inside the window) →
     `declineTask`, source `cooldown`; then one of its keywords (folded the same way) → run, source `rule`, reason `keyword`;
  4. the Decision service answers one binary question from the Bot's name and description, the maintainer's `policy` as the criterion (the first
     1000 characters, as `maintainerInstructions`; the state still counts as evidence only) and the last 12 messages (never its role). The
     probability, rounded to three decimals, below the Bot's `decline` → `declineTask`; below its `run` → run with `mayDecline` (the runtime
     offers `TeamNoReply`); from `run` on → run. Source `decision`. Defaults are 0.3 and 0.75.

  No verdict → decline in a group or channel, run in a Bot DM, source `fallback`. A Bot DM has neither quiet hours nor a cooldown: every
  message there asks the Bot. The verdict is `{ action, source, reason?, probability? }` with codes only (`TRIAGE_*` in
  `packages/teams/execution.ts`); nothing a model said is kept. The Bot does not ask the requester questions in an unasked group task.
- Log (B5a). The host tells the service every verdict before it acts on it: host command `recordTriage { taskId, action, source,
  reason?, probability? }`, authenticated as `declineTask` is (`checkHost`), which stores `triage: { …, at }` on the task with the service's
  clock. Until the task has started a later verdict replaces the first (a restarted Desktop judges again); after that the same verdict is
  answered and a different one is refused (`TRIAGE_CONFLICT`, or `TASK_ALREADY_STARTED` for a task with none). Only optional tasks take one
  (`TASK_NOT_OPTIONAL`). A failure to record is logged (`team_triage_not_recorded`) and the task goes on. The maintainer reads the log with
  `listTriage { sharedBotId, conversationId?, cursor?, limit ≤ 50 }` → `{ items: [{ taskId, conversationId, messageId, createdAt, triage,
  state, conversationTitle? }], nextCursor? }`, newest first, hidden tasks included, only for Chats the maintainer is in (a page can be
  short), refused with `MAINTAINER_REQUIRED` to anyone else (`INVALID_TRIAGE_CURSOR` for a cursor that is not a task of that Bot and Chat). Both
  are refused to hosted services. `recordTriage` is a host command only: it is not in the member contract and a member's envelope for it is
  refused (vector `reject-recordTriage-member`). `listTriage` is a member read like `listTasks` (B5b): it is in the member contract
  (`MemberExecutionPayloads`) and in the validators of Web and Desktop (`execution-validation.ts`), iOS (`TeamExecutionContract`) and
  Android, with vectors (`valid-listTriage*`, `reject-listTriage*`: ids of 8 to 64 characters, `limit` an integer from 1 to 50) and in the
  member allow-list of the Desktop main process. Indexes (`firestore.teams.indexes.json`, to be deployed): `tasks` on `sharedBotId, triage.at
  desc, __name__` and on `sharedBotId, conversationId, triage.at desc, __name__`.
- Desktop screen (B5a): Bot detail → Auto-reply tab (`BotAutoReplyField.tsx`; `bots_reply_*` and `bots_triage_*` in `botsCatalog.ts`). It
  edits the settings above with the rest of the Bot (one draft, one Save; problems from `botDraftProblems`), and below them shows the log
  of each Team the Bot is published to, a page at a time (`bridge/botTriageLog.ts`). The window never holds a token: the main process
  reads the Teams (`triageTargets`) and the pages (`triageLog`) through `TeamsService`. Remote Control does not show it; Web, iOS and Android do (next bullet).
- Log on the clients (B5b), for the Bot's maintainer only (`packages/teams/triage-log.ts`, `triage-hooks.ts`, `TriageLogDialog.tsx`; iOS
  `TeamTriageLog.swift`, `TeamTriageLogViews.swift`; Android `TeamTriageLog.kt`, `TeamTriageLogScreen.kt`; strings `teams_triage_*`):
  - Team: an "Auto-reply log" button on the card of a Bot the viewer maintains (same uid and generation) that has `autoReply` published (Web
    `BotDirectory`, which Desktop's Teams pages share; iOS and Android Team details and the Friends Bots page). It opens the paged list with the
    Desktop semantics: newest first, 20 a page, Refresh, Load more, an empty state, a retry on failure; a row shows the verdict (glyph and
    word: no reply, reply, leave it to the Bot), the probability as a whole percent (only when the Decision service judged), the reason, the Chat's
    title, the time and where the message ended up (completed: Replied; declined and cancelled: Stayed silent; queued, running and waiting: In
    progress; the rest: Failed). A row this version cannot read is left out and the page is still shown; a reason or probability it does not know
    is left out of the row. `MAINTAINER_REQUIRED` takes the entry away and closes the list; any other error shows a retry. Nobody else sees the
    entry.
  - Chat: the same list limited to one Chat (`conversationId`): Web a clock button in the Chat header beside the auto-reply one (a Bot select
    when the viewer maintains several of the Chat's Bots), iOS and Android in the Chat details beside the auto-reply setting. The Bots offered are
    those the viewer maintains that are in the Chat and have `autoReply` published or switched on for the Chat; taking part in the Chat is not
    required, as the service shows a maintainer the Chats of their Bots. A friend chat has none.
  - Why a Bot DM did not answer: a declined optional task in a Bot DM keeps its "Did not reply" card and, for the Bot's maintainer, that card
    carries the reason and the probability from the log. They are read lazily, on first sight of a declined task of that Bot in that Chat, with
    `listTriage { conversationId }` pages of 50 from the newest down until every wanted task has its row, the log ends, or 4 pages were read; each
    task is looked for once, and only while there is a connection (the look is made when it returns; on iOS and Android a look the screen
    cancelled is made again); a log that cannot be read or a task with no row (the host could not record its verdict) leaves the card as it was. Nobody else triggers a read. In a group or channel a task nobody asked is
    `hidden`: it has no card on any client, so the Chat-scoped log above is the way to see why a Bot stayed silent there.
- Runtime (H `team_process/decline_tool.rs`): bootstrap `mayDecline`, result status `declined`; the choice survives a killed worker.
- Clients: Web/Desktop publish wizard switch and daily limit, a header dialog per Chat that first says where messages go, a declined
  run shown as "Did not reply"; iOS and Android the same switch per Bot in the conversation details, and the `declined` state text.
- Tests: `shared-bots.emulator.test.mjs` (Auto-reply), `bot-triage.test.mjs` and `bot-triage.emulator.test.mjs` (the log),
  `apps/electron/test/teams-reply-triage.test.ts`, `teams-host.test.ts` (`auto-reply:`, `auto-reply log:`), `bot-reply-field.test.ts`
  (the tab), `apps/web/test/teams-bots-ui.test.ts` (`Auto-reply:`), vectors in `contracts/execution-vectors.json`, iOS
  `TeamBotTests`/`TeamsPointerParserTests`, Android `TeamBotsTest`. B5b: `apps/web/test/teams-triage-log-ui.test.ts`, the member read in
  `teams-host.test.ts`, the vectors in `bot-triage.test.mjs`, iOS `TeamTriageLogTests`, Android `TeamTriageLogTest`.

## Replies the maintainer reads first (approval, Bot v3 B6a)

A Bot's maintainer can ask to read its reply before anyone else does. The setting is `autoReply.approval` of the Bot's own `bot.json`:
`never` (today's behaviour), `uncertain` (a reply whose triage verdict was `run_may_decline` waits) or `always` (every reply waits,
@mentions and Bot DMs included). The Desktop Auto-reply tab edits it (`BotReplyApproval` in `BotAutoReplyField.tsx`, `bots_reply_approval_*`
in `botsCatalog.ts`); the copy says a held reply is posted only once the maintainer approves it in Teams and is dropped after 10 minutes or,
when nobody asked the Bot, as soon as someone writes in the chat again. A hosted Bot service is never held (no Team, no maintainer to ask there,
and its gate does not list the commands).

- Host (`teams/connector.ts`). The hold is decided once, when the task starts, from the setting then (`always`, or `uncertain` with `mayDecline`)
  and kept as `hold: true` on the saved execution; a later change of the setting reaches the next task, not a running one. `saveResult` copies
  it onto the saved delivery only for a result that would have been posted (outcome `completed`, no private data), so a Bot that stayed
  silent, a failure and the failure the host makes up for a task with no result go as they always did. The delivery is sent as `holdReply`
  with the payload and the command id `completeTask` would have had, at both sends (the run's own and the outbox loop), so retries, a
  restart and a new account epoch behave exactly as they do for a completion. The outbox file carries `hold`, so a restart sends it as
  `holdReply` again; an execution record left behind with its result is held by its own `hold`.
- Service (`held-replies.mjs`, `shared-bots.mjs`). `holdReply` is `completeTask` for a run that completed, kept instead of posted. After the
  same checks (permit, deadline, output gate, the digest of the result for replays) the task moves to `awaiting_approval` with its deadline
  set to now + 10 minutes, and the Chat's execution lock is released at once. A result that could not be posted anyway (output gate refuses,
  a stale revision) ends as `completeTask` ends it; a replay with the same payload answers the same, a different one is `TASK_RESULT_CONFLICT`.
- Where the draft lives. `teams/{team}/heldReplies/{taskId}` holds the reply exactly as the host sent it (text, document, usage, files read,
  who asked, whether anybody asked, when it was held and when it expires). Only the service reads it: no rule matches the path, so the default
  deny applies to members, the maintainer and the Bot's host alike (`teams.rules.emulator.test.mjs`). The task, which every member reads
  through `taskView`, never carries a word of it, so the other members see a task in `awaiting_approval` and nothing else; a held task past
  its deadline is shown as `declined` even before the job that ends it has run. The maintainer reads the draft only through
  `listHeldReplies`, which also checks that they still read the Chat.
- Commands (maintainer only, `MAINTAINER_REQUIRED` 403 for anyone else; `TASK_REQUIRED` 404 for an unknown task):
  - `listHeldReplies { sharedBotId, conversationId? }` → `{ replies: [{ taskId, conversationId, messageId, requester, unasked, heldAt,
    expiresAt, text, document?, conversationTitle? }] }`, newest first, at most 100, drafts past their time left out.
  - `approveReply { taskId, text? }`: posts the draft by the same code `completeTask` posts with (`post`), as if the host had just finished, so
    it works while the Desktop is off. `text` replaces the reply text (trimmed, 1 to 8192 bytes); the document stays as the run made it.
    The output gate is checked again at that time: a message edited or recalled since, or a requester who left, ends the task as `declined`
    with `declineReason: revoked` and posts nothing. Answer: `{ taskId, state, replyId, artifactId }` (`state` is what posting came to,
    `completed` as a rule).
  - `rejectReply { taskId }` → `{ taskId, state: 'declined', declineReason: 'approval_rejected' }`; nothing is posted.
  - Races: each command reads the task and its draft in one transaction and writes both, so of an approval, a rejection, the end of the time,
    a newer message and a cancellation the first one wins and the later ones are refused with `409` and the way the task ended:
    `REPLY_APPROVED`, `REPLY_REJECTED`, `REPLY_EXPIRED`, `REPLY_SUPERSEDED`, or `REPLY_NOT_HELD` for any other end (a cancellation, a
    revocation). A repeat with the same command id is answered from its receipt, so an approval whose answer was lost is not decided twice.
- Time and supersede. 10 minutes: `approvalMs`. The task trigger (`index.mjs`, `becameHeld`) calls `HeldReplyJobs.held`, which enqueues a
  Cloud Tasks job on the existing `inboxWork` queue (`type: heldReply`, id from team, task and deadline, so a replayed trigger enqueues once,
  due one second after the deadline) and sends the push; `expireHeldReply` ends a reply nobody decided as `declined` with `declineReason:
  approval_timeout` and deletes the draft (`HELD_REPLY_NOT_DUE`, retried, if the queue is early). A new human message in the Chat ends the
  drafts of replies nobody asked for as `declined` with `declineReason: superseded`, in the transaction that writes the message (`supersedable`
  and `endSuperseded`, from `optionalTasks`, and from an answer to a wait); a reply to a task someone asked for (an @mention, a Bot DM) is
  never superseded. `cancelTask` accepts a task in `awaiting_approval` and deletes its draft with it. Every end of the state deletes the draft
  in the same transaction, so the draft exists exactly while the task is held. A run that left an operation of unknown outcome
  (`unverifiedOperations > 0`, copied onto the draft) is not declined by a rejection, the timeout or a supersede but ends in
  `needs_reconciliation` with `reconciledState: declined` (and is shown if it was hidden), as `completeTask` would end it: a person still has
  to check what it did.
- Push. Kind `reply_approval` to the Bot's maintainer, once per reply and person (claimed before FCM, as for a message), only while they are
  still a member who reads the Chat; copy is generic (`HELD_REPLY_COPY`), route `team/chat?conversation=<id>&team=<id>`, which the three
  clients' push-route allow-lists accept (`packages/deep-links`, iOS `PushMessagePolicy`, Android `PushRoute`; vectors in
  `friends-vectors.json`).
- Log. A row of `listTriage` carries `approval` (`held`, `approved`, `rejected`, `timeout`, `superseded`), derived from the task by
  `approvalOutcome`; the row's state is `awaiting_approval` while it waits. Web and Desktop label it (`teams_task_waiting_approval`, and the
  four decided words), iOS and Android decode it and show the same words; no client decoding ever fails on the new state or code.
- Contract. `execution-vectors.json` has `awaiting_approval` among `taskStates`, `approvalOutcomes`, and 24 rows for `listHeldReplies`,
  `approveReply`, `rejectReply` and for `holdReply` from a member (`reject-holdReply-member`: it is the host's command, as `recordTriage` is).
  `packages/teams/held-replies.ts` has the client calls (`readHeldReplies`, `approveHeldReply`, `rejectHeldReply`) and the decoders the
  approval screens build on; the native clients have the validators (`TeamExecutionContract`) and the state and outcome tables, and
  show a held task as "Waiting for approval" with the sentence that its maintainer reads it first. The approval screens of Web, iOS and
  Android are B6b, below.
- Index. One field override (`firestore.teams.indexes.json`, to be deployed): `heldReplies.result` is not indexed (the draft's text and document
  are never queried). The list reads by one field (`sharedBotId` or `conversationId`) so that no composite index is needed.
- Tests: `apps/firebase/test/held-replies.test.mjs` (schemas, codes, vectors, the trigger's rule), `held-replies.emulator.test.mjs` (the state
  machine on the emulator: hold, approve with and without edits, reject, timeout, supersede, cancel, races and replays, the draft unreadable to
  everyone but the service) and `teams.rules.emulator.test.mjs`; `apps/electron/test/teams-host.test.ts` (`approval`), `bot-reply-field.test.ts`
  (the control, the log words); `apps/web/test/teams-held-replies.test.ts`, `teams-bots-ui.test.ts`, `teams-triage-log-ui.test.ts`; iOS
  `TeamBotTests`, `TeamTriageLogTests`, `TeamsTests`, `FriendLinkTests`; Android `TeamBotsTest`, `TeamTriageLogTest`, `TeamsBotTextTest`,
  `FriendLinkTest`.

### The maintainer's approval screens (Bot v3 B6b)

The three commands above, as screens on Web and Desktop (`packages/teams`, one implementation), iOS and Android. Only the Bot's maintainer
is shown any of it; every other member keeps seeing a task that "Waits for approval". No new request shape: the vectors did not change.

- Where. (1) In the Chat, in the place of the task's card, for a task in `awaiting_approval` of a Bot the viewer maintains: the draft (as
  Markdown, with the title of the document when it has one), the time left, and Approve (posts it as it is), Edit (an editor with the draft,
  Send and Cancel) and Reject (an in-app confirmation, never the browser's). (2) A reply nobody asked for has no task in the Chat's list (the
  service keeps such a task out of every member's), so it is a row of its own after the last message, from the Bot that made it, tagged
  "Auto-reply". (3) The Bot's card, where the log button is: "Replies waiting" with the number, only while there are some, opens the list of
  the Bot's replies in every Chat, grouped by Chat (`conversationTitle`), each with the same card and "Open chat". (4) The push opens the Chat
  by its route and the Chat reads the replies when it opens, so the card is there.
- Reading. `listHeldReplies` is asked for each Bot of the Chat that the viewer maintains (membership and generation as now held), with
  `conversationId` in a Chat and without it on the Bot card, and only by a maintainer and only while there is a connection. It is read when
  the screen opens, whenever the Chat's tasks may have moved (the Bot signals, the Chat's last sequence, the set of tasks that await approval),
  when the connection returns and when a reply runs out; never on a timer of its own, so no oftener than the tasks are. One read at a time:
  a request that comes during one is answered by one more read after it. A reply this version cannot read is left out and the rest are shown;
  a document that cannot be read leaves its reply out (nobody approves what they did not see); an answer that is not a list is `INVALID_DATA`.
  `MAINTAINER_REQUIRED` from every Bot means the person does not maintain it any more: the entry and the list go away.
- Time. The card says how long is left (minutes, hours, days, rounded up, at least a minute) and is brought up to date every 30 seconds and
  at the end of the first reply that waits (read again 0.5 s after it). A reply whose time ran out on the clock shows as dropped, without
  buttons, even before the service is asked; the client's clock is only ever used for this and for the line, the service decides.
- Decisions. Each is one command, with a command id made once per decision and reused by every retry of it, so an approval whose answer was
  lost is not decided twice; Approve and Send (with other text) are different decisions. Sending trims the text and needs 1 to 8192 UTF-8
  bytes; text equal to the Bot's once trimmed is sent as no text, so the reply is not recorded as changed. A decision on its way disables the
  card's buttons. Once the service took a decision the Chat's tasks are read again. An approval answered with `declined` and `revoked` posted
  nothing: the card says so (`REPLY_NOT_POSTED`, a client-only code) and stays until closed.
- Errors. Every code has words (`teams_error_reply_*`, `teams_error_held_maintainer`, `teams_held_revoked`). A reply the service says is not
  there to decide (`REPLY_APPROVED`, `REPLY_REJECTED`, `REPLY_NOT_HELD`, `REPLY_EXPIRED`, `REPLY_SUPERSEDED`, `TASK_REQUIRED`,
  `MAINTAINER_REQUIRED`) stays on screen saying what became of it, with Close, and is not tried again; a reply that leaves the list for no such
  reason (decided on another screen, dropped for a newer message) simply goes, unless a decision on it is on its way or its time ran out
  while it was shown. Any other failure leaves the card as it was, with the message, and Retry is the same button. A failed read keeps the
  rows that were read and says so, with Retry (Chat banner and list), unless the Chat is already saying the same.
- Code. Web and Desktop: `held-replies.ts` (rules and `HeldRepliesBoard`), `held-hooks.ts` (`useHeldReplies`), `HeldReplyCard.tsx`,
  `HeldRepliesDialog.tsx`, wired in `TeamConversationView.tsx`, `BotDirectory.tsx` (Teams) and `FriendBotsPanel.tsx`; `heldReplyErrorText`
  in `errors.ts`. iOS: `TeamHeldReplies.swift` (Foundation only: decoding, rules, `TeamHeldBoard`), `TeamHeldViews.swift`, the three model
  calls in `TeamMessagingModel.swift`, wired in `TeamConversationView.swift`, `TeamMessageViews.swift`, `TeamBotViews.swift`,
  `TeamDetailsPage.swift`, `FriendBotsView.swift`. Android: `TeamHeldReplies.kt` (the same, with `TeamHeldBoard`), `TeamHeldViews.kt`, the
  three model calls in `TeamMessagingModel.kt`, wired in `TeamConversationScreen.kt`, `TeamBotComponents.kt`, `TeamDetailsScreens.kt`,
  `TeamsTab.kt`, `friends/FriendBotsScreen.kt`, `friends/FriendChatScreen.kt`. Words: `teams_held_*` in `resources/translations/*.json`
  (`tw_error_reply_*` became `teams_error_reply_*`).
- Not here. The Desktop's local Bot editor is unchanged (it sets the policy, B6a); the Desktop's own Bots tab has no "Replies waiting" yet (the
  Chat shows the cards). A friend chat itself has no Bot and reads nothing; a friendship's Bot DM is a Chat like any other.
- Tests: `apps/web/test/teams-held-replies-ui.test.ts` (the board, the card, the Chat, the Bot card and the list, the words), iOS
  `TeamHeldRepliesTests` (Foundation-only, run with the macOS logic runner), Android `TeamHeldRepliesTest`. The screens were not run on a
  simulator or a device.

## Several Bots answering one message (reply rounds, Bot v3 B7a)

Until B7 every Bot with auto-reply on judged an unasked group or channel message on its own Desktop, so three Bots could answer one message. Now the
service decides who answers, because no Desktop can referee the others: the message that admits the optional tasks (B5) also opens a **round**, the
Desktops bid, and the service picks the best bids, as many as the Chat allows (`maxReplies`, 1 to 5, default 1). This phase is the service, the
Desktop host and the clients' decoding; the Chat settings screen, the offline line (rule 6) and the edit hint (rule 8) are B7b, below.

- Round (`apps/firebase/src/reply-rounds.mjs`). `teams/{team}/replyRounds/{messageId}` is written by `optionalTasks` in the transaction that writes
  the message and its tasks (`openRound`): `{ state: 'open', deadlineAt: now + 15 s, maxReplies, candidates: [{ taskId, sharedBotId, addressed? }],
  bids: {} }`. `maxReplies` is copied from the Chat then, so a later change does not move an open round. Only the service's clock is used (`now`
  of the transaction; a Desktop's is never sent). No rule matches the path: members, maintainers and hosts cannot read it
  (`teams.rules.emulator.test.mjs`), so a Bot's bid is seen by nobody but the service and, through `listTriage`, its own maintainer.
  States: `open` collects bids; `resolved` has chosen (`winners`, `standby`, `resolvedAt`, `startBy = resolvedAt + 20 s`); `voided` ended before
  anyone was chosen. A task of a round carries `roundId`, `addressed?`, `coMentioned?` and, once decided, `round: { outcome, tier? }`.
- `bidReply { deploymentId, hostId, accountEpoch, taskId, tier, score?, source }` (host command, `checkHost`; a member's envelope for it is
  refused, vector `reject-bidReply-member`; a hosted Bot service takes none: its tasks are asked for and have no round). Tiers, best first:
  `addressed`, `rule`, `run`, `may_decline`, `decline` (an abstention: the host judged "no"); `score` is the Decision probability in 0..1, rounded to
  three decimals, absent for rules. The bid must come from the deployment that holds the task (`TASK_REQUIRED` 404 otherwise), for an optional task
  of a round (`TASK_NOT_OPTIONAL`, `ROUND_REQUIRED`), still queued and never started (`TASK_ALREADY_STARTED`), within its deadline (`TASK_EXPIRED`),
  whose requester may still ask (`TASK_REVOKED`), and `addressed` only for a task the service marked so (`BID_TIER_INVALID`). The first bid of a
  task stays whatever is sent again (a restarted Desktop judges and bids again), a task already decided answers with its outcome, and a bid for a
  round that is not open is refused `ROUND_CLOSED`. Answer: `{ taskId, round }`, `round` being `open` or the task's outcome.
- Settlement (`resolveRound`). When every candidate that is still waiting has bid, the bid that completes the set settles the round in its own
  transaction; otherwise the queued job does at the deadline. Order: the tier (an `addressed` task counts as `addressed` whatever its host bid, but
  it must have bid), then the score, then a fixed hash of the task id, so every settlement of the same round gives the same answer. A candidate
  that never bid, or bid `decline`, cannot win. The first `maxReplies` win (`round.outcome: 'won'`); the other bidders end `declined` with
  `declineReason: 'coordination'` (`round.outcome: 'lost'`); a Bot that bid `decline` ends `declined` as `triage`, as its own host would have done,
  and one that did not bid in time ends `declined` as `coordination`, both `abstained`; no quota is charged, since quota is charged at `startTask`.
  Every Bot of the round is woken through its `signals/{sharedBotId}` doc.
- Timers (`reply-round-jobs.mjs`, `index.mjs`). The task trigger calls `becameCandidate` for the first write of a task with a `roundId`, which enqueues
  a `replyRound` job (`phase: 'bids'`, due one second after `deadlineAt`) on the existing `inboxWork` queue, and `becameWinner` for a task that has just
  won, which enqueues `phase: 'start'` at `startBy`. Job ids come from the team, the round and the phase, so a replayed trigger or a task of the
  same round enqueues once. A job that runs early throws `REPLY_ROUND_NOT_DUE` and is retried; one that finds nothing to do ends quietly.
- Promotion (`promote`). At `startBy`, winners that have not started (`startedAt` unset) end `declined` as `coordination` (`lost`) and give their seat
  once to the next-ranked bidders whose deployment is still enabled, ready and online (`round.outcome: 'promoted'`, the task is queued again); the
  round never promotes twice, so a failure cannot chain. A winner that started keeps its seat even if it then waits in approval (B6, `awaiting_approval`): that Bot owns the round. A winner
  that started and then failed is not replaced.
- Starting. `startTask` is refused `ROUND_OPEN` for a round task whose outcome is not `won` or `promoted`; `listHostTasks` rows carry `round`
  (`open`, `won`, `promoted`), `roundTier` and `addressed`, so a restarted Desktop knows where its task stands and how sure its Bot was without
  judging again.
- Voiding. A newer human message in the Chat voids the Chat's open rounds (`openRounds`/`voidRounds` in the transaction that writes it), and
  `answerWait` does too; the tasks of a round that have not started end `declined` as `superseded` (`round.outcome: 'superseded'`). The recall of a
  message voids its own round and cancels the queued tasks that asked a Bot (`recalled`, in `team-messages.mjs`); an edit never creates or changes
  a task. Messages by Bots never create tasks or rounds.

Whom a message speaks to is decided by the service once, when it admits the message (`submit` and `optionalTasks` in `shared-bots.mjs`,
`bot-address.mjs`), so every Desktop agrees. The numbers are the plan's:

1. **It asked a Bot (`targets`):** that Bot has its own asked task and answers; no round, and no other Bot joins in whatever its settings say
   (`optionalTasks` admits nobody when the message asked someone).
2. **It asked several Bots:** each answers, with no coordination; each task carries `coMentioned`, the names of the others, which `prepareTask`
   returns and the connector appends to the run's instructions as quoted data ("they answer it too, each in its own role"), at most eight names of
   forty characters on one line.
3. **It mentions only people (`mentionEntities`, no Bot):** no round and no Bot task, unless the Chat's `humanMentions` is `judge` (default `quiet`).
4. **It answers a Bot's message, without an @:** a soft mention. The round holds only that Bot (every Bot spoken to, if several), its tier is forced
   to `addressed`, `maxReplies` is as many as were spoken to, and the task is `addressed: true`. The Bot may still stay silent: the run offers
   `TeamNoReply` and the Bot is told to answer unless the message plainly needs no answer. A Bot spoken to that may not answer unasked still keeps
   the others out.
5. **It calls a Bot by name in its text, without an @:** the same as 4. Name matching moved here from the host (`calledBy`: NFKC and lowercase
   folding, whole words where the script has words, at least two code points); `replyTriage.ts` no longer looks for names, its `addressed` check
   and that check's tests and text are deleted, and a task with `addressed: true` is judged `run`, source `rule`, reason `addressed`.
6. **A mentioned Bot whose Desktop is offline:** the message still posts and its task queues for 24 hours (`offlineHost` in `submit`; a Bot DM or a
   hosted service still refuses with `HOST_UNAVAILABLE`). The sender's "X is offline, will reply when online" line is B7b, below.
7. **Settings against an @:** quiet hours, the cooldown and the Bot's policy are part of the unasked judgement only, so they never apply to an asked
   task; approval `always` holds the reply of an asked task too, `uncertain` only that of an unasked one (B6); an asked Bot in a group cannot choose
   silence (`mayDecline` belongs to optional tasks: unasked ones, soft mentions included, and a Bot DM's).
8. **Edit and recall:** an edit never creates a task, and so never triggers a Bot (its hint in the UI is B7b, below); a recall voids the message's open round
   and cancels its queued asked tasks (`recalled` in `reply-rounds.mjs`, called from `team-messages.mjs`).
9. **A message written by a Bot** (a reply, a status message, an @ it wrote) never creates a task or a round.
10. **A new human message** voids every unsettled round of the Chat and ends its unasked held replies (`end()` in `optionalTasks`, in the same
    transaction that writes the message); so does the answer to a wait (`answerWait`).

Settings. `setConversationAutoReply { conversationId, expectedRevision, targets, maxReplies?, humanMentions? }` (`maxReplies` an integer 1 to 5,
`humanMentions` `quiet` or `judge`; what is left out is left as it is, `null` is refused). The Chat document's `autoReply` carries them back, and
`team-messages.mjs` projects them to the clients.

- Desktop host (`teams/connector.ts`, `teams/replyTriage.ts`). An unasked task of a group or channel is judged as before (settle 2.5 s, B5 verdict,
  `recordTriage`), and then, instead of `declineTask`/`startTask`, the verdict becomes a bid: `bidOf` maps a decline to `decline`, `addressed` to
  `addressed`, a rule (a keyword) to `rule`, `run` to `run` and `run_may_decline` to `may_decline` with the Decision probability as the score. The
  host sends `bidReply` (command id from the task, tier, score and source, so a repeat is one request), remembers that it has bid (`bids`, memory
  only) and does not start the worker: the service wakes it when the round is settled. The answer, and later the row's `round`, decide: `won` or
  `promoted` starts it (a Bot DM task never has a round and keeps the direct `declineTask`), anything else ends it with nothing left to do. A Bot
  DM's optional task keeps the direct path and gets the service's `addressed`. After a restart the verdict is not asked of the model again: the row's
  `roundTier` says how sure the Bot was (`may_decline` offers `TeamNoReply`), and the first bid of a task stays the one the service kept. Tasks of
  a round are never superseded locally (the service ends them). `bidReply` is on the host allow-list of `service.ts`.
- What the maintainer sees. A row of `listTriage` carries `round: { outcome: 'won' | 'lost' | 'abstained' | 'promoted' | 'superseded', tier? }`
  for the maintainer's own Bot only; other Bots' bids and scores are never in any answer. Web and Desktop (`triage-log.ts`, `TriageLogDialog.tsx`,
  `BotAutoReplyField.tsx`), iOS (`TeamTriageLog.swift`) and Android (`TeamTriageLog.kt`) decode it and say it in words (`teams_triage_round_*`
  in `resources/translations/*.json`); a row with an outcome a client does not know is still a row, and no decoding fails on the new fields.
- Contract. `execution-vectors.json` lists `bidTiers`, `roundOutcomes`, `humanMentionModes` and `maxRepliesLimit` and has 14 new rows for
  `setConversationAutoReply` and the member's `bidReply`; the validators of Web and Desktop (`execution-validation.ts`), iOS
  (`TeamExecutionContract`) and Android (`TeamExecutionContract`) read the same rows. The member commands builder (`executionCommand`) accepts
  the two settings; the screens for them are B7b, below.
- Index. None: the round is read by id, and `openRounds` queries `replyRounds` on two equality fields (`conversationId`, `state`), which Firestore
  serves without a composite index.
- Tests: `apps/firebase/test/reply-rounds.test.mjs` (codes, schemas, name matching, trigger rules), `reply-rounds.emulator.test.mjs` (the state
  machine: bids, settlement by tier, score and hash, deadline job, promotion, voiding, recall, the mention rules, offline, quota, visibility,
  replays), the adapted `shared-bots`, `bot-triage` and `held-replies` emulator suites and `teams.rules.emulator.test.mjs`;
  `apps/electron/test/teams-reply-triage.test.ts` and `teams-host.test.ts` (the bid flow, restart, `coMentioned`); `apps/web/test/teams-reply-rounds.test.ts`
  and `teams-triage-log-ui.test.ts`; iOS `TeamTriageLogTests`, `TeamsPointerParserTests`, `TeamBotTests`; Android `TeamTriageLogTest`, `TeamBotsTest`.
- Not here, or not verified. The Chat settings screen, the offline line (rule 6) and the edit hint (rule 8): B7b, below. Nothing was run against a signed Desktop package or a real Desktop restart. The
  bids window (15 s) covers the host's 2.5 s burst wait and a Decision call of up to 10 s (`TRIAGE_TIMEOUT_MS`); a Desktop slower than that bids late and
  the round settles without it. A name or a reply in an earlier message of a burst no longer makes a later message `addressed` (the service judges each message
  itself, and each has its own round), where the old local check read the burst. Round documents are not deleted yet (no TTL).

## Chat settings, the offline line and the edit hint on the clients (Bot v3 B7b)

The screens for what B7a built, on Web and Desktop (`packages/teams`), iOS and Android. The service keeps its rules; the only addition is one
field, `hostWait`, on the task view. All text is in `resources/translations/*.json` (`teams_auto_reply_max_replies`, `_human_mentions`,
`_human_mentions_hint`, `_rank_note`, `teams_bot_offline_wait` / `_fmt`, `teams_edit_no_notify_hint`).

- Chat settings. Where a Chat's auto-reply Bots are edited (Web and Desktop `AutoReplyDialog.tsx`; the iOS details sheet, `TeamDetailsViews.swift`;
  the Android details sheet, `TeamDetailsScreens.kt`) a group or channel also sets `maxReplies` (1 to `maxRepliesLimit`: a select, a segmented
  picker, chips) and `humanMentions` (`quiet` or `judge`: a checkbox, a toggle, a switch). A Bot DM shows neither, since it has the one Bot, which is
  always the one asked. One line says Bots on different Desktops are ranked by how sure each is first, and that Decision probabilities from different
  maintainers are not compared directly (the code does use the score, but only between Bots of the same tier, before the fixed hash). Every change
  goes through `setConversationAutoReply` with the revision the screen read, so a change made meanwhile by someone else is a conflict. The dialog
  sends its whole Bot list and, of the two settings, only what changed; iOS and Android send one change per tap, with the Bots as they stand (hidden
  Bots kept) and the controls locked until the listener brings the next revision. Decoding of the two fields (default 1 and `quiet`) was B7a's.
- Offline line (rule 6). `taskView` carries `hostWait: true` for a task that is `queued`, was admitted while its Bot's Desktop was off (`queuedTask`
  stores it) and has never started (`epoch` 0). It is gone once the task starts, ends or, after `answerWait`, is queued again for the usual five
  minutes, so an expired task shows the existing expired state. The run card says "<Bot> is offline and will reply when it's back online" in place of the
  generic queued line (Web and Desktop `BotRunCards.tsx`, iOS `TeamBotViews.swift`, Android `TeamBotComponents.kt`; `waitsForHost` on the iOS and
  Android `TeamTask`). The composer had to stop refusing for it: in a group or channel a Bot whose card says `offline` no longer blocks the message and
  the footer says the same line (Web `Composer.tsx`, iOS `blockedAvailability(... waitsForHost:)` and `TeamComposerView`, Android
  `TeamBotEntry.blockedRequests`/`requireNewRequest(... waitsForHost)` and `TeamComposer.kt`). A Bot DM keeps its refusal, and paused, maintenance
  and quota exhausted refuse in every Chat. Android's "refused before" flag has no kind, so a Bot whose status is merely unknown after a refusal
  still blocks everywhere; only a current `offline` is waited for.
- Edit hint (rule 8). While a message is being edited in a group, a channel or a Bot DM, the composer says "Editing doesn't notify Bots. To ask one,
  send a new message." Always, not only when the edit adds a Bot mention; an edit between two people shows nothing.
- Docs. `apps/web/src/data/docs/collaboration.ts` (Teams `auto-reply`, the new `reply-rounds` and `bot-mentions`, Bots `no-reply`) and the
  generated `docs/technical/{en,zh}/{teams,bots}.md`. The documentation baseline (`docs:sync`) was not refreshed here.
- Tests: Web `teams-bots-ui.test.ts` (the run card's offline line in three locales, the Chat dialog and what it sends) and
  `teams-workspace-interaction.test.ts` (a Bot whose Desktop is off in a channel and in a Bot DM, the edit hint); iOS `TeamBotTests` and
  `TeamBotMemberEntryTests`; Android `TeamBotsTest` and `TeamBotEntryTest`; `reply-rounds.emulator.test.mjs` gained the `hostWait` assertions
  (set, absent for a started Bot, gone once expired).
- Not here, or not verified. The `hostWait` assertions of the emulator suite were written but not run. The iOS and Android screens were type-checked
  and unit tested, not run on a simulator or a device. The Desktop's own Bot editor still says a Bot that is replied to or called by name "always
  answers" (`bots_reply_mention_note`), where rule 4 lets it stay silent when the message plainly needs no answer.

## Bots delegating to Bots (Bot v3 B8)

A Bot that is running for a person can hand part of the request to another Bot of the same Chat, and either go on with that Bot's reply or let it answer in the
Chat itself. This is B8a: the service (`delegations.mjs` and the commands in `shared-bots.mjs`), the runtime's team worker (H, the tool `TeamDelegate`) and the
Desktop connector, and the four publication controls of the Desktop publish wizard (`BotDirectory`). The other screens (the chain on a task, the wait on it, the
capability shown on a card, on every client) are B8b, below the notes of B8a; the fields they need are on the task view and the card.

- Only a tool call delegates. The model calls `TeamDelegate {target, request, expectReply}`; an @ it writes in its text triggers nothing. With
  `expectReply: false` the worker asks its host for the operation `delegate`, which the connector sends as `delegateTask`; the answer is `{sent: true, ...}` or
  `{sent: false, code}`, and a refusal is an answer the Bot is told in words, never an error. With `expectReply: true` the worker does not call the host: it parks
  through the same arbiter as `AskUser` and the list proposal (`ask_tool::Wait::Delegation`), the checkpoint reports a wait of the new kind `delegation` whose
  `question` is the call as JSON, and the host sends `pauseTask` of kind `delegation` with `delegation {logicalStepId, target, request}`. The delegation is made in
  the same transaction as the pause (`parkForDelegation`), so the other Bot cannot answer before the run waits, and a delegation the service refuses leaves a
  task that is queued again at once with `resume {waitId, refusal, target}`. Both paths end in `delegate()`, which is idempotent by the logical step
  (`delegations/{hash(parentTaskId, logicalStepId)}`): the same step again answers what it made, another request under it is `DELEGATION_CONFLICT`.
- Both Bots have to agree. The target publishes `acceptDelegation` (`none`, the default; `team_bots`; or `listed` with `acceptFrom`, the shared ids it takes
  requests from), the delegating Bot publishes `delegateTo` with `delegation {daily}` (the two go together, never the Bot itself). The target must be a
  participant of the Chat and in its execution policy. It runs as the person who asked the first Bot: `BOT_USE_REQUIRED` unless that person may use it (its
  `usePolicy` and every grant), exactly as for a message they sent. Scope is not inherited; the target's own scope narrowed by the Chat applies. Both deployments
  must report `capabilities.delegation` (now a boolean, true on this Desktop whatever the route); a hosted service never delegates (`DELEGATION_UNSUPPORTED`
  at publication). `directory()` cards carry `capabilities.delegation`, `acceptDelegation`, `acceptFrom`, `delegateTo` and `delegation {daily}`.
- What one delegation makes, through `submit` (new parameter `delegation`, beside `dataCause`): a message of the delegating Bot, "@Target request", a reply to
  the message that started the run (`bot_` ids); a task of the target asked in that message, with `rootTaskId` (the family's first task), `delegation
  {delegationId, rootTaskId, parentTaskId, parentBotId, depth, chain [{sharedBotId, name}], expectReply}` and a stamped triage (`run`, `rule`, `addressed`) so
  `listTriage` shows it; and the server-only record `teams/{team}/delegations/{id}` (`parentTaskId`, `parentBotId`, `rootTaskId`, `targetBotId`,
  `targetTaskId`, `conversationId`, `messageId`, `expectReply`, `digest`, `depth`, `state`, `waitId`, `createdAt`; later `result.text`, `doneAt`). No rule
  matches it, so the default deny covers it and a reply kept for a waiting run is never readable before that run has used it. The message opens no reply
  round and no auto-reply (`submit` skips `dmOptional` and `unaskedTasks` for it), a target whose Desktop is off queues the task for a day (`hostWait`), and
  the quota is the delegating Bot's own: `delegation.daily`, counted under the day and that Bot (`hash([day, 'delegation', botId])`).
- Limits. A chain is at most two hops from the person's message (person -> A -> B -> C, and C cannot delegate); a Bot already on the chain cannot be asked again
  (`DELEGATION_LOOP`); one root task makes at most three delegations and data handoffs together (`usedByRoot`, a count over `tasks where rootTaskId ==`, a
  single-field query, so no index; `DATA_HANDOFF_LIMIT` in the handoff transaction, `DELEGATION_LIMIT` here). A handoff counts as one hop. A request is at
  most 2000 bytes. Nobody asked a Bot that only judges a message (`DELEGATION_UNASKED`), and a run on private data never delegates. The Bots a run may ask
  are computed by `delegationOffers` and returned as `delegates [{sharedBotId, name, description?}]` by `prepareTask`, `validateTask` (the context a lost answer or a recovery is reconciled from), `startTask` and `continueTask`; the list is
  empty when the Bot has none, its deployment cannot, the chain is at its end, the root's count is used up or none of the targets would accept. The host passes it
  to the worker as `delegates` and keeps it with the attempt; the worker refuses a call to a Bot that is not in it, and the host checks the same before it
  sends anything. A continuation that answers a delegation is always offered the Bot it asked, whatever has changed since, because the replay of that call
  needs it.
- The reply. A delegation that waits sets the target's task `expectReply`. When that task finishes, `fate()` reads the record and the waiter, and `post()` hands
  the reply to the waiting run instead of posting it: the record is `done` (with the text, cut to 7 KiB, since a continuation carries 8 KiB), `withheld` (the
  audience gate refused it for this Chat: only the fact is handed over) or `failed`; the target's task ends with `handedBack` and no reply message, and the
  waiting task is queued again with `resume {waitId, delegation}`, the wait `resolved` and the Chat lock kept. The same code serves a reply the target's
  maintainer held (B6): approving it hands back the text the maintainer approved, rejecting it or letting the hold run out hands back `withheld`, and an approval
  that was revoked in the meantime hands back `failed`, so the waiting run never waits for a reply that will not come (`held-replies.mjs`). A task whose waiter has
  gone posts its reply in the Chat as any reply does: the waiter detached, was cancelled or expired, or its Bot was published again or moved to another deployment.
  That is a new revision, which refuses every earlier task of the Bot at its next step (`TASK_REVOKED`), for a wait on another Bot as for a wait on a person; the new
  deployment is not given the run (a task is bound to the deployment and the revision it was admitted under, and `listHostTasks` lists a deployment's own tasks), so
  `waiterOf` does not count such a run as waiting. The other Bot moving is the same as ever: its earlier deployment's result is refused (`DEPLOYMENT_REQUIRED`), and
  the run goes on after its ten minutes, told the reply has not come.
  `prepareTask` and `continueTask` give the resumed run the outcome (`wait {waitId, delegation {delegationId, targetBotId, targetName, outcome, text? | code?}}`);
  the host keeps it before it asks to continue, writes one sentence for it ("<Bot> replied: ...", "<Bot> has not finished yet", "The request to <Bot> was
  cancelled", the reason a request was not sent) and passes that as the continuation's answer, with the delegation id as `message_id`. The replayed tool call
  returns it. `answerWait` and `decideWait` refuse such a wait (`WAIT_KIND_MISMATCH`); nobody types into it.
- The time. A run waits ten minutes (`expiresInMs` of the pause, the wait's `expiresAt`, shown as `wait.expiresAt`); the task itself stays a day
  (`holdsUntil`, since its Desktop may be off when the reply comes). After the ten minutes the host sends `detachDelegation` (the command is the same each
  time, repeated at most every 30 seconds); the service refuses it before the time (`DELEGATION_WAIT_ACTIVE`) and otherwise releases the task with the outcome
  `detached`, "<Bot> has not finished yet", and the target's reply is posted in the Chat when it comes. The timer is the host's because a waiting task is not in
  `listHostTasks`, so only the host's own private record knows it is waiting; no job was added to Cloud Tasks.
- Cancelling. The person who asked, the target's maintainer and the delegating Bot's maintainer can cancel the target's task (`canCancel`); the cancellation
  covers the family by `rootTaskId`: unfinished tasks made under it are cancelled (a running one is asked to stop), open delegations they made are
  `cancelled`, and the task that waits for it goes on at once with `cancelled`. Cancelling the waiting task itself does the same downward.
- Where it is shown. `taskView` carries `delegation` (the chain, "A -> B") on the target's task and, on a waiting task, `wait.kind: 'delegation'` with
  `wait.delegation {sharedBotId, taskId}`; the inbox skips such waits (nobody is asked anything); `listTriage` rows carry `delegation`. Existing clients
  parse a wait by `messageId`, which stays, and ignore a kind they do not know.
- Screens (B8b). Every client reads the fields above by the same rules, each in its own code: Web and Desktop in `packages/teams/delegation.ts` (the rules), `BotRunCards.tsx`,
  `BotMemberDetails.tsx` (the Bot directory and a Bot's details), `TriageLogDialog.tsx` and, on Desktop, `BotAutoReplyField.tsx`; iOS in `TeamDelegations` and `TeamBotMemberText.delegation`
  (`TeamBots.swift`), `TeamBotViews.swift` and `TeamTriageLog*.swift`; Android in `TeamDelegations` (`TeamBots.kt`), `TeamBotEntry.delegation`, `TeamBotComponents.kt` and `TeamTriageLog*.kt`.
  A task another Bot asked for says "Delegated: A -> B" when its chain has two Bots or more, and keeps saying it once it has ended. A Bot is named by its card as the Team lists it now, else by the name
  the chain carries, and an id is never shown as a name (a handoff-made chain carries ids): a Bot with no name to give is "Bot". The task that waits says "Waiting for B" where a question to a person would say
  "Waiting for you . expires ...": the wait has nothing to answer, so there is no answer control and no expiry; B is named by its card, else by the name the task it asked has in its chain, else "Bot". A row
  of the auto-reply log says "Delegated by A", A being the chain link whose id is `parentBotId`. Cancel on the card follows `canCancel`: the person who asked, the maintainer of the Bot that runs the task and the
  maintainer of `delegation.parentBotId`, read from the Team's Bot cards (the service still decides). The offline line (`hostWait`) of a target whose Desktop is off shows on the delegated task in the Chat as on any
  task. A published Bot's card and details say "Accepts delegation from" (No Bots, Any Bot of the Team, or the listed Bots by name; a listed Bot that is no longer in the Team is left out, and a list of such takes
  nobody) and, when it may ask any, "May delegate to"; the daily limit belongs to its maintainer and members do not see it. Publishing stays on Desktop. The native models read only `parentBotId` and `chain` of a
  delegation and `sharedBotId` and `taskId` of the wait, and a delegation they cannot read leaves the task or the log row as it was. The text is `teams_task_waiting_bot`, `teams_task_delegation_chain`,
  `teams_triage_delegated_by`, `teams_bot_accepts_delegation`, `teams_bot_accepts_none`, `teams_bot_accepts_team` and `teams_bot_delegates_to` in the catalog (five locales, every client). The iOS and Android vector
  tests refuse `reject-delegateTask-member` and `reject-detachDelegation-member` as they refuse any action a member may not send (an unknown action fails the contract); both now assert it by name.
- Tests (B8b): Web `teams-delegation-views.test.ts` (the chain, the run card in five languages, the wait, who may cancel, an offline target, a Chat with a delegated task, the log helpers, the Bot's card, the catalog words, the
  two vectors) and `teams-triage-log-ui.test.ts` ("Delegated by"); Electron `bot-reply-field.test.ts`; iOS `TeamDelegationTests` (and `TeamBotTests`, `TeamTaskUsageTests` for the `canCancel` argument); Android `TeamDelegationsTest`
  (and `TeamsBotTextTest`, `TeamBotsTest`).
- Runtime (H). `TeamDelegate` is registered only with an offer, on a route that continues; `TeamWaitKind::Delegation`; `Bootstrap.delegates` (strict, at most 20,
  absent for private data); `TeamOperationTool::Delegate`; `verify_continuation` accepts a delegation answer only with the tool present and no approval flag.
  The host's request carries `delegates` for `run` and `continue` and nothing else.
- Desktop. `process.ts` validates the `delegate` operation (a stable step, exactly `target` and `request`, a Bot that was offered) and the new wait kind;
  `connector.ts` offers and keeps `delegates`, sends `delegateTask`, pauses with `delegation`, resumes from the recorded outcome, sends `detachDelegation`,
  reports `capabilities.delegation: true`, and accepts `acceptDelegation`, `acceptFrom`, `delegateTo` and `delegation` in `publishLocalBot`; `service.ts`
  allows the two commands. `hosted-bot-service.mjs` is deliberately not extended: a hosted service never delegates. The publish wizard (`packages/teams/BotDirectory.tsx`,
  Desktop only) has a fieldset "Bots asking Bots" (not for a friendship's Bots): what the Bot takes from other Bots (none, any Bot of the Team, or the Bots ticked, at
  least one), the Bots it may ask, and its daily limit when there is one. Choosing a Bot brings back what it is published with, so publishing again keeps it; what was
  ticked of a Bot that is gone is not sent; the review names them and a change after it asks for the review again. The text is `tw_botdir_delegation_*`,
  `tw_botdir_accept_*`, `tw_botdir_delegate_to` and `tw_botdir_review_{accepts,asks}_*` in the catalog (five locales, Desktop and Web only).
- Operation ids. `operationIdOf` is now `hash([taskId, taskId, logicalStepId, category])`: the Desktop derives the id of a ledger write from the task alone,
  and the root task of a delegated task is another task. For every task that is its own root (all but the tasks another Bot asked for, by this phase or by a data
  handoff) the id is the same as it was; for those the id now agrees with what the Desktop derives.
- Local router delegation (`accept_from` in the Bot's own `bot.json`) is unchanged and independent.
- Codes a Bot may be told (as words): `DELEGATION_NOT_ALLOWED`, `_NOT_ACCEPTED`, `_UNSUPPORTED`, `_UNSUPPORTED_TARGET`, `_TARGET_UNAVAILABLE`, `_DEPTH`,
  `_LOOP`, `_LIMIT`, `_UNASKED`, `_WITHHELD`, and what `submit` says (`BOT_USE_REQUIRED`, `BOT_PARTICIPANT_REQUIRED`, `REQUEST_QUOTA_EXCEEDED`, ...).
- Tests: `apps/firebase/test/delegation.emulator.test.mjs`, registered in `scripts/test-teams.mjs` after `reply-rounds.emulator.test.mjs` (both-sided
  agreement and the person's own rights, depth and loops, the per-root count with a handoff, the daily limit, the wait and the hand-back, the ten minutes and
  `detachDelegation`, a refusal at the pause, an offline target, cancelling by each of the three parties and the cascade, cancelling the waiting run itself, the waiting Bot or the other Bot moving to another deployment, no round and no auto-reply, a Bot
  nobody asked, a held reply approved, rejected and run out, the directory); `shared-bot-commands.test.mjs` and `team-execution-contract.test.mjs` (the commands and
  the boolean capability); `teams.rules.emulator.test.mjs` (the record is server-only); `contracts/execution-vectors.json` (`delegateTask` and `detachDelegation`
  are host commands a member may not send); Web `teams-delegation-ui.test.ts` (the wizard's controls).
- Not here, or not verified. Not here: a hosted service never delegates; no Cloud Tasks job ends a wait (the host does, and the service holds it to the time). Not verified: a run on two
  real Desktops with a real model (the tests use the emulator, a fake service and a scripted provider, never a signed package); the B8b screens were not run on a device, a simulator or a signed Desktop
  (Web and Desktop render in tests; iOS is type-checked whole-app and its logic tests run on macOS, without the SwiftUI screens); the Android code and `TeamDelegationsTest` were written and read through but
  never compiled or run (Gradle was not started: the disk was too full). A known gap: a run that
  is still marked waiting after its own 24 hours (its Desktop was off all that time) is queued again by a reply that comes late, with a new deadline, instead of the
  reply being posted in the Chat; `waiterOf` does not read the deadline.

## What a Bot may use of its maintainer's Desktop (Bot scope)

What a shared Bot reaches of its maintainer's Desktop in a Team is four layers that each only narrow: the local Bot's own upper bound
(`scope` of `bot.json`: Local Apps and datasets), the scope the maintainer published, what a Chat narrows it to, and, for datasets, a live
LingXi Data grant. Nothing is shared by default: a Bot published without a scope has an empty one (no fallback for older documents).

- Data (`bot-scope.mjs`, `packages/teams/bot-scope.ts`): `sharedBots/{id}.scope = { localApps: [{id, name}], datasets: [{source, dataset,
  write, reportReview}] }`, at most 64 of each, an id or dataset once, ids and names only. It is part of the Bot's revision: `publishSharedBot`
  takes `scope` (left out = empty), a publication replaces it and a handover resets it, so a run that started under the old one is revoked
  like any republished Bot's. Hosted services refuse a non-empty one (`SCOPE_UNSUPPORTED`). The Chat holds
  `conversations/{id}.botScopes[sharedBotId] = { revision, localApps?: string[], datasets?: [{source, dataset}] }`: a list that is left out
  narrows nothing, `[]` allows none, and lifting keeps only the `revision`. Members set it with `setConversationBotScope
  { conversationId, sharedBotId, expectedRevision, scope | null }` (the people who may use `setConversationAutoReply`; the Bot must be in the
  Chat: `BOT_PARTICIPANT_REQUIRED`; `BOT_SCOPE_REVISION_CONFLICT`; only within what the Bot published: `SCOPE_NOT_PUBLISHED`). The field is
  written by the service alone, the list projection of `team-messages.mjs` carries it, and neither the rules nor the indexes change.
- Service: `effectiveScope(published, narrowing)` is what `validateTask` answers as `scope`. `hostDataOffers` offers a dataset only inside it
  and with a grant (a write needs the `write` flag and an edit grant, a report review the `reportReview` flag), `hostDataOperation` refuses
  outside it, a handoff receiver needs the dataset in its own scope of the target Chat, and a task caused by a dataset is revoked
  (`TASK_REVOKED`) when the dataset leaves the Bot's scope.
- Desktop (`teams/connector.ts`): publishing checks `scope` against the local Bot (`SCOPE_OUTSIDE_BOT`; a write or a report review the Bot
  does not have is refused too) and a run checks the Bot's bound again as it is then: dataset offers are cut to it and `data_*` operations
  outside it answer `DATA_ACCESS_REQUIRED`. `TeamHostBootstrap.localApps` (at most 16, left out when empty) is the bound, the service's
  `scope` of the last `validateTask` and the installed apps; every request is checked against all three again.
- Worker operations (`teams/process.ts`): `local_app_tools { app }` lists the tools of a running offered app (as many whole tools as fit in
  64 KiB), `local_app_call { logicalStepId, app, tool, arguments }` calls one through the injected `TeamLocalApps` (`LocalAppMcpManager`
  in `index.ts`). An app that was not offered is a protocol violation, not an answer the model sees. The answer is `{ content: [{type: 'text',
  text}], isError? }` and at most 64 KiB (`OPERATION_BYTES`): only text is carried (an image or file is named by its kind and place), text
  that does not fit is cut at the end on a character boundary and a last block says so, `isError` is the app's own word and is passed on, and
  the operation `succeeded` when the app answered. Refusals: `LOCAL_APP_UNAVAILABLE`, `LOCAL_APP_TOOL_UNAVAILABLE` (also when the tool
  changed since it was listed), `LOCAL_APP_CALL_LIMIT` (64 calls a task), `OPERATION_CONFLICT` (the step again with another call),
  `LOCAL_APP_CALL_REJECTED` (the app or its host answered invalid params: nothing was done, the step may be tried again).
- A step is called once: `TeamPrivateStore.beginAppCall` writes `app-calls/<taskId>/<step>` before the app is asked and `finishAppCall`
  completes it with exactly what the worker is given. The same step again answers the record; one that began and has no end (the app,
  the process or the machine stopped) answers `LOCAL_APP_CALL_UNKNOWN` and is never repeated. The records go with the task's delivery.
- Clients: `SharedBotCard.scope`, `TeamConversation.botScopes` and `effectiveBotScope()` give a screen what the Bot may use in a Chat
  (ids and names only; a dataset is named by its id, which is all a scope holds of it). Desktop and Web (`packages/teams`) show and set it
  (B4c-1), and so do iOS and Android (B4c-2, below); all of them share the `teams_bot_scope_*` strings (with their iOS `%@` key and `_fmt` sibling).
  - Shown: `BotScopeSummary` lists the Local App names and the dataset ids of a Bot, a dataset it may edit with a "Can edit" badge, or says
    "Uses nothing from its maintainer's computer". `BotMemberDetails` shows it on the Bot cards (the team's Bots, Member home: what the
    maintainer published) and in the Chat's composer (`conversation` given: `effectiveBotScope(published, botScopes[id])`, with "Limited in
    this chat" when the Chat leaves out something that was published).
  - Set per Chat: a header button beside the auto-reply one opens `BotScopeDialog` for a person who takes part in a group, channel or Bot DM
    (the guard `inBotChat` is the one of auto-reply): one section per Bot of the Chat that published something, its Local Apps and datasets as
    checkboxes ticked where the Chat allows them. Save sends `setConversationBotScope` with the `expectedRevision` the Chat holds (0 with no
    entry) and `narrowingOf(published, ticked)`: a kind with nothing left out is not narrowed (what is published of it later is allowed too),
    nothing left out of either is `null`, and only published ids are named. "Allow everything shared" sends `null`. A section starts again from
    the Chat and the Bot whenever either changes, so the ticks always show what the service holds; `BOT_SCOPE_REVISION_CONFLICT` and
    `SCOPE_NOT_PUBLISHED` read the Bots again (`refreshBots`) and say so in the dialog, and the Chat's side arrives by its listener.
  - Published (Desktop only, `BotDirectory`): the wizard offers the Local Apps and datasets of the local Bot's own scope
    (`LocalPublishableBot.scope`, from `TeamConnector.localBots()`: an app is named as it is installed here and one that is not installed is
    left out), nothing ticked by default; a dataset offers "Can edit" and "Can open review cases" only where the Bot has them, and review
    needs edit. Publishing again ticks what is published now, says what the Bot's own scope no longer has ("stops being shared"), and sends
    nothing wider than the Bot's scope as it is when published. The review lists it, the consent and the host hint then say the Bot uses what
    is shared from this computer. Hosted services have no such section (`SCOPE_UNSUPPORTED`; `SCOPE_OUTSIDE_BOT` is told in `errors.ts`).
  - Mobile (B4c-2): the first two of these natively on iOS and Android, and no publishing (that stays on Desktop). What is checked has one source
    of truth: the effective-scope cases are `contracts/bot-scope-vectors.json` (`cases`), and the `setConversationBotScope` command rows live in
    `contracts/execution-vectors.json` with every other member command, which both native validators already run, on every command sent and in
    their tests (the Web and service tests read the same rows).
    - Decoded leniently: `scope` on a card is absent or a scope (a list that is there and unreadable breaks the card, as any field of it does);
      `botScopes` on a Chat keeps an entry only with a safe-integer `revision` of at least 1 and well-formed lists, drops what it has no use for, and
      an entry it cannot read is no narrowing and never hides the Chat (`TeamBotScopeRules.decode`; iOS `TeamBotScope.swift`, Android
      `teams/TeamBotScope.kt`, both pure and tested against the vectors).
    - Shown: `TeamBotScopeSummary` is part of the Bot's member info (`TeamBotMemberInfo`), so the team's Bot cards, the new-chat picker and the list of
      Bots show what the maintainer published; the Bot DM header and the Bot rows of the conversation details show what the Chat leaves of it, with
      "Limited in this chat".
    - Set per Chat: in the conversation details, next to the auto-reply section, "What Bots can use in this chat" opens the editor, for a person who
      takes part in a group, channel or Bot DM that has a Bot that published something, and never in a friend chat (iOS: pushed in the details'
      navigation stack, `TeamBotScopeEditor`; Android: a sheet, `TeamBotScopeSheet`, which replaces the details sheet). One section per Bot, a
      switch (iOS) or checkbox (Android) for each Local App and dataset, with "Can edit" read as part of the dataset's name; Save sends
      `expectedRevision` and `narrowingOf(published, ticked)` exactly as the Web dialog does, and "Allow everything shared" sends `null`. The ticks start
      again whenever the Chat's entry, the Bot's published scope or what the Chat allows changes (`TeamBotScopeBasis`), and after a write the Bot's
      controls wait until the Chat's listener has brought the revision the write made (as the auto-reply switches wait). `BOT_SCOPE_REVISION_CONFLICT`
      and `SCOPE_NOT_PUBLISHED` read the Bots again and are told under the Bot (`teams_error_scope_conflict`, `teams_error_scope_not_published`; the
      same words as `tw_error_scope_*`, for the native clients).
    - Validated like the service: `setConversationBotScope` is a case of `TeamExecutionContract.validate` on both platforms (`scope` must be there,
      `null` or an object of at most 64 unique Local App ids and 64 datasets, each named by its source and its id and nothing else).
- Runtime (H): not part of this phase. The host's bootstrap refuses unknown fields, so `localApps` and the two operations need an H change
  and a pin bump before a Bot is published with a Local App.
- Tests: `apps/firebase/test/bot-scope.test.mjs`, `bot-scope.emulator.test.mjs` (also in `scripts/test-teams.mjs`), `handover` and
  `hosted-bot-service` emulator tests, `apps/web/test/teams-bot-scope.test.ts` (the effective-scope cases in `contracts/bot-scope-vectors.json`, the
  command rows in `execution-vectors.json`, which the native clients check with every other command), `teams-host.test.ts` (`Bot v3 B4a`; the
  Bots a computer can publish and their scope), `apps/web/test/teams-bot-scope-ui.test.ts` (what is shown, the narrowing dialog with its
  revisions, conflicts and refusals, the publish section; Desktop and Web share the components), and the native clients' own suites:
  `apps/ios/native/Tests/TeamBotScopeTests.swift` and `apps/android/native/app/src/test/java/com/lingxi/code/teams/TeamBotScopeTest.kt` (the same
  cases, decoding, ticks, who can limit what, what is sent and refused). The editors and summaries themselves are type-checked on both platforms
  but have not been run on a simulator or a device.

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
