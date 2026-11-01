# iOS in-process engine

The app calls the shared engine through generated UniFFI Swift bindings. `MobileEngineHandle.submit` delivers `ClientEvent` values through `IosEventListener`; account management uses `providerAccountsSnapshot` and `providerAccountCommand` on the same handle.

## Build matching bindings and native libraries

Use macOS with Xcode 27+, XcodeGen, the repository Rust toolchain and its iOS targets. Supply the official pinned GStreamer SDK through `LINGXI_GSTREAMER_SDK_IOS`; see [the SDK instructions](../../../scripts/ci/GSTREAMER_SDK.md). The build also verifies the Cargo-locked Mobile Linux native support and rootfs.

From `apps/ios/native`:

```sh
scripts/build-xcframework.sh
scripts/install-sherpa-runtime.sh
xcodegen generate
```

`Generated/` and `Frameworks/LingxiCodeFFI.xcframework` are ignored build outputs. Regenerate both when the Rust FFI or shared protocol changes. The default build contains ARM64 device and ARM64/x86_64 simulator slices. `LINGXI_ARM64_DEVICE_SIM_ONLY=1` builds both Apple Silicon execution targets for local verification; `LINGXI_SIM_ARM64_ONLY=1` produces a simulator-only artifact unsuitable for device delivery.

The script combines generated namespace module maps, deduplicates shared Swift scaffolding and initializes callback registration. These transformations preserve the generated ABI. To stage a complete pair before replacing the app inputs, set `LINGXI_GENERATED_DIR` and `LINGXI_FRAMEWORKS_DIR` to isolated output directories.

## Configure accounts

Configure Provider definitions and enabled model bindings in Settings, then add independent API key or supported OAuth accounts under that Provider. iOS OAuth uses a device code: open the verification page, approve the code and let the account-bound Host flow finish. Credentials go through the native secure-storage interface; the app does not obtain LLM credentials from environment variables.

The Host owns account priority, availability and model permissions. Swift displays the metadata-only `modelAvailability` projection. Discovery marks access unverified; successful model use confirms access. A model-specific denial affects that account/model pair. It does not authorize metered API billing for an OAuth account.

To validate a real model call, add an account through Settings and send a prompt using an eligible model. Never embed credentials in source, build arguments or test logs.

## Run native tests

Select an isolated simulator and run the hosted tests:

```sh
TEST_RUNNER_LINGXI_UI_TESTING=1 xcodebuild -scheme LingxiCodeStore -sdk iphonesimulator \
  -destination 'platform=iOS Simulator,id=<isolated-simulator-uuid>' \
  -only-testing:LingxiCodeTests/ProviderAccountRepositoryTests \
  -only-testing:LingxiCodeTests/SecureStorageReplacementTests \
  -only-testing:LingxiCodeTests/IOSCloudAudioTests \
  test CODE_SIGNING_ALLOWED=NO
```

The host UI flag prevents its default conversation from initializing an unrelated Linux runtime before a unit fixture; tests still call the real FFI. The SDK permits one immutable kernel/rootfs configuration per app process. Run each filesystem-dependent positive method (`NativeSettingsEngineRoundtripTests/testKeylessLayerWritesReadBackAndSurviveEngineRebuild` and `NativeAdminEngineRoundtripTests/testKeylessAdminCatalogsAndCorrelatedHookValidationReturnThroughNativeCallbacks`) in a separate fresh `test-without-building` invocation with that flag. Keep every positive assertion; an unavailable simulator kernel does not prevent verified host-path translation.

`ProviderAccountsNativeUITests` launches the actual app with the host UI flag explicitly disabled and checks synthetic account creation, disabling, persistence after restart and deletion. It never invokes the account-test or prompt-send actions. Run this UI case with normal simulator signing (`CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=-`); its real system Keychain requires the application entitlement packaging that unsigned source-only tests omit.

`ProviderAccountRepositoryTests` covers device-code flow, priority, model projections, stale responses and read-only recovery. `SecureStorageReplacementTests` covers isolated secure-storage replacement and real FFI account persistence/model projection with synthetic credentials. `NativeSettingsEngineRoundtripTests` checks empty-account startup and settings persistence. `EngineRoundtripTests` checks terminal events from an unconfigured model turn; it does not establish successful upstream authentication or inference.

Native test results require freshly matched bindings and libraries. Isolated Swift source tests do not establish native ABI or device acceptance. Real OAuth approval and physical-device acceptance are separate checks.
