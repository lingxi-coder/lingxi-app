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
