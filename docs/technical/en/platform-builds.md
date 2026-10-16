# Native builds, bindings and release acceptance

Build matched Rust/native artifacts, select distribution variants and separate compile, simulator and physical-device acceptance.

<a id="build-matrix"></a>

## Platform and distribution matrix

Public product capabilities are selected by host, target OS, native callback readiness and distribution flavor. Product version fields are artifact-specific; iOS marketing/build versions, Android versionName/versionCode, Electron package version and Rust crate versions must not be presented as one universal SDK revision. Source baselines and product dependency pins provide the reproducible identity.

| Target | Current build contract | Acceptance boundary |
| --- | --- | --- |
| Android product | minSdk 26; compileSdk 37; targetSdk 35; arm64-v8a and x86_64. | JNI packaging and instrumented device behavior. |
| Android play / direct | play is store with high-risk native offloads disabled; direct is full with separate application id. | Compile-time flavor selection plus runtime permission decisions. |
| iOS product | iOS 18.0; device arm64; arm64/x86_64 simulator; Store/Full configurations. | Signed-device guest execution; simulator only API/link checks. |
| Desktop macOS | Electron plus Rust sidecar and signed credential broker through package:mac:flare. | Static package verification and packaged runtime smoke. |
| CLI / Web | Rust binary / static React-Vite site. | Terminal/provider behavior / browser documentation behavior. |

<a id="binding-artifacts"></a>

## Generate one matching binding/library set

Android build-jni.sh builds android-aar for both supported ABIs, generates each UniFFI component using its configuration and stages owned Kotlin packages and libandroid_aar.so before replacement. It preserves handwritten Java and other native helpers. play/direct select different JNI outputs. Do not copy only a new binding or only a new library into a live Gradle build.

iOS build-xcframework.sh generates Swift/C headers/modulemaps, builds device and simulator Rust slices and assembles LingxiCodeFFI.xcframework. MobileLinuxNativeSupport contains iSH glue without another Rust core; static frameworks link into the app and are not embedded dylibs. Optional isolated output roots stage matching frameworks and bindings before promotion.

LINGXI_SIM_ARM64_ONLY=1 makes a simulator-only iOS framework for local tests and is not shippable. LINGXI_DEVICE_ONLY=1 selects the device path. Reusing generated bindings or staged native runtime is valid only when the build scripts validate matching sources and artifacts; a convenience flag is not a release provenance check.

### Product native build entry points

```bash
bash apps/android/native/scripts/build-jni.sh --variant play
bash apps/android/native/scripts/build-jni.sh --variant direct
bash apps/ios/native/scripts/build-xcframework.sh
(cd apps/ios/native && xcodegen generate)
```

Verification method: Environment validation

<a id="release-flow"></a>

## Release assembly and resource identity

Use the product locked SDK revision together with its native-support release, archive SHA-256, manifest, SBOM and platform resource set. Android packages extracted executable helper libraries; iOS uses converted fakefs ZIP, RootfsPatch.bundle, VDSO and stable workspace mounts. Changing a source pin requires refreshing all dependent native artifacts and verifying package inventory, not only editing a manifest.

Android store/full flags describe compiled capabilities and still require runtime permissions. iOS XcodeGen preserves target, distribution and signing settings from project.yml. macOS packaging must keep the configured team and correct development application/broker profiles; the flare wrapper performs preflight and verified signing. Credentials are runtime inputs and must not be compiled into framework or APK resources.

1. Select the source/dependency revisions and distribution configuration.
2. Generate matched bindings, Rust libraries and native support; verify resource hashes and ABI inventory.
3. Build and sign the final product using the platform entry points.
4. Run static package, simulator/host and physical-device checks at their separate acceptance levels.

<a id="release-checks"></a>

## Testing levels and common failures

Host Rust compilation verifies facade features and FFI type shapes; Swift/Kotlin compilation verifies generated converters; simulator tests verify UI and linking; device smoke verifies actual native execution. Archive checks establish resource trust before execution, and packaged smoke establishes the launched sidecar/service identity. Record which exact level passed rather than expanding a green build into hardware acceptance.

Undefined FFI symbols indicate a binding/library/component mismatch; missing Android helper or wrong ELF ABI is packaging; unavailable simulator guest is expected; RestartRequired is iOS process identity; profile rejection is signing; source-check success with no matching release provenance is not a distributable artifact. Device network/memory, OS permission, suspension/resume and audio-owner cleanup remain explicit acceptance items.

- [Native callback facade](platform-integration.md)
- [Standalone Android SDK](mobile-android.md)
- [Standalone iOS SDK](mobile-ios.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app/tree/main/apps/ios/native/scripts/build-xcframework.sh)

- [lingxi-app/apps/ios/native/scripts/build-xcframework.sh](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/apps/ios/native/scripts/build-xcframework.sh) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [lingxi-app/apps/ios/native/scripts/build-linux-runtime.sh](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/apps/ios/native/scripts/build-linux-runtime.sh) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [lingxi-app/apps/ios/native/scripts/verify-linux-runtime.sh](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/apps/ios/native/scripts/verify-linux-runtime.sh) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [lingxi-app/apps/ios/native/project.yml](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/apps/ios/native/project.yml) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [lingxi-app/apps/android/native/scripts/build-jni.sh](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/apps/android/native/scripts/build-jni.sh) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [lingxi-app/apps/android/native/app/build.gradle.kts](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/apps/android/native/app/build.gradle.kts) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [lingxi-app/apps/android/ffi/uniffi.toml](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/apps/android/ffi/uniffi.toml) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [lingxi-app/apps/ios/ffi/uniffi.toml](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/apps/ios/ffi/uniffi.toml) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [lingxi-app/apps/electron/scripts/package-macos-flare.sh](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/apps/electron/scripts/package-macos-flare.sh) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [mobile-linux-runtime/scripts/checks/check-resource-contracts.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/scripts/checks/check-resource-contracts.sh) — Committed source (b65f8856d773d27ebf73b0cade907884d3080704)

- lingxi-app: HEAD cc3f725e248e4d717ceecec4c029a86a318dec3e; Referenced input fingerprint sha256:af041726ea2dcbfff18678637efb08857c8817c356f2a3bf2e0ddd2bd1205b1e
- mobile-linux-runtime: HEAD b65f8856d773d27ebf73b0cade907884d3080704; Referenced input fingerprint sha256:de6f69b8671150c8682b938faea726c0360978b776acaa4c077b257b13792174; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
