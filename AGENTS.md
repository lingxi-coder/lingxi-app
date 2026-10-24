# LingXi repository instructions

## Development checkout

- Use `~/lingxi/lingxi-app` as the canonical product checkout and start repository commands from this real directory.
- `/Users/luolingfeng/Projects/LingXi-Next` is a compatibility symlink to the same checkout; changes and Git configuration are already shared between these paths.
- The product `origin` is `git@github.com-lingxi-coder:lingxi-coder/lingxi-app.git`. Keep the repository-owner SSH alias for fetch and push.

## macOS Electron packaging

- Always package the signed macOS Desktop app through `npm run package:mac:flare` from `apps/electron`, or invoke `apps/electron/scripts/package-macos-flare.sh` directly. Do not bypass this wrapper with `npm run package:mac` during normal development or release preparation.
- Use the same Apple Developer team as iOS: Flare App, Inc., Team ID `AZ4AX7J833`. The wrapper rejects a different Team ID and selects the currently valid `Apple Development: lingfeng luo (KQ7KX8LCYL)` identity by certificate hash.
- Development packaging uses the isolated `development` credential channel and requires Mac App Development profiles authorizing `com.lingxi.code.development` and `com.lingxi.code.credential-broker.development`. An iOS Xcode Managed Profile is not a substitute for a macOS profile.
- When either profile is missing, the wrapper must use the checked-in provisioning bootstrap target with Xcode Automatic Signing and `-allowProvisioningUpdates -allowProvisioningDeviceRegistration`. This reuses the Flare account already configured in Xcode; do not automate the Apple Developer website with a browser.
- Explicit profile paths may be supplied with `LINGXI_MAC_PROVISIONING_PROFILE` and `LINGXI_MAC_BROKER_PROVISIONING_PROFILE`. Use `--no-register` only for CI or a deliberately read-only preflight. Never commit Apple IDs, app-specific passwords, private keys, App Store Connect API keys, or local profile files.
- Run `npm run package:mac:flare -- --check` to create/download missing profiles and validate signing assets without building the real app. Add `-- --launch` to launch the packaged app after all verification passes.
- Do not use `npm run dev` to test macOS credential persistence. The Credential Broker intentionally rejects unpackaged Electron processes.
- The wrapper must retain the release Rust path remapping, inside-out signing performed by `package-mac.mjs`, and `verify:package` static/runtime verification. A successful compilation without those checks is not a completed macOS package.

## Mobile Local App presentation (user decision, 2026-10-24)

- The first Android and iOS Local App release opens application pages in the external system browser, such as Chrome or Safari. Do not use embedded WebView/WKWebView as the Mobile product entry point. Desktop keeps its existing application windows.
- LingXi owns runtime services, Agent execution, model credentials and trusted native authorization/tool approval. Browser-to-host access must be authenticated and scoped to the installed app and current run. Do not expose host administration, credentials or approval APIs to pages.
- Use browser camera/file selection. Validate external-browser navigation, photo return, host background expiry/recovery, Agent requests and cancellation on real Android and iPhone devices. Historical WebView results are not external-browser acceptance.
- This product architecture requirement does not guarantee store approval. Do not switch back to WebView without an explicit user decision.
