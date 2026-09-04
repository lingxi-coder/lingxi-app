# LingXi repository instructions

## macOS Electron packaging

- Always package the signed macOS Desktop app through `npm run package:mac:flare` from `clients/electron`, or invoke `clients/electron/scripts/package-macos-flare.sh` directly. Do not bypass this wrapper with `npm run package:mac` during normal development or release preparation.
- Use the same Apple Developer team as iOS: Flare App, Inc., Team ID `AZ4AX7J833`. The wrapper rejects a different Team ID and selects the currently valid `Apple Development: lingfeng luo (KQ7KX8LCYL)` identity by certificate hash.
- Development packaging uses the isolated `development` credential channel and requires Mac App Development profiles authorizing `com.lingxi.code.development` and `com.lingxi.code.credential-broker.development`. An iOS Xcode Managed Profile is not a substitute for a macOS profile.
- When either profile is missing, the wrapper must use the checked-in provisioning bootstrap target with Xcode Automatic Signing and `-allowProvisioningUpdates -allowProvisioningDeviceRegistration`. This reuses the Flare account already configured in Xcode; do not automate the Apple Developer website with a browser.
- Explicit profile paths may be supplied with `LINGXI_MAC_PROVISIONING_PROFILE` and `LINGXI_MAC_BROKER_PROVISIONING_PROFILE`. Use `--no-register` only for CI or a deliberately read-only preflight. Never commit Apple IDs, app-specific passwords, private keys, App Store Connect API keys, or local profile files.
- Run `npm run package:mac:flare -- --check` to create/download missing profiles and validate signing assets without building the real app. Add `-- --launch` to launch the packaged app after all verification passes.
- Do not use `npm run dev` to test macOS credential persistence. The Credential Broker intentionally rejects unpackaged Electron processes.
- The wrapper must retain the release Rust path remapping, inside-out signing performed by `package-mac.mjs`, and `verify:package` static/runtime verification. A successful compilation without those checks is not a completed macOS package.
