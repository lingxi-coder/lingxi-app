# Local App remote access implementation and verification

Date: 2026-10-28. Branch: `codex/local-app-remote-access-20261028`.
Isolated checkout: `worktrees/local-app-remote-access-20261028/lingxi-app`.
Base: `bdc3533167a9d0ed4fb138836305cb9b4b2155fd`.
Six code lanes were assigned GPT-6.1 Terra with xhigh reasoning as requested.

## Delivered source behavior

- Desktop shares individual installed Local Apps through the existing approved
  Remote Control account/device connection. Web, Android and iOS expose a shared
  app catalog and browser launch without starting the backend on the viewer.
- The loopback gateway serves existing static/server releases, browser SDK,
  AG-UI, streaming uploads/responses and registered WebSocket services. Trusted
  forwarding headers and registered-loopback redirects preserve backend behavior.
- Every browser launch has an independent DNS origin and a short-lived fragment
  ticket. Account/device/app/release/run/session checks, revocation generations,
  HttpOnly cookies, request bounds and specific Agent cancellation protect the
  authority boundary across async work and transport loss.
- Optional RTC accelerates bounded credential-omitted GET/HEAD reads; HTTPS
  handles credentialed reads, all writes, large responses and streams. Writes
  are never replayed by RTC fallback. Abort propagation is tested under forced
  garbage collection and retained Request objects.
- Mobile deployment remains available. Desktop is recommended for long tasks.
  Default browser is the default opening mode, with Android chooser and iOS
  optional system in-app browser. New UI strings cover five existing locales.
- A local frp configuration generator creates private files, verifies trusted
  relay configuration, preserves Host and keeps TLS termination on Desktop.
  Public infrastructure and credentials are supplied separately.

Deployment contract and commands: [LOCAL_APP_REMOTE_ACCESS.md](../LOCAL_APP_REMOTE_ACCESS.md).

## Passing verification

| Check | Result | Evidence boundary |
| --- | --- | --- |
| Final feature suite | 57 passed, 0 skipped/failed | Includes gateway, host, SDK, Agent, RTC, bindings, lifecycle, service proxy and tunnel config |
| Real Chrome gateway tests | 2 passed, included in 57 | Actual local HTTP and TLS proxy; test resource adapter; no mocked fetch |
| Web complete tests | 174 passed | Python with tomllib and installed offline test renderer dependency |
| Bridge complete tests | 342 passed, 5 skipped | 347 cases; Python 3.12 plus Cargo available on PATH |
| Rust Remote Control complete tests | 129 passed | Default features, local HTTP fixtures allowed; no production service |
| Desktop TypeScript | Node and renderer pass | Final source after proxy/RTC changes |
| Desktop production build | Pass | Unsigned JS build, not installed signed-package acceptance |
| Existing Local Apps Electron GUI | Pass | Both immutable baseline and this branch, using existing release bridge-server |
| Project creation Electron GUI | Pass after fixture update | New LocalApps API methods added to the mock |
| Android and iOS Rust FFI | Both cargo check pass | Host compiler; no matching device-library/binding regeneration |
| Swift | Parse and focused simulator SDK checks pass | Focused view check used stub FFI types; not full application typecheck |
| Tunnel generator | Tests, actual CLI and TOML parsing pass | No installed frpc or live relay verification |
| New locale keys | Canonical/platform values agree in all five locales | Unrelated existing entries preserved |

The final feature command, from `apps/electron`, is:

```sh
LINGXI_TEST_PLAYWRIGHT_MODULE=/absolute/path/to/playwright/index.mjs \
LINGXI_TEST_CHROME='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' \
node --expose-gc --import ../../packages/bridge-client/node_modules/tsx/dist/loader.mjs \
  --test --test-concurrency=1 \
  test/local-app-remote-*.test.ts test/remote-local-app*.test.ts \
  test/local-app-remote-tunnel.test.mjs test/local-app-remote-browser.test.mjs \
  test/local-app-service-broker.test.ts
```

Rust: `CARGO_TARGET_DIR=<existing-target> cargo test --locked -p remote-control-client -j 2`.
Desktop: `npm run typecheck && npm run build` from `apps/electron`.
Bridge: `npm run build && npm test` from `packages/bridge-client`, with a Python
version supporting tomllib and Cargo on PATH.

Session-local receipts were saved under `/private/tmp/`:
`lingxi-local-app-remote-final-feature-tests.log`,
`lingxi-local-app-remote-bridge-final.log`,
`lingxi-local-app-remote-rust-full-tests.log`, and
`lingxi-local-app-remote-desktop-build.log`.

## Full Desktop suite audit

The exploratory complete Desktop run had 2,696 cases: 2,651 passed, 36 failed and
9 skipped. The failures were classified against an independent Git archive of
the immutable base, with its own built bridge-client and shared installed Node
dependencies:

- 32 failures reproduced on unchanged baseline source, primarily locale-dependent
  UI assertions and existing rendering/settings cases.
- 3 were environment failures: the isolated checkout lacked a bridge-server
  executable and its default Python lacked tomllib. The corresponding tests
  passed with the existing release executable and a suitable Python.
- 1 was introduced by the expanded LocalApps API: the project creation mock
  lacked sharing/configuration methods. The fixture was updated, and its actual
  Electron interaction test passed afterward.

The original full run is not claimed as passing. A separate broad Local App /
Remote Control pass had 553 passes, five skips and one cancelled unchanged
process-guardian case after a timeout left its child process hanging. Only that
owned test child was terminated. The same guardian test had passed in the earlier
full run; no guardian implementation was modified. Final feature gates above
completed with no failures or cancellations.

The prerequisite account theme repair is its own commit, `2c06e4fe1`: one unused
import and two unsupported token references already present in the base prevented
renderer typechecking. The feature remains separate from that small repair.

## Pending release and deployment acceptance

No public DNS, certificate, frp server, TURN service or cloud resource was created.
No signed Desktop installer, APK or iOS application was built or installed for
this change. Existing native archives predate `listApps`/`openApp`; matching
UniFFI bindings and device libraries must be regenerated before native builds.
Available disk was about 10 GiB after host checks, so large native rebuilds were
not started.

Actual cross-network Mobile/Desktop access, real TURN, background/long-duration
behavior, native permission UI, device photo/file return and signed-package
acceptance remain unverified. The current Mobile Agent pending input is browser
IndexedDB state: iOS in-app continuation cannot silently move it to the default
browser. This limitation is stated in the native handoff UI and Mobile contract.

The locale generator's full check still reports pre-existing resource drift.
Feature keys were reconciled and unrelated iOS catalog-only entries preserved;
this task did not replace or normalize unrelated resource content.

All feature edits and commits are in the isolated checkout. No feature changes
were merged or pushed to main, and concurrent primary-checkout source edits were
preserved.

## Follow-up review and fixes

The implementation and prerequisite account repair were reviewed against
`bdc3533167a9d0ed4fb138836305cb9b4b2155fd`, including 83 changed files. Stage 1
checked the requested Desktop hosting, browser choice and continued Mobile
hosting before Stage 2 examined authority boundaries, transport semantics,
lifetimes and client races. Four GPT-6.1 Terra xhigh lanes reviewed the gateway,
RTC, Mobile and Web/protocol paths; the parent reviewed host integration and
Agent ownership. The host changes received a further independent review after
fixes, including a late runtime activation after a launch timeout.

Seventeen findings were fixed: five HIGH and twelve MEDIUM, with no CRITICAL or
LOW findings. The references below point to the corrected source locations at `8ee542855`;
the defective implementation was reviewed at `4946747f7`.

| Severity | Location | Issue and correction |
| --- | --- | --- |
| HIGH | `apps/electron/src/main/local-apps/host.ts:849`, `apps/web/src/remote/appsController.ts:106`, `packages/remote-control-client/src/connection.rs:709` | A 30-second viewer timeout rejected legitimate first launches, while host startup had no matching deadline. Use a 900-second host deadline and 930-second client deadline, abort pending startup, check cancellation before ticket issuance, and reconcile even an activation that commits after the public promise rejects. |
| HIGH | `apps/web/src/remote/LocalApps.tsx:12` | Passive unmount cleanup allowed an obsolete host/account launch to navigate its reserved tab before cleanup. Layout cleanup now invalidates synchronously. |
| MEDIUM | `apps/android/native/app/src/main/java/com/lingxi/code/remote/RemoteControlManager.kt:243` | Pending launches could complete between initiating connection close/switch/forget and receiving the new state. Advance the generation at action initiation. |
| MEDIUM | `apps/android/native/app/src/main/java/com/lingxi/code/apps/UnifiedAppRuntimePage.kt:95` | Agent wake consent forced a default-browser opening even when the pending input belonged to another browser. Preserve the pending open decision and chooser, with localized same-browser guidance. |
| MEDIUM | `apps/web/src/remote/appsController.ts:108`, `apps/android/native/app/src/main/java/com/lingxi/code/remote/RemoteControlManager.kt:356` | A launch could return a different release from the catalog selection. Both clients now verify the selected revision, matching iOS. |
| MEDIUM | `apps/web/src/remote/appsController.ts:94` | Closing the reserved tab left Open disabled for the entire startup timeout. Watch closure, release the UI and ignore the old reply. |
| MEDIUM | `apps/electron/src/main/local-apps/remoteGateway.ts:260` | WebSocket requests retained untrusted forwarding metadata. Strip it and supply only the validated session host/protocol. |
| MEDIUM | `apps/electron/src/main/local-apps/remoteGateway.ts:68` | One transient loopback bind failure poisoned all future launches. Clear the rejected listener state so a later launch can retry. |
| MEDIUM | `apps/electron/src/preload/localAppSdk.ts:292` | Service WebSocket paths could escape their named prefix through URL normalization. Apply the HTTP helper's path containment validation. |
| HIGH | `apps/electron/src/main/local-apps/serviceBroker.ts:61` | Fetch decoded compressed bytes while the proxy retained encoded representation metadata, breaking compressed application assets. Remove stale encoding/length/digest/ETag metadata, request identity for ranges and reject compressed partial responses. |
| MEDIUM | `apps/electron/src/main/local-apps/remoteRtc.ts:78` | Browser cancellation did not stop the corresponding Desktop resource. Send request-specific cancellation and reject duplicate active request IDs. |
| MEDIUM | `apps/electron/src/main/remoteControl/peerHost.ts:128` | Hidden-window startup could post an offer after its peer had closed. Check pending identity before posting and clean up synchronous startup failures. |
| MEDIUM | `apps/electron/src/main/local-apps/remoteRtc.ts:128` | Responses discarded during RTC fallback could leave their upstream body running. Abort/cancel them before HTTPS fallback. |
| HIGH | `apps/electron/src/main/local-apps/host.ts:972` | Injecting scripts materialized arbitrary HTML streams without a limit. Enforce 8 MiB per response and a 32 MiB shared budget, releasing it on error/cancellation. |
| MEDIUM | `apps/electron/src/main/local-apps/host.ts:964` | HTML processing manufactured bodies for HEAD/204/205/304 responses, including invalid Response constructions. Preserve empty bodies and cancel unused streams. |
| HIGH | `apps/electron/src/main/local-apps/agentBroker.ts:640`, `apps/electron/src/main/local-apps/host.ts:1005` | Tracking only the last returned task let an idempotent terminal retry overwrite a still-active task; another session's retry could also cancel the wrong task on expiry. Cancel the broker's actual active tasks matching the exact browser session, and recheck session authority after asynchronous authorization. |
| MEDIUM | `apps/electron/src/main/local-apps/host.ts:1645` | Disabling background/Agent grants could stop a foreground browser, or retain an old background run forever when a pending open failed. Retain the exact run for active/pending browser use and reconcile it when the last access ends, including overlapping cleanup callbacks. |

No further definite source defects were found in the final review. This is a
bounded source-review result, not a claim that no defects exist.

### Follow-up validation

- Feature/peer-host regression suite: **98 passed**, including real Chrome HTTP
  and TLS bootstrap, new timeout/revocation/late-activation cases, Agent session
  ownership, compressed bodies and ranges. Tests used Electron 43.1.1's bundled
  Node 24.18 / Undici 7.28 to validate actual production fetch decoding, including
  gzip, Brotli, x-gzip, stacked codings and zstd.
- Full Web suite: **176 passed**; Web typecheck and production build passed.
- Full shared bridge suite: **342 passed, 5 skipped**; package build passed.
- Existing Agent broker/host/executor/SDK/assets and authoring regressions:
  **56 passed** under the bundled real Node 24.19 runtime.
- Full Rust Remote Control suite: **130 passed**; this compiles the changed
  shared request timeout and strict DTO paths.
- Desktop Node/renderer typechecks and production JS build passed.
- Compiler AST syntax checks covered **43 changed TypeScript/TSX files** with no
  syntax errors. Searches found no hardcoded secret assignments or console.log
  calls; empty catches were inspected in context rather than treated as defects
  solely by their shape.
- Changed Swift files passed frontend parsing; changed Android XML and all five
  canonical translation values were parsed/compared. Android launch-generation
  and revision regression cases were added, but native JVM/instrumented tests
  have not run against regenerated bindings.

`lsp_diagnostics` and `ast_grep_search` are unavailable in this environment;
project compiler diagnostics and TypeScript AST traversal were used instead.
Android's generated Kotlin bindings are absent in this isolated checkout and
the existing native archives predate the new APIs. Full native compilation,
fresh JNI/XCFramework generation and installed-device acceptance remain open.
For that reason the final review recommendation is **COMMENT**, rather than an
unqualified approval of every platform or of release readiness. No confirmed
CRITICAL/HIGH/MEDIUM/LOW source findings remain unaddressed.

Receipts: `/private/tmp/lingxi-local-app-review-feature-tests-final.log`,
`/private/tmp/lingxi-local-app-review-web-tests.log`,
`/private/tmp/lingxi-local-app-review-rust-tests.log`,
`/private/tmp/lingxi-local-app-review-bridge-tests.log` and
`/private/tmp/lingxi-local-app-review-desktop-build.log`. The existing Agent
regression receipt is
`/private/tmp/lingxi-local-app-review-agent-regressions-final.log`.

## Second independent review

The user explicitly invoked `code-review` again after the first repair commit.
The review covered the cumulative feature and then the new repair diff, using
independent gateway, lifecycle/Agent and client reviewers plus the parent's RTC
and integration inspection. The built-in reviewer role's configured model was
unavailable to this account; the review lanes therefore used the user's
requested GPT-6.1 Terra xhigh. Spec compliance remained the first stage.
The cumulative reviewed change now spans 85 files, including the guarded
atomic-write helper and its new regression test.

This round identified and fixed 13 further findings: one HIGH, eleven MEDIUM
and one LOW. Earlier passing tests did not cover these interactions. The
references below identify the corrected locations after this round.

| Severity | Location | Defect, impact and correction |
| --- | --- | --- |
| MEDIUM | `apps/electron/src/main/local-apps/remoteGateway.ts:252` | Named service Cookie paths were not mapped to `/__services/<name>`, breaking login and colliding across services. Map explicit/default paths, reject incompatible named-service `__Host-` cookies, and verify two services with the same Cookie name in real Chrome. |
| MEDIUM | `apps/electron/src/main/local-apps/host.ts:985` | Injecting HTML into 206 responses invalidated byte offsets, and transformed full HTML retained upstream validators. Preserve partial HTML exactly and remove stale representation hashes/ETags from injected HTML, including HEAD metadata. |
| MEDIUM | `apps/electron/src/main/local-apps/remoteGateway.ts:250` | All HEAD lengths were discarded. Preserve lengths for unchanged bodies; omit lengths that describe upstream compressed or uninjected HTML rather than the delivered representation. |
| MEDIUM | `apps/electron/src/main/local-apps/host.ts:973` | A named service's `Location: /login` escaped into the root application. Preserve the service prefix for root-relative redirects and retain query/fragment. |
| HIGH | `apps/electron/src/main/local-apps/agentBroker.ts:641`, `apps/electron/src/main/local-apps/store.ts:21` | Browser revocation during a broker lock wait still allowed queued chat deletion/archive/create or task admission; checking only before atomicJson also left a temporary-write-to-rename window. Carry the session signal through mutations, check after queue admission and at destructive/rename dispatch, and clean incomplete task/chat artifacts. Deterministic tests hold locks and temporary writes across revocation. |
| MEDIUM | `apps/electron/src/main/local-apps/agentBroker.ts:675` | Session cancellation waited behind the global ledger lock before aborting active work. Abort matching controllers synchronously, then persist terminal records through the queue. |
| MEDIUM | `apps/electron/src/main/index.ts:634`, `apps/electron/src/main/local-apps/host.ts:926` | Device or app withdrawal invalidated other devices/apps' in-flight launches. Track owner generations, target pending controllers/grants by owner/app, and retain global generations only for global withdrawal. Tests cover deleting an unrelated app as well as revoking another device. |
| MEDIUM | `apps/electron/src/main/remoteControl/localAppsBindings.ts:43` | A transient signaling failure after a new opening revoked every existing browser session of its owner. Revoke only the returned, unredeemed ticket. A real gateway/delegate test proves an old session still works while the rejected new ticket cannot redeem. |
| MEDIUM | `apps/electron/src/main/local-apps/host.ts:1450` | Desktop window closure could stop a run while remote opening was pending or appeared across an await. Coordinate window cleanup with startup, recheck run use, and keep the next cleanup runnable after a prior failure. |
| MEDIUM | `apps/electron/src/main/local-apps/host.ts:1386` | Failed Desktop cache/navigation setup stopped an already shared foreground run. Retain runs still used by remote browsers or pending openings on both failure paths. |
| LOW | `apps/electron/src/main/index.ts:758`, `apps/electron/src/main/local-apps/host.ts:906` | A windowless non-macOS process retained for a browser did not re-evaluate quitting when the last grant expired. Notify the existing idle checker after remote cleanup, which rechecks windows, viewers and background work. |
| MEDIUM | `apps/electron/src/main/local-apps/host.ts:805` | The 257th shared application made the entire strict catalog invalid. Enforce an explicit trusted 256-share quota, clear shares on confirmed deletion and prune deleted IDs on settings writes so stale entries cannot exhaust slots. |
| MEDIUM | `apps/ios/native/Sources/Remote/RemoteControlView.swift:178` | Refreshing the catalog invalidated an ongoing launch silently. Use an independent list sequence while retaining view-lifetime invalidation for both operations. |

Follow-up checks also corrected interactions introduced during repair: rejected
cleanup chains now recover, app deletion invalidates only that app's startup,
and atomic writes check cancellation immediately before rename. The HTTP host
test fixture now runs in Node's own realm, retaining the Electron stub without
the cross-VM native `execution_async_id` assertion seen in exploratory runs.

Final feature/peer/browser/atomic/store regressions: **126 passed, 0 failed or
skipped**. Existing Agent/authoring regressions: **56 passed, 0 failed or skipped**.
The final regression receipts are
`/private/tmp/lingxi-local-app-review-round2-final.log` (feature/peer/browser,
atomic-write and store tests),
`/private/tmp/lingxi-local-app-review-round2-agent.log` (56 existing Agent and
authoring tests), and `/private/tmp/lingxi-local-app-review-round2-build.log`
(Desktop production JS build). Node/renderer typechecks, Swift frontend parsing
and AST syntax checks of 45 changed TS/TSX files also passed. Native package,
device, actual public tunnel and real TURN acceptance remain open as documented
above; no matching Mobile binary generation was performed by this review.

Recommendation remains **COMMENT** pending those platform acceptance gates.
Within the reviewed source and covered regression scenarios, no confirmed
findings remain unaddressed. This does not claim exhaustive absence of defects.

## Third independent review

Reviewed the cumulative 85-file feature through `f76c75a60`, with particular
attention to interactions introduced by the previous two repair commits.
Requirements coverage was checked before the transport/Agent/lifecycle review.
The interrupted read-only pass was resumed against a clean checkout; three
GPT-6.1 Terra xhigh workers then reproduced and repaired the confirmed gateway,
broker and RTC findings while the parent handled host cleanup integration.

Nine further findings were confirmed and fixed: **two HIGH, six MEDIUM and one
LOW**. References below identify corrected source locations after this round.

| Severity | Location | Trigger, impact and correction |
| --- | --- | --- |
| MEDIUM | `apps/electron/src/main/local-apps/remoteGateway.ts:185` | Disconnecting during asynchronous authorization happened before the active-request close listener was registered. Repeated disconnects leaked slots and eventually caused persistent 429 responses. Register cleanup before authorization and check closed responses before counting or dispatch. |
| MEDIUM | `apps/electron/src/main/local-apps/remoteGateway.ts:300` | Simultaneous WebSocket upgrades all passed the size check before any was registered, bypassing the session connection limit; a disconnected pending client could also reach the backend. Reserve a browser connection slot before any await, count pending upgrades, release on every close/failure/revocation, and bound early data while observing EOF. |
| MEDIUM | `apps/electron/src/main/local-apps/remoteGateway.ts:252` | A disconnect during response reauthorization, a rejected recheck, or an unused HEAD response left a returned body without a consumer or cancellation owner. Explicitly cancel unconsumed bodies, transfer ownership to the Node stream once, and recheck already-fired abort/close state before piping. |
| HIGH | `apps/electron/src/main/local-apps/agentBroker.ts:674` | Stopping/revoking an application waited behind another application's ledger operation before aborting an active executor. Abort exact-app controllers synchronously before queued terminal persistence, matching session cancellation. |
| HIGH | `apps/electron/src/main/local-apps/agentBroker.ts:704` | A success waiting behind the ledger lock could commit ahead of a cancellation queued after it, even though the controller was already aborted. Recheck host state, controller and app epoch inside success settlement before publishing terminal events or assistant output. |
| MEDIUM | `apps/electron/src/main/local-apps/agentBroker.ts:620` | Chat reads bypassed the admission lock and exposed provisional user messages/state that could later roll back on revocation; attachment rehydration could race pruning. Serialize summary/list/history reads and asset rehydration through the same ledger queue. |
| MEDIUM | `apps/electron/src/main/local-apps/remoteRtc.ts:115` | RTC stripped the correct Content-Length from eligible HEAD reads, so page behavior differed from HTTPS fallback. Preserve the resource handler's valid HEAD length while continuing to omit GET lengths and inaccurate transformed metadata. |
| MEDIUM | `apps/electron/src/main/local-apps/host.ts:1034` | Last-session cleanup could see a background Agent still cancelling and retain its headless run indefinitely. Reconcile the matching run again after session cancellation has actually settled. |
| LOW | `apps/electron/src/main/local-apps/host.ts:910`, `apps/electron/src/main/local-apps/remoteRtc.ts:140` | The dedicated hidden RTC window delayed an otherwise idle non-macOS process's exit until the peer host's idle timer. Release that reusable window when no opening or browser access remains, before the existing process lifetime check. |

Deterministic pre-fix tests reproduced the broker lock/cancellation/result/read
races, both host cleanup failures, the HTTP disconnect leak, WebSocket capacity
bypass and the response reauthorization body leak. The RTC regression includes
framed transport and browser Response reconstruction. Cross-review found no
new dependency cycle between broker cancellation, per-app host cleanup and RTC
shutdown after these fixes.

Cancellation was calibrated at the operation's dispatch/commit boundary. The
pre-rename signal guard blocks a revoked operation still preparing its temporary
file; a valid filesystem operation already dispatched may complete. Replacing
the OS call with a delayed JavaScript stub does not demonstrate that already
dispatched side effects can be retroactively cancelled, so that speculative
finding was not treated as an additional defect.

Final verification:

- Feature/peer-host/real-Chrome/atomic/store regression suite: **138 passed**,
  zero failed, cancelled or skipped.
- Existing Agent broker/host/executor/SDK/assets and authoring regressions:
  **56 passed**, zero failed, cancelled or skipped.
- Desktop Node/renderer typechecks and production JS build passed.
- AST syntax traversal covered **45 changed TS/TSX files** without syntax
  errors, console.log calls or literal-secret assignments matching the reviewed
  patterns. Compiler diagnostics substitute for unavailable LSP tooling.
- `git diff --check` passed. No Mobile source, Rust wire format or Web source
  changed in this round; their previous receipts retain their stated scope.

Receipts: `/private/tmp/lingxi-local-app-review-round3-final.log`,
`/private/tmp/lingxi-local-app-review-round3-agent.log` and
`/private/tmp/lingxi-local-app-review-round3-build.log`.

Recommendation: **COMMENT**. No confirmed source findings remain unaddressed in
this review scope. Matching Mobile native builds, installed-device behavior,
public tunnel/TURN and long-duration acceptance remain pending; passing source
checks do not establish release readiness or exhaustive absence of defects.

## Fourth independent review

Reviewed the cumulative 85-file feature through `1f39809d5` and the final repair
diff. Stage 1 checked Desktop hosting, retained Mobile hosting and browser choice
before three independent GPT-6.1 Terra xhigh lanes examined the gateway,
Agent authority and client/lifecycle paths. The parent reproduced the HTML
encoding case and integrated verification. Five findings were confirmed and
fixed: **two HIGH and three MEDIUM**, with no CRITICAL or LOW findings.

| Severity | Corrected location | Trigger, impact and correction |
| --- | --- | --- |
| HIGH | `apps/electron/src/main/local-apps/agentBroker.ts:193`, `apps/electron/src/main/local-apps/host.ts:1031` | Disabling/changing Agent permissions cancels app access while preserving the browser session. A queued archive/delete or disk-blocked task admission checked only the still-live session signal and could commit after withdrawal. Capture an app-level operation signal before awaits, cancel it with app access, and combine it with the session signal through existing commit guards. Check the host Agent epoch across asynchronous authorization so stale requests cannot join a newly granted epoch. Regrant creates fresh operation authority. |
| HIGH | `apps/electron/src/main/local-apps/agentBroker.ts:774` | Broker shutdown left pending mutations alive and waited for the ledger lock before aborting active executors. Abort all app operation controllers and active executor controllers synchronously before shutdown awaits, then settle terminal records through the queue; reject new mutations once stopped. Wait for a ledger barrier and rollback before returning, including serialized chat creation with no active run. |
| MEDIUM | `apps/electron/src/main/local-apps/remoteGateway.ts:184` | The HTTP quota was counted only after asynchronous authorization, so 64 pending requests could all enter storage checks with active count zero. Reserve capacity after cookie/session identification and before any authorization await, with idempotent close/revocation release. The regression proves that only 32 checks enter and the other 32 requests receive 429. |
| MEDIUM | `apps/electron/src/main/local-apps/host.ts:993` | Unknown Content-Encoding values intentionally preserved by the service proxy reached HTML injection, which interpreted encoded bytes as UTF-8 and removed the encoding header. Reject unsupported full-HTML codings with 502 and cancel the body; identity HTML still receives the SDK, and partial response semantics stay intact. |
| MEDIUM | `apps/ios/native/Sources/Remote/RemoteControlView.swift:121`, `apps/ios/native/Sources/Remote/RemoteAppLaunchURL.swift:5` | The optional in-app browser belonged to the ready-only app catalog and disappeared on ordinary reconnect even though its gateway grant remained valid. Move browser presentation to the stable host view; invalidate pending launches independently, retain a presented ticket across catalog loss, and clear it on explicit dismissal. |

The authority regressions hold the ledger queue or a temporary file write
across app withdrawal/shutdown, check that no unauthorized archive/delete/task
record commits, and verify later regrant. An active executor is also checked
before an unrelated lock is released. Cross-review caught a shutdown-drain
regression during repair: synchronous abort must be followed by waiting for
queued rollback. Two regressions now prove shutdown waits for held admission
and chat-creation cleanup even with no active run. The HTML regression failed with a 200
corrupted representation before the fix and now returns 502. HTTP quota tests
use both controlled authorization gates and the actual loopback gateway.

Final verification:

- Feature/gateway/peer/real-Chrome/atomic/store regression suite:
  **148 passed**, zero failed, cancelled or skipped.
- Existing Agent/authoring regressions: **56 passed**, zero failed, cancelled
  or skipped.
- Desktop Node/renderer typechecks and production JS build passed.
- Swift frontend parsing passed for changed production/test files. A compiled
  standalone executable used the production browser-lifetime helper and URL
  validator to check reconnect invalidation, stale-launch rejection, explicit
  dismissal, reopening and expiry. These checks establish state behavior, not
  actual SFSafariViewController presentation.
- AST traversal of **45 changed TS/TSX files** found no syntax errors or matches
  for the reviewed console.log/literal-secret patterns; `git diff --check`
  passed. Unavailable LSP tooling is still substituted with compiler checks.

Receipts: `/private/tmp/lingxi-local-app-review-round4-final.log`,
`/private/tmp/lingxi-local-app-review-round4-agent.log`,
`/private/tmp/lingxi-local-app-review-round4-build.log`, and
`/private/tmp/lingxi-local-app-review-round4-swift.log`.

Recommendation: **COMMENT**. The confirmed findings in this scope are addressed;
matching Mobile native builds, actual SwiftUI/device behavior, public tunnel,
real TURN and long-duration acceptance remain unverified. This source review
does not establish release readiness or exhaustive absence of defects.

## Fifth independent review

Reviewed the cumulative **88-file** change against the original base, through
`ae08b0e00` and this repair. Stage 1 mapped Desktop hosting, retained Mobile
deployment, browser choice and scoped authority before three GPT-6.1 Terra
xhigh lanes reviewed gateway/RTC, Agent/run lifetime and cross-platform clients.
The parent cross-read the repairs and ran integrated verification. Seven new
findings were confirmed and fixed: **seven MEDIUM, zero CRITICAL/HIGH/LOW**.
No confirmed issue remains open in this review's source scope.

| Severity | Corrected location | Trigger, impact and correction |
| --- | --- | --- |
| MEDIUM | `apps/electron/src/main/local-apps/remoteGateway.ts:176` | Anonymous bootstrap bodies entered the same 128 MiB buffer budget as authenticated Agent/control requests before any Host/Origin check. Incomplete uploads could starve legitimate requests. Match a live ticket's origin route before buffering, use a separate 1 KiB body limit, cap pending exchanges at 32 and one per ticket, and abort on the 15-second deadline, disconnect or revocation. |
| MEDIUM | `apps/electron/src/main/local-apps/remoteGateway.ts:187`, `apps/electron/src/main/local-apps/remoteGateway.ts:198` | A client disconnect during bootstrap authorization still consumed its single-use ticket and created an unreachable eight-hour session, retaining its run and a session slot. Race authorization with disconnect/revocation/deadline and recheck the connection before granting; a disconnected exchange leaves the unexpired ticket reusable. |
| MEDIUM | `apps/electron/src/main/local-apps/host.ts:656`, `apps/electron/src/main/local-apps/agentBroker.ts:518` | A Desktop-origin Agent could retain an app after the last remote session ended, but its later settlement never retried cleanup. Add settlement subscriptions after active-run removal, reconcile only the matching remotely managed invocation, and recheck process lifetime. Listeners are isolated and not awaited, preventing a cancellation/cleanup deadlock. |
| MEDIUM | `apps/electron/src/main/local-apps/host.ts:902` | A remote opening used runtime.start for a stopped invocation retained by an authorized Desktop background Agent. The new invocation caused that task's next authority check to fail. Resume the retained same-revision invocation, matching native reopen behavior. |
| MEDIUM | `apps/ios/native/Sources/Remote/RemoteControlManager.swift:139`, `apps/ios/native/Sources/Remote/RemoteAppLaunchURL.swift:17` | Forgetting the current computer awaited FFI before invalidating app requests. A delayed open response could still launch a browser, and the stale ready state accepted new requests. Invalidate before the first suspension and block requests for that host until the connection state moves on; forgetting another host leaves current requests intact. |
| MEDIUM | `apps/electron/src/main/local-apps/remoteRtc.ts:107` | If cancellation/timeout won the resource race, a later Response never reached the local response variable and its body was not closed. Give the resource promise responsibility for cancelling late bodies after abort/peer closure; normal successful consumption remains unchanged. |
| MEDIUM | `packages/bridge-client/src/remote/transport.ts:120` | Empty continuation fragments retained chunk objects without increasing pendingBytes, bypassing the new bounded RTC input mode. Reject zero-payload continuations for explicitly bounded consumers; whole empty messages and ordinary fragmentation retain their semantics. The existing default transport mode is unchanged. |

Controlled production-handler reproductions established both bootstrap failures
before repair. Regressions now exercise anonymous body isolation, global and
per-ticket admission, disconnected authorization, blocked body revocation and
successful ticket retry over real loopback HTTP. The RTC regression covers a
late body after timeout, explicit request cancellation and session revocation,
plus normal consumption. Shared transport tests check empty whole messages,
rejected empty continuations and a normal fragmented message.

The two Desktop-task regressions use the real LocalAppRuntime and
LocalAppAgentBroker with a controlled gateway; both failed before repair. The
resume regression isolates the existing stopped-owner behavior in the same way
as the pre-existing native reopen test. It proves invocation preservation and
post-resume task success, not unrestricted execution while services are stopped.
Additional tests cover success/failure/cancellation settlement, a throwing or
reentrant subscriber, subscription replacement and obsolete invocation cleanup.

Final verification:

- Feature/gateway/peer/real-Chrome/atomic/store suite: **155 passed**, zero
  failed, cancelled or skipped.
- Existing Agent/authoring suite: **59 passed**, zero failed, cancelled or skipped.
- Complete bridge-client suite: **343 passed, five skipped** out of 348; zero
  failed or cancelled. Its build passed before exercising the changed transport.
- Desktop Node/renderer typechecks and production JS build passed.
- Swift frontend parsing passed. The production request-epoch/browser helper and
  final XCTest source passed simulator SDK typechecking (the test copy only
  removes its application-module import). A recompiled standalone async harness
  passed delayed forget, host isolation, reset and presented-browser retention.
  XCTest itself and the full FFI-backed application were not run.
- The simulator typecheck needs both the XCTest framework search path and
  `-I /Applications/Xcode.app/Contents/Developer/Platforms/iPhoneSimulator.platform/Developer/usr/lib`
  for its Swift overlay. An initial command without the latter failed; the
  corrected command completed with exit code zero.
- AST inspection covered **48 changed TS/TSX files**, with no syntax errors or
  console.log/literal-credential matches. Empty catches were reviewed in context;
  they isolate listeners, cleanup or explicitly handled fallback paths.
  `git diff --check` passed. LSP and ast-grep tools are unavailable; compiler and
  TypeScript AST checks provide the available diagnostics.

Receipts: `/private/tmp/lingxi-local-app-review-round5-feature.log`,
`/private/tmp/lingxi-local-app-review-round5-agent.log`,
`/private/tmp/lingxi-local-app-review-round5-bridge.log`,
`/private/tmp/lingxi-local-app-review-round5-build.log`,
`/private/tmp/lingxi-local-app-review-round5-swift.log`,
`/private/tmp/lingxi-local-app-review-round5-swift-typecheck.log`, and
`/private/tmp/lingxi-local-app-review-round5-ast.json`.

Recommendation: **COMMENT**. Confirmed findings are repaired. Full Mobile native
builds, physical browser/photo/file flows, public tunnel/TURN and long-duration
acceptance remain pending; source review and loopback checks do not establish
release readiness or exhaustive absence of defects.

## Integration with current main

Integrated the reviewed feature `cd4834d8bc30a3e79821bf1243419e81545eb7f3`
with main `d0ecc1937054bb16747e73bccf328806856e1685` in the isolated checkout
on `codex/local-app-remote-main-delivery-20261028` before updating main.

Main had moved Android/iOS Remote Control implementations into shared Mobile
FFI source. Both platform clients now delegate list_apps/open_app to that shared
core; its records, conversion and generation/connection checks live in
`packages/mobile-ffi/src/remote.rs`. A shared tunnel regression checks catalog
mapping, opening, invalid launch rejection and disconnected calls. The account
theme conflict retains main's existing supported surface token. Automatic merges
in the Electron main/preload files retain both features.

Before appending this record, blob comparisons verified that all 83 feature-only
paths and 128 main-only paths matched their respective parent versions. The five
overlapping paths were reviewed directly; the two additional shared FFI changes
are the intentional adaptation to main's refactor. Main's dependency pins and
Cargo.lock are preserved. The canonical main checkout was clean and unchanged
during isolated integration.

Integrated-tree verification passed:

- Feature/gateway/peer/real-Chrome/atomic/store: **155/155**.
- Existing Agent/authoring: **59/59**.
- Complete Web tests: **176/176**.
- Complete bridge-client tests: **345 passed, five skipped** out of 350.
- Desktop Node/renderer typechecks and production JS build.
- Android and iOS Rust FFI cargo check with the locked dependency graph.
- The new shared FFI tunnel regression passed in both platform crates: one
  Android test and one iOS test. Their existing unselected tests were filtered,
  not claimed as executed. Scoped rustfmt checks passed.
- Complete Rust Remote Control tests: **130 passed**. The initial sandboxed
  HTTP-fixture attempt could not bind loopback sockets; the authorized rerun
  completed successfully.

JS receipts: `/private/tmp/lingxi-local-app-main-feature.log`,
`/private/tmp/lingxi-local-app-main-agent.log`,
`/private/tmp/lingxi-local-app-main-web.log`,
`/private/tmp/lingxi-local-app-main-bridge.log`, and
`/private/tmp/lingxi-local-app-main-desktop-build.log`.
Preservation receipt: `/private/tmp/lingxi-local-app-main-merge-preservation.json`.
Rust receipts: `/private/tmp/lingxi-local-app-main-ffi-tests.log` and
`/private/tmp/lingxi-remote-main-merge-rust-client.log`.
Full native packages, physical devices and public deployment acceptance retain
the pending boundaries recorded above.

## Sixth independent review

This review used the isolated branch `codex/local-app-remote-review6-20261028`
at canonical main `8f193300b9577b3b4600a20d22214ca28c3c646d`. Stage 1 checked
that Desktop hosting, retained Mobile hosting and the user's browser-opening
choices remained intact before Stage 2 examined the transport, authorization,
concurrent startup, lifetime and cross-platform paths. Its coverage was the
89-file feature change plus runtime and runtime tests, Web remote-client and
reuse files and tests, and the store fixture: **95 files in the union**.

Ten MEDIUM and five LOW findings were fixed; there were no CRITICAL or HIGH
findings. Seven MEDIUM findings came from the independent review, two from
cross-checking the shared-start repair, and one was a pre-existing main Web
typecheck blocker. No confirmed finding remains open in this source scope.
Locations below refer to the corrected source.

| Severity | Corrected location | Trigger, impact, correction and evidence |
| --- | --- | --- |
| MEDIUM | `apps/electron/src/main/local-apps/remoteGateway.ts:199` | Redemption near the end of a ticket's lifetime started a new eight-hour timer and cookie lifetime, allowing an existing stream, WebSocket or RTC peer to outlive the original session deadline. Use the remaining time for both the abort timer and cookie Max-Age. The gateway regression holds a live stream until the shortened original deadline and checks the cookie lifetime; its cookie assertion failed before the fix. |
| MEDIUM | `apps/electron/src/main/local-apps/remoteGateway.ts:335` | A WebSocket request for `/__services/api?token=...` was matched against the raw URL, missed the service and was routed to the default backend or rejected. Split pathname and query and preserve the raw query when forwarding. The regression checks a root query, nested named-service path and default-service path; before the fix the root query reached the default backend. |
| MEDIUM | `apps/electron/scripts/local-app-remote-tunnel.mjs:35` | Every generated frpc configuration registered the same proxy name, so two Desktop-specific routes on one relay could conflict. Derive a stable proxy name from the route. The generator regression checks stable distinct names for two routes and failed against the original constant name. The [frp configuration reference](https://gofrp.org/en/docs/features/common/configure/) describes proxy-name uniqueness between clients. |
| MEDIUM | `apps/electron/src/main/local-apps/host.ts:1708`, `apps/electron/src/main/index.ts:127` | A remote opening could appear while the asynchronous background-app list was being read, after the idle check's first access check, allowing Desktop to quit despite the new opening. Recheck remote access after the list and immediately before quitting. A controlled host test inserts access while the list is blocked. |
| MEDIUM | `apps/electron/src/main/local-apps/runtime.ts:159` | Two owners could join one startup whose cancellation belonged to the first caller, so revoking that caller could cancel the other owner's launch. Track shared waiters and give the underlying operation a shared cancellation controller; stop it when the last waiter leaves. Tests cancel either waiter, cancel all waiters, resume a retained invocation and revoke one of two owners during real runtime startup. |
| MEDIUM | `apps/electron/src/main/remoteControl/peerHost.ts:197` | Shutdown or disposal between creating a hidden RTC window and its deferred open could leave a late window or peer. Recheck ownership before and after open, close a late resource and avoid closing a replacement. Peer tests cover shutdown and disposal before the deferred open and a late completion after a replacement starts. |
| MEDIUM | `apps/android/native/app/src/main/java/com/lingxi/code/remote/RemoteControlManager.kt:260` | Forgetting the current host invalidated existing requests but left a window during which new app requests could enter before FFI returned or the old connection departed. Block admission and result acceptance for that binding, with scoped handling for concurrent/failed forgets, account replacement and connection attempts. The Android test source exercises these cases; a separate JVM harness using the production method reproduced and checked the fix. |
| MEDIUM | `apps/electron/src/main/local-apps/runtime.ts:145` | In the first shared-start repair, a new or incompatible caller waiting for an abandoned startup's cleanup could not promptly observe its own cancellation. Before repair, a cancelled second caller remained unsettled until a blocked workspace preparation was released. Race the wait against that caller's signal without interrupting the original operation. Controlled runtime tests cancel each kind of waiter while cleanup or the other start remains blocked. |
| MEDIUM | `apps/electron/src/main/local-apps/runtime.ts:161` | The first shared-start repair could coalesce incompatible start or resume intents, dropping a requested replacement or revision check. Before repair, an incompatible request with expected revision 999 succeeded after a blocked ordinary start. Key sharing by intent, wait for incompatible work independently, and recheck the expected revision. A held-start test publishes a new revision, requests replacement and verifies that an invalid expected revision is rejected. |
| MEDIUM | `apps/web/src/remote/remoteClient.ts:371` | A pre-existing main event accepted an arbitrary permission-mode string where typed controls require a supported mode, blocking Web typechecking and potentially inserting an unsupported mode. Guard the event with the shared mode validator. The Web regression checks known and unknown events and a later event. |
| LOW | `apps/electron/test/local-app-agent-host.test.ts:325` | A captured `unknown` IPC request was indexed without narrowing. Narrow the array and compare its serialized message. |
| LOW | `apps/electron/test/local-app-remote-gateway.test.ts:275` | A forwarded header's `string | string[] | undefined` type was passed directly to a string assertion. Narrow it before matching. |
| LOW | `apps/electron/test/local-app-remote-rtc.test.ts:178` | An inferred union of header fixtures was incompatible with the response header type. Type the fixtures as string records. |
| LOW | `apps/electron/test/local-app-service-broker.test.ts:55` | The HTTP upgrade callback supplies a `Duplex`, while the test stored it as a narrower `Socket`. Use the declared callback type. |
| LOW | `apps/electron/test/fixtures/local-app-store.ts:8` | The runtime test store fixture lacked `bindAuthoring` and could drift from the production store contract. Implement the method and declare the fixture's contract. |

Final verification:

- Feature/gateway/peer/browser suite: **163 passed**, with no failures,
  cancellations or skips, after the final production runtime repair and before
  the test-fixture-only `bindAuthoring` addition. Agent/runtime suite: **83
  passed**; the runtime-only rerun after aligning the fixture passed **24**.
- Web suite: **177 passed**. Bridge suite: **345 passed, five skipped** out of
  350. Desktop Node and renderer typechecks and the unsigned Desktop JS build
  passed; Web typechecking passed after the permission-mode guard.
- The installed TypeScript language server published diagnostics for all
  **56 requested files**, with zero errors. Its configured projects use the
  parsed compiler options, paths and libraries, the effective `composite: false`
  setting and an ES2022 test project. An earlier default-project snapshot
  included stale generated output and inferred settings; the five real test
  typing errors it exposed were corrected before the final configured scan.
- A further independent read-only review confirmed both shared-start repair
  findings were closed and reported no additional grounded issue.

An initial TypeScript AST pass found no syntax errors in 52 changed files, and
the injected browser SDK source also parsed cleanly. Production code in scope
had no matches for the reviewed `console.log` or literal-credential patterns;
empty catches were examined in context. Android repository JUnit was not run:
generated bindings and JNI were unavailable. The JVM harness checked the exact
production forget method, and Swift 6 parsing passed for changed Swift files;
neither establishes a native application or physical-device pass.

The generated frpc configuration uses an outbound relay connection and the
Desktop loopback gateway, so a Desktop address change does not require a new
address registry. Reconnection is supported by the [frp v0.71.1 client
implementation](https://github.com/fatedier/frp/blob/v0.71.1/client/service.go#L265-L360);
the process supervisor is an external deployment responsibility. No frpc binary
or public relay was available for live tunnel validation.

Receipts are under `/private/tmp/lingxi-review6-*`, including the feature,
Agent/runtime, Web, bridge, Desktop typecheck/build, LSP and AST logs and the
Android harness source and artifacts. The last runtime-only rerun is
`/private/tmp/lingxi-review6-runtime-final.log`; LSP diagnostics are in
`/private/tmp/lingxi-review6-lsp-final.json`. The round changed 22 files. This
work remains uncommitted and unpushed in the isolated review branch; canonical
main remains unchanged and clean at the recorded commit. Recommendation:
**COMMENT**, bounded by the outstanding native package, physical browser/device
and public deployment acceptance described above.

## Seventh independent review

Stage 1 checked the Desktop-authoritative runtime, per-opening browser origins,
scoped withdrawal, HTTP/WebSocket/SSE with optional RTC fallback, and the
outbound reconnecting tunnel contract. Stage 2 continued the cumulative
**95-file union** of feature changes, fixtures, runtime and Web context,
with fresh inspection focused on this round's eight changed runtime, native
and test files and their callers. Five new
findings were confirmed and repaired: one HIGH and four MEDIUM. No CRITICAL or
unresolved HIGH finding remains in this reviewed source scope.

| Severity | Corrected location | Trigger, impact and evidence |
| --- | --- | --- |
| HIGH | `apps/electron/src/main/local-apps/runtime.ts:160`, `:532` | An incompatible start or resume waiting outside the runtime queue survived Stop and could start again after the stopped operation settled. In a real runtime repro, start A was held, replacement B waited, and Stop was followed by B reaching `running`. Register waiting operations and abort them and shared controllers on Stop, removal and disposal; also recheck cancellation in the existing-run fast path. Runtime regressions cover incompatible start/resume, lifecycle cancellation and a fresh start after Stop. |
| MEDIUM | `apps/electron/src/main/local-apps/runtime.ts:193`, `apps/electron/src/main/local-apps/host.ts:906` | Revoking the last remote opener just after activation committed made the shared wrapper hide that successful run from host ownership, leaving it running without a session. Preserve the committed operation's success for the final cancelled waiter; the host records its run and then checks its own cancellation. The real-runtime host regression failed before the fix with `running`, no session and no cleanup owner, and passes afterward. Earlier cancellation of one shared waiter remains isolated from another owner. |
| MEDIUM | `apps/android/native/app/src/main/java/com/lingxi/code/remote/RemoteControlManager.kt:261` | A delayed forget could complete after an account change and modify the replacement account's hosts list. Capture the originating account and client, and apply completion only if both still match. The faithful extracted Kotlin harness reproduces the old cross-account effect and verifies isolation after the fix. |
| MEDIUM | `apps/android/native/app/src/main/java/com/lingxi/code/remote/RemoteControlManager.kt:268`, `apps/ios/native/Sources/Remote/RemoteAppLaunchURL.swift:7` | Departure could clear a forget barrier while its unpair operation was still suspended; a reconnect could then admit app requests before forget completed. Track pending operations and departure per barrier, preserving identity across account reset and concurrent forgets. The pre-fix Android and Swift harnesses reproduce premature admission; the corrected harnesses check pending forget, reconnect and recovery. |
| MEDIUM | `apps/android/native/app/src/main/java/com/lingxi/code/remote/RemoteControlManager.kt:380` | App request admission sampled connection state and epoch separately, allowing a replacement attempt between those reads to be treated as the originating attempt. Snapshot and validate them under the same lock used by connection updates, then recheck account, client, epoch and attempt on completion. A controlled old/new Kotlin harness exercises the interleaving. |

Fresh verification on this review tree:

- The feature/gateway/peer/real-Chrome suite passed **164/164** using the
  bundled Electron Node runtime. Standalone Node 24 Agent/runtime tests passed
  **90/90**. Desktop Node and renderer typechecks and production JS build passed.
- The installed TypeScript language server returned diagnostics for **56/56**
  requested files with zero errors. The TypeScript AST scan found zero syntax
  errors in 56 files, no reviewed `console.log` or literal-credential matches,
  and 26 empty catches were reviewed in context. The dedicated LSP and
  ast-grep MCP tools were unavailable; these checks used the installed language
  server over LSP and the TypeScript AST directly.
- A regenerated harness using the exact Android production method compiled and
  passed pending-forget, reconnect, recovery and account-isolation checks; its
  compilation reported two unnecessary safe-call warnings. The standalone
  Swift helper passed, all three changed Swift files parsed, and the helper plus
  a copy of the XCTest source typechecked together with the simulator SDK and
  XCTest overlays. Only the application-module import was removed in that copy.
  Neither Android repository JUnit nor XCTest was executed; no full native
  application build or physical-device acceptance is claimed.
- Prior round 6 evidence remains **177/177** Web tests and **345 passed, five
  skipped** bridge tests; those suites were not rerun in this round. A standalone
  Node host-suite attempt passed its individual assertions but hit a native
  `async_id` assertion during teardown even when the new regression was skipped;
  it is not counted as a passing suite. The fresh bundled-runtime suite above
  completed normally.

Receipts: `/private/tmp/lingxi-review7-feature.log`,
`/private/tmp/lingxi-review7-agent-runtime.log`,
`/private/tmp/lingxi-review7-desktop-typecheck.log`,
`/private/tmp/lingxi-review7-desktop-build.log`,
`/private/tmp/lingxi-review7-lsp-final.json`,
`/private/tmp/lingxi-review7-ast.json`,
`/private/tmp/lingxi-review7-host-before.log`,
`/private/tmp/lingxi-review7-host-targeted.log`, and
`/private/tmp/lingxi-review7-swift-typecheck.log`. Android and Swift harness
sources and binaries are under `/private/tmp/lingxi-review7-*`.

The round directly changed eight runtime, test and native files; the cumulative
uncommitted review tree contains 25 modified files, including the three iOS
files added in this round. Canonical main was rechecked clean at
`8f193300b9577b3b4600a20d22214ca28c3c646d`. Recommendation: **COMMENT**;
native packages, physical browser/device flows and public relay/TURN acceptance
remain separate outstanding release checks. No commit or push was made.
