# LingXi product applications

Product entrypoints share one pinned Harness runtime. Electron and Web use the
Rust Bridge service; iOS and Android use their platform-local Rust FFI wrappers.
Each platform directory owns its interface and build entrypoints.

| Directory | Responsibility |
|---|---|
| [`electron/`](electron/README.md) | Electron desktop interface, native helpers and signed macOS packaging. |
| [`ios/native/`](ios/native/README.md), [`ios/ffi/`](ios/ffi/) | SwiftUI/Xcode application and the `ios-framework` Rust wrapper. |
| [`android/native/`](android/native/README.md), [`android/ffi/`](android/ffi/) | Kotlin/Gradle application and the `android-aar` Rust wrapper. |
| [`cli/host/`](cli/host/), [`cli/tui/`](cli/tui/), [`cli/tui-core/`](cli/tui-core/) | CLI entrypoint and its terminal interface. |
| [`bridge-server/`](bridge-server/) | Rust service consumed by the desktop and Web clients. |
| [`web/`](web/) | Web interface. |
| [`../packages/bridge-client/`](../packages/bridge-client/) | Private `@lingxi/bridge-client` package: protocol types and Node `BridgeClient`. |
| [`../resources/voice/`](../resources/voice/README.md) | Shared voice model and audio configuration sources and generators. |
| [`../resources/translations/`](../resources/translations/README.md) | Canonical translations and generators for native localized resources. |

The repository root remains the Cargo workspace. Select Rust packages with
`cargo -p`; platform UI builds continue to use npm, Gradle or Xcode. Use npm
and the checked-in `package-lock.json` in each JavaScript package.

## One-command setup

Run from the repository root:

```sh
./apps/setup.sh
```

The bootstrap builds `bridge-server`, installs and builds `packages/bridge-client`,
then installs Electron dependencies. Android and iOS steps run when their optional
toolchains are available; missing mobile toolchains skip those steps with guidance.
The required core is Cargo and Node/npm. Re-run the script after installing a
missing toolchain or changing dependencies.

The shared package must be built before Electron: Electron resolves
`@lingxi/bridge-client` through `file:../../packages/bridge-client` and consumes
its compiled `dist/` exports. The bootstrap never commits or embeds provider keys.

## Manual setup

Run from the repository root:

```sh
cargo build --locked -p bridge-server --bin bridge-server
npm --prefix packages/bridge-client install
npm --prefix packages/bridge-client run build
npm --prefix apps/electron install

# Android: cargo-ndk and an Android NDK
bash apps/android/native/scripts/build-mobile-linux-native.sh --variant play

# iOS: macOS, Xcode and XcodeGen
(cd apps/ios/native && bash scripts/build-xcframework.sh && xcodegen generate)
```

For macOS, use the signed packaging wrapper from `apps/electron`:

```sh
cd apps/electron
npm run package:mac:flare -- --check
npm run package:mac:flare -- --launch
```

Signing requirements, provisioning and required package verification are governed
by [`../AGENTS.md`](../AGENTS.md). `npm run dev` supports interface development;
credential persistence and real macOS sessions require the signed application.

Open `apps/android/native` in Android Studio or
`apps/ios/native/LingxiCode.xcodeproj` in Xcode after generating the project.
Changes to Rust FFI signatures require regenerating the platform bindings before
building the native consumers. Pure interface changes can reuse existing bindings.

## Further reading

- [Bridge conversation and transport checks](README-bridge.md)
- [Electron development and packaging](electron/README.md)
- [Android development](android/native/README.md)
- [iOS development](ios/native/README.md) and [engine integration](ios/native/README-engine.md)
- [Translations](../resources/translations/README.md)
- [Multi-repository development and SDK independence](../docs/development/multi-repo-workflow.md)
