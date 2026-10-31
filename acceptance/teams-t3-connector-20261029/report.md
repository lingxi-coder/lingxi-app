# Teams T3 authenticated connector prerequisite — 2026-10-29

The trusted macOS connector now consumes the backend's one-shot start snapshot,
persists its attempt/start/result state and runs the actual dedicated worker.
**T3 remains incomplete and disabled.** This is a single-queue-item executor
port, not a product account integration, queue service, approval/question broker
or four-client feature. All worker tools stay denied.

Companion commit **`e8adfe0785e1cceef630df475015ff1b1c180667`**, parent
`43031991ba7246b2f1b0f38e05632009334d3aaa`, is local on `codex/teams-runtime`.
The product changes/evidence commit containing this report is based on
`def9cf9cd2991f5668aa8c17ea9b556d257c0b54` on `claude/teams`. Product Cargo
files remain unchanged, pinning `eed08ca06c35371c1eb7ea5b02ebc10bcae63a12`.
The companion still pins SDK `ff71b2ec1139bc67cc8719d78b1735aca967bbe8` and
has no Cargo.lock change. No dependency is added; the client uses the pinned
SDK's HTTP-backend reexport. API: `harness-runtime/docs/TEAM_CONNECTOR.md`.

## Implemented boundary

- Only the backend transaction changing claimed to running returns
  `execute:true` and the versioned execution snapshot. Owner, binding revision,
  target, attempt, original message and human UID/generation/text are validated
  before private runtime preparation. Claim text is deliberately adversarial in
  the fixture and never reaches the model. Repeated live starts cannot supply a
  snapshot/second permit. Human Unicode/punctuation UID rules now match the
  backend; existing private scope digest order is preserved.
- Firebase credentials remain in the parent. The host account UID/epoch port and
  synchronous session cancellation fence awaited refresh/account changes. HTTPS
  is required except literal loopback. Redirects, ambient proxies and automatic
  transport retries are disabled. Token/request/body waits share five seconds;
  JSON bodies are bounded to 64 KiB; error bodies are withheld.
- A canonical private journal creates/syncs immutable intent, pre-dispatch start,
  permit and result records. The stable key excludes binding revision/generation
  so a change cannot mint a replacement invocation; the stored target still
  validates both. Locks prevent concurrent local drivers. Symlink/hardlink,
  partial JSON and non-private storage fail closed. Saved permits never launch
  on restart. The independent process-started tombstone remains mandatory.
- A result is synced before finish. Result-only retry uses no model credential
  and repeats exactly the same text/outcome/attempt. Lost finish ACKs do not
  duplicate execution or replies. Ambiguous start/worker failure has no automatic
  replay, replacement quota or fixed successful reply.
- The backend now lets the current owner manually settle an expired claimed
  attempt when a conservative local start marker preceded an unsent request.
  Live/reclaimed leases reject recovery; transactions fence late claim/start.
  The operation never executes work and the connector never calls it implicitly.
- The original absolute lease is captured once into the process deadline. Live
  authorization polls require that same lease; revocation, auth/protocol/network
  failure, account/external cancellation and future drop invoke cancellation/kill
  guards. Polls wait two seconds after successful responses; observation can take
  that delay plus the five-second token/request bound and cleanup. Reaping is
  allowed up to two seconds; future drop does not await it. These are not hard
  real-time termination/reaping guarantees during OS/filesystem/scheduler stalls.

## Final validation

**101 distinct selected tests passed, zero failures.** Counts are disjoint:

| Suite | Passed | Boundary |
| --- | ---: | --- |
| New connector | 13 | Actual worker/SDK with synthetic loopback HTTP/SSE |
| Explicit connector emulator fixture | 1 | Actual Auth-emulator ID token, backend HTTP/Firestore and worker |
| Original process integration | 15 | Clean env/credentials, strict pipes/FDs, quota, timers, cancellation, no replay |
| Original private profile | 13 | Private roots/credentials/memory, actual macOS sandbox Bash and Git fixtures |
| Backend and rules emulators | 52 | Entire selected Team rules/member/message/Bot suites, including four new cases |
| Desktop/Web Firebase SDK | 7 | Real SDK sessions/listeners against the isolated emulator |

The ordinary connector run intentionally marks the explicit Firebase fixture
ignored (13 passed, one ignored). That same case then passed with
`--ignored --exact actual_firebase_backend_and_worker`; its other 13 tests were
filtered. There is no unexecuted fixture counted as passing. This is not the
entire repository suite; older 721/1440/visible-text/recovery totals are historical
and were not fully rerun in this checkpoint.

The real emulator fixture used only `demo-lingxi-teams-t0` with explicit loopback
Auth/Firestore endpoints. A human ID token creates the actual execution evidence;
an owner ID token claims/starts/finishes through the real handler. Invalid token,
member claim and forged permit requests return 401/403/400. One model request
produces exactly three messages (human/status/reply), one trigger, one start and
two exact finish requests, with one reply/checkpoint. These are emulator accounts
and signatures, not production Firebase/account/service acceptance. The account
port is a test implementation, not the product AccountService.

All final process/connector tests ran against the freshly built **normal
Desktop-feature** standalone worker, not a binary relying on dev-test feature
unification. Worker SHA-256 before and after all tests:
`065919a51882401d442c9f2854d438c32aea640c6aceadebd3530e499a74ac04`.
It is an unsigned local debug artifact built before the companion commit; the
nine final source/manifest/document bytes were verified against that commit.
Final test-binary hashes, commands, JSON metadata and source hashes are sealed
in `results.json`. This is not a signed product/native package.

Desktop and Mobile-feature library checks and the worker build pass. Seven Rust
files pass formatting, three JavaScript files pass syntax checks, and diff checks
pass. Independent read-only review covered 12 implementation/glue/test/fixture/
document files: APPROVE for the disabled prerequisite, no unresolved findings.
The reviewer did not run tests/build/network; final test evidence is parent-run.
Two findings were fixed: original-human UID mismatch and expired-claim manual
recovery. Documentation timing/reaping wording was corrected after review.

Ten compressed logs include final tests/build/checks/format evidence and a
separate historical outer-sandbox failure log. The initial ordinary tool sandbox
denied loopback bind (0 passed, 10 failed); it is not acceptance. Approved local
fixture execution outside that outer sandbox passed. Initial diagnostic logs and
own build outputs remain under `harness-runtime/target/teams-connector-evidence`.
No automatic approval rejection occurred.

## Remaining gates

Product account/session and queue/discovery integration, manual recovery UI,
owner-only approval broker and separately authenticated original-human question
broker remain. Generic owner/TUI/Remote Control answers cannot substitute for the
human. Before tools can be allowed, detached-process containment, reliable
deadline/revocation termination and independent CPU/memory/disk/input-total-token/
billing ceilings still need work. Only macOS and selected static Anthropic auth
are admitted. Other auth/providers, all four T3 client UIs, production Firebase/
provider, host crash/offline/rejoin/revocation and signed/native/device acceptance
remain open. No push, merge, deploy or IAM operation was performed; product pins
and entry points are unchanged. No production account, saved test session,
browser or device was used, and no cache/source/worktree cleanup occurred.
