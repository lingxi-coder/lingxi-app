# Teams Firestore SDK message cache acceptance

Date: 2026-10-30 (America/Los_Angeles). Parent: `76f1bcbb29d1b9001d11f2a6fcf63d7c0598259d`.
Scope: local source delivery on `claude/teams`; no deployment, push, main merge,
new signed macOS release, physical-device or Store acceptance.

## Result

Ordinary messages, conversation membership and read marks now come directly from
Firestore SDK listeners and its persistent cache. The selected message listener
applies SDK document changes to an ephemeral view. There is no application
message cursor, durable body replica, ordinary HTTP list/sync read path, or fixed
message refresh timer. Cache status combines directory and selected-message
metadata; a directory becoming current cannot mark stale messages current.

Pending HTTP commands retain their exact identities, payloads, claims and receipts
in command-only storage. The old history fields are removed without dropping
pending work. Bot execution journals and immutable context/result receipts remain.
Pending-command backoff, Bot heartbeats and execution lease observation remain
because those operations are separate from ordinary message synchronization.

| Client | Actual verification | Result |
| --- | --- | --- |
| Web | Chrome, actual Web API/model and multi-tab SDK cache; browser process closed and reopened offline | PASS |
| Desktop | Actual bundled sandboxed Chromium SDK data page, private partition, SDK termination/page reopen without a new custom token | PASS |
| iOS | Full product build; actual `TeamMessagingModel` and native SDK, two independent XCTest host processes on the owned simulator | PASS |
| Android | Full Direct Debug APK with native SDK verification; actual model/SDK in two independent instrumentation processes on the owned AVD | PASS |

Every client loaded 125 messages, reopened from SDK disk cache offline, then
received an edit to the oldest message, a recall of the second message, a remote
read watermark of 120, and membership revocation that cleared the visible view.
Desktop additionally verified that logout removes the private SDK authentication.
Its private Session download handlers are released on close and failed login.
The ordinary HTTP read counter remained **0**. Test data used only
`demo-lingxi-teams-t0` on loopback emulators; no real-provider credentials were
used by these cache tests.

Actual browser IndexedDB v1-to-v2 upgrade retained the pending command ID and
payload while removing conversations, message bodies and the old cursor. The
browser inspected the resulting records rather than a mocked storage adapter.
SDK authentication profiles and caches remain private, Git-ignored artifacts.

## Regression evidence

- Desktop/Bot related regressions: **132/132**; Web Teams regressions: **29/29**.
- Backend/API/default rules: **57/57**; Desktop SDK: **5/5**; Web SDK: **3/3**.
- Android unit tests: **1,275/1,275**, 155 suites; cache instrumentation: **1/1**
  in each process, zero skips.
- iOS selected tests: **12/12** in the warm process and **1/1** in the offline
  process, zero skips. The final run has no additional diagnostic listener.
- Real sandboxed Bot host/worker with a local synthetic model: four model turns,
  one owner approval, rejected non-origin answer, exact answer retries, lost
  finish acknowledgement followed by durable restart/retry, and offline queued
  execution once. All worker processes drained and pending results settled.
- Actual TypeScript language-server diagnostics cover every modified TS/TSX file:
  zero errors and warnings. Native builds have no Teams source diagnostics;
  pre-existing non-Teams iOS warnings remain outside this change.
- Web and Electron production frontend builds pass. Locked/offline Cargo
  metadata, feature resolution, Rust formatting, JavaScript and shell syntax
  checks pass. The two product entrypoints exactly match the pinned Harness SDK.
  A build/provenance regression verifies the shared locked build command and
  rejection of changed inputs/binaries or an unauthorized development source.

The Harness pin remains `b09033fa446dfdee48e71ea18ca16095b068a353`; its worktree
is unchanged. Shared Rust build wiring is validated by resolution and command/
provenance tests, without claiming a newly compiled or signed macOS native package.

The previously approved performance repairs remain in this scope: bounded/fair
host admission through process drain, resumable Bot discovery and queue/recovery
backoff, serialized asynchronous private-journal I/O, batched authorized Bot state
reads, bounded message rendering, cancellable Android HTTP, bounded push fanout,
message-text index exemption, lazy Teams/Firebase loading and the shared Rust
build graph. Their related regressions are included above.

## Findings resolved during verification

The iOS SDK path exposed Swift overlapping projection accesses and attempted JSON
encoding of Firestore-only timestamp metadata. The model now separates reads
from mutations, selects DTO fields and validates JSON values. Both native clients
combine directory/message cache metadata, and all clients invalidate a previously
accepted membership when its cached record disappears or changes generation.

The Apple emulator configuration must set the local host before disabling TLS.
One emulator failure also reported `CANCELLED` after a revocation had committed.
The test controller accepts that specific cancellation only after an authoritative
read confirms the member is absent. Failed runs remain in the ignored output
directory; final tests retain strict HTTP and UI assertions.

## Evidence and replay

[checks.json](checks.json) contains the sanitized results and artifact hashes;
[source-sha256.json](source-sha256.json) binds them to the source inputs. Detailed
local logs are under `output/teams-sdk-cache-20261030/` and the task's private
temporary dependency checkout. Private tokens, accounts and browser storage are
excluded from these tracked records.

The existing Web dependency links in this worktree point to a removed checkout.
Web checks therefore used a matching-lock dependency checkout with hash-verified
copies of the current source. Repository dependency manifests and lockfiles were
not changed to work around that local condition.

The cache fixtures are `apps/firebase/test/support/teams-cache-server.mjs`,
`apps/web/test/fixtures/teams-cache.html`,
`apps/electron/test/fixtures/teams-cache-main.ts`, `TeamFirestoreCacheTests` and
`TeamFirestoreCacheTest`. Use fresh demo emulators, the explicit native opt-ins
(`TEAM_CACHE_EMULATOR=1`, `TEAM_CACHE_PHASE=warm|offline` on iOS;
`teamsCacheEmulator=true`, `teamsCachePhase=warm|offline` on Android), and run warm
and offline phases as separate processes. Do not run suites that clear Firestore
concurrently with the cache fixtures. `apps/firebase/scripts/test-teams.mjs`
runs the backend and SDK regression suites.

## Release boundary

Deploy the trusted backend invalidation writes, default-database signal rules
and index changes before distributing the updated clients. Current production
is still the previously published T3 baseline. SDK caches contain previously
loaded documents, are subject to eviction, and cannot be remotely erased; cached
data is not a new server authorization.

References: [SDK listeners](https://firebase.google.com/docs/firestore/query-data/listen),
[offline cache](https://firebase.google.com/docs/firestore/manage-data/enable-offline),
[Web persistent cache](https://firebase.google.com/docs/reference/js/firestore#persistentlocalcache),
[realtime query scaling](https://firebase.google.com/docs/firestore/real-time_queries_at_scale).
