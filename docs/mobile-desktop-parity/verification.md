# Native mobile parity verification

Baseline: Desktop `e0d900b8fb3033be23718a22694db92a9ee561f5`.
Android uses Compose and iOS uses SwiftUI. No chat WebView or new third-party
packages were introduced. Existing credentials, projects and session records are
preserved.

## Final results

| Surface | Result |
|---|---|
| iOS Store unit suite | 712 passed, zero failures |
| iOS Full unit suite | 712 passed, zero failures |
| iOS phone/tablet UI across both configurations | 16 applicable checks passed; opposite-device-only cases intentionally skipped |
| Android Direct JVM suite | 841 passed, zero failures |
| Android Play JVM suite | 830 passed, zero failures |
| Android real native engine checks | 4 Direct + 4 Play passed |
| Android final UI | 66 passed: Direct26, Play14, large text/reduced motion13, tablet13 |
| Android lint | Direct/Play passed with zero errors/fatal findings; existing warnings documented |
| Shared configuration administration | 220 Rust tests passed |
| Mobile engine integration | 24 Rust tests passed; production configuration check passed |
| MCP stale-generation regression | Deterministic failure reproduced before fix; both registry guard tests pass afterward |
| Desktop shared transcript corpus | 8 scenarios passed through production transcript projection |
| CLI / Desktop extraction compatibility | Library checks and 88-crate dependency gate passed; shared Clippy completed with existing warnings |

Both iOS configurations and both Android flavors exercise the real generated
ABI: model/turn callbacks, user/project/local settings writes, snapshot and disk
readback, rebuilding the engine with persisted settings, typed MCP/Skills/Plugins/
Hooks catalogs, and correlated terminal Hook validation. These use isolated
keyless sandboxes. Native Keychain and Android Keystore have dedicated secret
round-trip tests.

## Changes and simplifications

- Android entry points: `RootScreen.kt`, `AdaptiveConversationDrawer.kt`, and
  `settings/SettingsHost.kt`; iOS: `App/RootView.swift` and
  `Settings/SettingsHost.swift`.
- Native conversation projections own tool grouping, last-tool summaries,
  historical/active state, session-keyed disclosure restoration and native
  Markdown/details. Shared artwork contains 28 light/dark agent variants.
- Native settings use four searchable groups, layered drafts, read-only locks,
  admin commands, provider import, secure credentials and guarded application of
  saved changes. Replaced legacy settings views and dead placeholders were
  removed; compatibility routes remain.
- `configuration-admin` reuses the existing Desktop storage/admin/plugin
  lifecycle implementation, with CLI/bridge compatibility exports. Mobile
  command dispatch now supplies real replies through the connection event sink.
- MCP moves legacy entries once to its own app-private storage domain, retaining
  the original file. An atomic migration marker prevents deleted servers from
  reappearing.

## Capability differences and verification limits

Fusion is explicitly unsupported by the current mobile engine. Its navigation
entry is omitted and legacy routes explain the capability limitation. A writable
editor with no runtime effect was removed.

Authenticated provider-network completion was not exercised; runtime tests are
intentionally keyless. Phone/tablet verification uses simulators/emulators,
including a real software keyboard, large text, reduced motion and native accessibility semantics. Android runtime checks ran on arm64; x86_64 native libraries were built and checksummed. Existing
Swift concurrency/deprecation and Rust warning output is retained in logs. Android lint reports 1,123 Direct and 1,115 Play warnings; no error suppression baseline was added.

## Evidence

- `evidence/native-artifacts/`: final Android/iOS manifests, 35 promoted artifact
  checksums and 97 input-source fingerprints.
- `.omx/state/ios-final-validation/report.md`: configuration-by-configuration
  test results and screenshot locations.
- `.omx/state/mobile-desktop-parity/android-final/`: Android unit/native/UI logs,
  screenshots and native artifact readback.
- Shared test corpus: `clients/shared/fixtures/native-conversation-parity.json`;
  identity vectors: `clients/shared/fixtures/agent-avatar-identities.json`.

Signed iOS simulator tests use `CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=-` so
Keychain entitlements are valid. Android instrumentation uses an isolated
read-only emulator; physical-device user data is unchanged.

Final localization follow-up rebuilt both iOS bundles, verified compiled translations, and passed an additional settings light/dark UI smoke. Canonical generation check passes for 2,255 keys across five locales. Final source fingerprint recheck found no changes in all 97 native inputs.
