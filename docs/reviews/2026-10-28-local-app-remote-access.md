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
LOW findings. The references below point to the corrected source locations;
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
