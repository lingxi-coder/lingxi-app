# Deeplink main integration

The delivery integrates the deeplink feature with product main `fb9da646cb7dd028375d29ba9307415ed56e14d0`. It pins Harness `97d33cafb24bd459fb2e13116602e19fb52bfe9e`, based on upstream main `ea42d697703b29fb359851e80046cdd3abb5d02a`, retaining the upstream provider-startup fix and newer protocol. The original dirty SDK checkout is untouched.

## Preserved UI correction

Desktop transcript rows have no **Copy message link** button. Their UUID attributes, target expansion, scroll and highlight remain; normal Copy message remains. The interaction test asserts the link button is absent and no UUID was copied, while retaining all locator assertions. Its fixture supplies the current protocol's derived tool header/display instead of relying on an obsolete missing-header shape.

The corresponding signed-app verification recorded an actual `MODEL_ROUTE_OK` assistant reply with zero message-link buttons. That package passed the standard Flare wrapper, static verification and packaged smoke; ZIP SHA-256: `33c47ce9fe001d7e60791010b10a181381a4891bb2fdf0f98577ff8d7dcc05f6`. Local evidence is `/Users/luolingfeng/lingxi/.buildcache/message-link-button-rebuild/verification.json`. This package predates the final main integration; it is not a binary built from the newer pinned SDK.

## Integration checks

Logs are under `/Users/luolingfeng/lingxi/.buildcache/deeplink-main-delivery/`.

- Shared contract: TypeScript 4 and Rust 4 tests passed.
- Web: 39 tests, typecheck, production build and Direct association preflight passed. The public certificate is committed at `docs/deep-links/android-direct-release-cert.pem`; private signing files remain ignored.
- Electron: 155 focused tests, Node/renderer typechecks and production build passed. The real Electron preview interaction also passed after applying the no-button correction to the integrated source.
- SDK: 186 client tests, three mobile client-token tests and the real append-once JSONL UUID emission regression passed. The mobile tests cover slash/skill image retention through cold restoration as well as explicit turn identity.
- Cargo metadata and the source-identity gate passed: 77 Harness packages use the same pinned revision; five Mobile Linux packages retain their existing revision.
- Final product Rust compilation passed in 8m 37s: `cargo check -p bridge-server -p cli -p tui -p ios-framework -p android-aar --tests --features ios-framework/cli --locked --offline`. This checks the test targets and both FFI crates; it does not execute their tests or rebuild signed native packages.

Earlier native libraries, signed packages and OS acceptance results remain identified by their actual `98130`/`feadd` revisions in [implementation-status.md](implementation-status.md). They are historical artifact evidence, not fresh native-device acceptance for this newer integrated SDK. Physical-device lifecycle/restart UUID, signed iOS Universal Links, Play App Signing association and native Windows/Linux acceptance remain the explicit external release gates.

## Worktree preservation

Only the deeplink scope and required current-main adaptations are committed. Unrelated audio-probe and model-route changes already published upstream are retained; the original iOS provider-settings/localization/UI-test edits and differing audio comments remain uncommitted. The preexisting CLI fixture fields are already present in upstream main and remain in the delivered source. Original files, staged state and preservation manifests were saved outside the source tree before integration.
