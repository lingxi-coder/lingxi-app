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
