# Mobile Linux Runtime

Embed a Linux userspace in Android and iOS apps, with commands, streaming, PTY, raw stdio and verified rootfs lifecycle.

<a id="platforms"></a>

## Supported platforms

Mobile Linux Runtime is a standalone SDK. The application supplies paths, verified rootfs assets, mount and network policy, and lifecycle. Rust contracts and platform backends do not depend on Harness, an Agent, an LLM, or application preferences.

- Android: PRoot backend; arm64-v8a and x86_64; application minimum API 26. Optional legacy host-shell helpers require API 29.
- iOS: iSH arm64 backend on physical devices; minimum iOS 18.0. Simulator arm64 and x86_64 slices support linking and API integration checks, with no guest execution.

<a id="integration-mode"></a>

## Choose your integration

- Kotlin apps use mobile-linux-runtime with transitive installer and native-support AARs. Swift apps select MobileLinuxRuntime, which links the FFI and native-support XCFrameworks.
- Apps that already embed Rust use SDK Rust crates from one Git revision and native-support-only artifacts. Android also needs mobile-linux-installer; iOS selects MobileLinuxNativeSupport.
- Keep exactly one Rust runtime in the process. Native-support distributions contain platform glue and helpers rather than a second Rust library.

<a id="prerequisites"></a>

## Build prerequisites

Source builds pin Rust 1.94.0 and require Python 3.12 or newer. Android uses the Gradle wrapper, JDK 21, SDK platform 37, NDK 27.2.12479018 and cargo-ndk 4.1.2. iOS requires macOS, Xcode with the iOS SDK, Meson and Ninja; SwiftPM wrappers use Swift tools 5.9.

Run source checks from the SDK repository root. SDK_BUILD_DIR must be an absolute caller-owned directory outside the checkout; the SDK does not require a sibling application or Harness checkout.

### SDK-INTEGRATION.md · source-check excerpt

```bash
: "${SDK_BUILD_DIR:?Set an absolute directory outside the source checkout}"
export CARGO_TARGET_DIR="$SDK_BUILD_DIR/rust"
cargo test --locked -p mobile-linux-api -p mobile-linux-core -p platform-pty
bash scripts/checks/check-dependencies.sh
bash scripts/checks/check-resource-contracts.sh
bash scripts/checks/check-resource-tests.sh
```

Verification method: API excerpt

<a id="runtime-handle"></a>

## Create one persistent handle

The UniFFI namespace is mobile_linux_runtime. Kotlin and Swift expose RuntimeHandle through their MobileLinuxRuntime wrapper. Retain that object while task, PTY or raw-stdio handles remain in use.

### types.rs · RuntimeConfig declaration excerpt

```rust
pub struct RuntimeConfig {
    pub platform: RuntimePlatform,
    pub managed_root: String,
    pub app_sandbox_root: String,
    pub abi: String,
    pub rootfs_version: String,
    pub archive_sha256: Option<String>,
    pub native_library_dir: Option<String>,
    pub workspace_host_path: Option<String>,
    pub stable_workspace_id: Option<String>,
    pub authorization_file: Option<String>,
    pub rootfs_archive_path: Option<String>,
    #[uniffi(default = None)]
    pub rootfs_patch_path: Option<String>,
    pub default_mount_path: Option<String>,
    pub protected_host_roots: Vec<String>,
    pub allowed_mount_roots: Vec<String>,
    pub allowed_guest_roots: Vec<String>,
}
```

Verification method: API excerpt

### create_runtime

```text
pub fn create_runtime(config: RuntimeConfig) -> Result<Arc<RuntimeHandle>, MobileLinuxApiErrorFfi>
```

Creates a platform runtime. managed_root and app_sandbox_root must be absolute; ABI and rootfs version must be non-empty. iOS additionally requires workspace_host_path and stable_workspace_id.

<a id="rootfs-lifecycle"></a>

## Verified rootfs lifecycle

Rootfs archives are separate caller inputs and are not silently downloaded or bundled into the SDK. Pin the archive SHA-256 independently and supply the matching manifest and SBOM. Android stages verified tar.gz assets before repair and boot; iOS uses a converted iSH fakefs ZIP and explicit resource paths.

### RuntimeHandle::status

```text
pub async fn status(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi>
```

Read the rootfs lifecycle snapshot, version, paths and last error.

### RuntimeHandle::verify_rootfs

```text
pub async fn verify_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi>
```

Recompute integrity and return the current state.

### RuntimeHandle::repair_rootfs

```text
pub async fn repair_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi>
```

Attempt non-destructive repair; Android activates a valid staged image.

### RuntimeHandle::reset_rootfs

```text
pub async fn reset_rootfs(&self) -> Result<MobileLinuxStatusFfi, MobileLinuxApiErrorFfi>
```

Reset managed rootfs state while preserving external user data; handle RestartRequired on a booted iOS kernel.

<a id="release-identity"></a>

## Release identity

Use checksums and release-provenance.json from the specific release. Source builds, archive presence and simulator linking do not establish physical-device acceptance. Validation-only artifacts are deliberately not release artifacts.

<a id="lifecycle-flow"></a>

## Installation and session flow

Rootfs identity belongs to the embedding app. A version label describes the intended image; it does not substitute for the archive digest, manifest inventory or platform ABI. Keep immutable image assets separate from the writable workspace and guest home. The runtime owns execution handles after the host has admitted a request.

Boot establishes readiness, while verify explicitly refreshes integrity. A status read is diagnostic data; repair and reset are deliberate lifecycle actions. An app should show the returned state and last_error, preserve user workspace data, and ask the runtime to repair only the managed image.

1. Acquire the platform resource bundle and pin its archive digest and revision.
2. Stage and verify the rootfs; construct RuntimeConfig with app-private absolute roots.
3. Retain one runtime, inspect capability, boot, then issue commands or open sessions.
4. Consume output and terminal state; close PTY/raw/background handles before shutdown.

![Resource verification leads to a ready runtime, then owned execution sessions and cleanup.](../assets/platform-mobile-lifecycle.svg)

Repair targets managed rootfs; iOS process-wide kernel changes may require a restart.

<a id="validation-levels"></a>

## Implementation, evidence and open acceptance

The public trait, UniFFI handle and Kotlin/Swift wrappers implement the described integration surfaces. The source repository includes unit/contract checks and device-smoke applications. A documented test command is a repeatable check, not a claim that it has run on every supported device.

Acceptance for a new distribution still requires archive corruption and recovery tests, ABI packaging validation, permission denial, network/memory enforcement, process-tree cancellation and actual guest execution on a physical device. The simulator is suitable for API and linking tests only.

- [Portable Rust contracts](mobile-rust.md)
- [Mount, network and rootfs policy](mobile-authorization.md)
- [Product native host](platform-integration.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/Cargo.toml)

- [mobile-linux-runtime/Cargo.toml](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/Cargo.toml) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)
- [mobile-linux-runtime/rust-toolchain.toml](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/rust-toolchain.toml) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)
- [mobile-linux-runtime/crates/mobile-linux-ffi/src/runtime.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/crates/mobile-linux-ffi/src/runtime.rs) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)
- [mobile-linux-runtime/crates/mobile-linux-ffi/src/types.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/crates/mobile-linux-ffi/src/types.rs) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)
- [mobile-linux-runtime/android/runtime/build.gradle.kts](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/android/runtime/build.gradle.kts) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)
- [mobile-linux-runtime/ios/Package.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/ios/Package.swift) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)
- [mobile-linux-runtime/scripts/checks/check-resource-contracts.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/scripts/checks/check-resource-contracts.sh) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)

- mobile-linux-runtime: HEAD a632e878dc597dc41f6a3712c5a5cdebb794c99c; Referenced input fingerprint sha256:de6f69b8671150c8682b938faea726c0360978b776acaa4c077b257b13792174; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
