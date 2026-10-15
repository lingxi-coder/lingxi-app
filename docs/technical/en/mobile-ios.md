# iOS · Swift

Consume SwiftPM and XCFramework products, provide iSH fakefs resources, and control lifecycle through Swift.

<a id="swiftpm"></a>

## SwiftPM products

The package exposes MobileLinuxRuntime for Swift applications and MobileLinuxNativeSupport for applications already embedding Rust. The full product links MobileLinuxRuntimeFFI and MobileLinuxNativeSupport; the support product adds sqlite3, resolv, z and c++ linker libraries.

The checked-in repository-root Package.swift points to v0.1.0-rc.2 binary assets built from source 9e8e19a473728722183fcbfa0814398c1dd8a8ff and contains their actual checksums. Asset availability and release device acceptance must be checked against that release. Do not replace checksums with placeholder values.

For local SwiftPM, stage the two built frameworks in ios/Artifacts and matching generated bindings in ios/Bindings, then add the local ios package. The local manifest cannot resolve from a clean checkout without those binary artifacts.

### ios/Package.swift · local target excerpt

```swift
    targets: [
        .binaryTarget(name: "MobileLinuxRuntimeFFI", path: "Artifacts/MobileLinuxRuntimeFFI.xcframework"),
        .binaryTarget(name: "MobileLinuxNativeSupport", path: "Artifacts/MobileLinuxNativeSupport.xcframework"),
        .target(name: "MobileLinuxNativeSupportLink", dependencies: ["MobileLinuxNativeSupport"], path: "NativeSupportLink", linkerSettings: [.linkedLibrary("sqlite3"), .linkedLibrary("resolv"), .linkedLibrary("z"), .linkedLibrary("c++")]),
        .target(name: "MobileLinuxRuntimeBindings", dependencies: ["MobileLinuxRuntimeFFI"], path: "Bindings"),
        .target(name: "MobileLinuxRuntime", dependencies: ["MobileLinuxRuntimeBindings", "MobileLinuxNativeSupportLink"], path: "SDK"),
```

Verification method: API excerpt

<a id="build-frameworks"></a>

## Build frameworks from source

Build native support and FFI independently from the same source revision. Simulator slices support linking and API checks; real guest execution requires an arm64 iOS device.

### SDK-INTEGRATION.md · XCFramework build excerpt

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
bash scripts/build/build-ios-xcframework.sh \
  --output "$SDK_BUILD_DIR/ios-native" --cache "$SDK_BUILD_DIR/ios-cache" \
  --kind native-support --configuration Release
python3 scripts/build/build-ffi.py --platform ios --release \
  --output-dir "$SDK_BUILD_DIR/ios-ffi" --target-dir "$SDK_BUILD_DIR/ios-rust"
```

Verification method: API excerpt

<a id="ios-rootfs"></a>

## Prepare iSH fakefs resources

iOS consumes an iSH fakefs ZIP rather than the Android/Linux tarball. Convert a verified aarch64 rootfs with the pinned SDK tool, then distribute its ZIP, manifest, RootfsPatch.bundle, libvdso.so.elf and default mount resources. RuntimeConfig must use the converted ZIP digest.

Supply absolute app-private managedRoot and appSandboxRoot, workspaceHostPath, persistent stableWorkspaceId, rootfsArchivePath, rootfsPatchPath and defaultMountPath. The app also chooses protectedHostRoots, allowedMountRoots and allowedGuestRoots.

### SDK-INTEGRATION.md · rootfs conversion excerpt

```bash
: "${ROOTFS_ARCHIVE:?Set the aarch64 archive path}"
: "${ROOTFS_ARCHIVE_SHA256:?Set its pinned SHA-256}"
bash scripts/build/prepare-ios-rootfs.sh \
  --archive "$ROOTFS_ARCHIVE" --expected-archive-sha256 "$ROOTFS_ARCHIVE_SHA256" \
  --profile toolchain --native-output "$SDK_BUILD_DIR/ios-native/native" \
  --output "$SDK_BUILD_DIR/ios-rootfs" --cache "$SDK_BUILD_DIR/ios-cache"
```

Verification method: API excerpt

<a id="swift-wrapper"></a>

## Swift runtime wrapper

### MobileLinuxRuntime.swift · wrapper source

```swift
import MobileLinuxRuntimeBindings

/// The app chooses paths, authorization, mount policy and rootfs distribution.
/// Keep this object alive while using its task, PTY and raw-stdio handles.
public final class MobileLinuxRuntime {
    public let handle: RuntimeHandle
    public init(configuration: RuntimeConfig) throws {
        self.handle = try createRuntime(config: configuration)
    }
    public func boot() async throws -> MobileLinuxStatusFfi { try await handle.boot() }
    public func shutdown() async throws { try await handle.shutdown() }
    public func capability() async -> MobileLinuxCapabilityFfi { await handle.capability() }
}
```

Verification method: API excerpt

### MobileLinuxRuntime.init

```text
public init(configuration: RuntimeConfig) throws
```

Create a runtime. Retain it while using handle tasks, PTY and raw stdio.

### MobileLinuxRuntime.boot

```text
public func boot() async throws -> MobileLinuxStatusFfi
```

Install/boot the configured backend and return status.

### MobileLinuxRuntime.shutdown

```text
public func shutdown() async throws
```

Close logical runtime tasks. The iSH kernel remains alive for the app process lifetime.

### MobileLinuxRuntime.capability

```text
public func capability() async -> MobileLinuxCapabilityFfi
```

Probe live availability before execution.

<a id="swift-handle"></a>

## Commands and byte streams

Command execution is exposed on runtime.handle. Generated Swift bindings use camelCase names and Data for byte streams. The following signatures are from the checked-in RuntimeHandleProtocol.

### RuntimeHandle.runCommand

```text
func runCommand(request: MobileLinuxCommandRequestFfi) async throws -> MobileLinuxCommandResultFfi
```

Run a command and collect stdout/stderr and enforcement metadata.

### RuntimeHandle.runCommandStreaming

```text
func runCommandStreaming(request: MobileLinuxCommandRequestFfi, sink: RuntimeEventSink) async throws -> MobileLinuxCommandResultFfi
```

Stream structured runtime events while the command runs.

### RuntimeHandle.openRawStdio

```text
func openRawStdio(request: MobileLinuxCommandRequestFfi) async throws -> RawStdioHandle
```

Open exact binary stdin/stdout/stderr with no request stdin or timeout.

### RuntimeHandle.writeRawStdio

```text
func writeRawStdio(id: String, data: Data) async throws
```

Write binary stdin without text conversion.

### RuntimeHandle.readRawStdio

```text
func readRawStdio(id: String, maxBytes: UInt32) async throws -> RawStdioOutput
```

Read bounded output and inspect closed and exitCode.

### RuntimeHandle.openPty

```text
func openPty(request: MobileLinuxPtyOpenRequestFfi) async throws -> MobileLinuxPtySessionHandleFfi
```

Open an interactive PTY process.

### RuntimeHandle.writePty

```text
func writePty(handle: MobileLinuxPtySessionHandleFfi, input: Data) async throws
```

Write terminal bytes.

<a id="swift-example"></a>

## Command helper

This partial source excerpt from the device smoke supplies a disabled-network command and optional memory/timeout limits. Create and boot a configured runtime before passing the request to handle.runCommand.

### SDKDeviceSmoke.swift · helper excerpt

```swift
    private func command(_ script: String, network: NetworkPolicyFfi = .disabled, memory: UInt32? = nil, timeout: UInt64? = 10_000) -> MobileLinuxCommandRequestFfi {
        MobileLinuxCommandRequestFfi(command: "/bin/sh", args: ["-c", script], cwd: nil, env: [], stdin: nil,
            timeoutMs: timeout, network: network,
            resourceLimits: ResourceLimitsFfi(maxCpuSeconds: nil, maxMemoryMb: memory, maxProcesses: nil, maxOpenFiles: nil), mounts: [])
    }
```

Verification method: API excerpt

<a id="ios-lifetime"></a>

## Kernel lifetime and restart

An app process has one immutable iSH kernel/rootfs configuration, while sessions may use different workspace paths and stable IDs. Changing an initialized kernel configuration or repairing/resetting after boot can return RestartRequired. The host owns the app restart experience.

<a id="ios-end-to-end"></a>

## Execute and close a Swift session

This complete function receives a host-created configuration, boots the logical session, executes a bounded request and closes it on both success and error. The host must additionally inspect exitCode, timedOut, cancelled and enforcement.networkPolicyEnforced before accepting output. Device archive, overlay, VDSO and mount resources must match the selected SDK build.

### Swift lifecycle function

```swift
import MobileLinuxRuntime
import MobileLinuxRuntimeBindings

func executeOnce(config: RuntimeConfig) async throws -> MobileLinuxCommandResultFfi {
    let runtime = try MobileLinuxRuntime(configuration: config)
    _ = try await runtime.boot()
    do {
        let request = MobileLinuxCommandRequestFfi(
            command: "/bin/sh", args: ["-c", "printf 'ready\n'"], cwd: "/root",
            env: [], stdin: nil, timeoutMs: 10_000, network: .disabled,
            resourceLimits: ResourceLimitsFfi(maxCpuSeconds: nil, maxMemoryMb: nil,
                maxProcesses: nil, maxOpenFiles: nil), mounts: [])
        let result = try await runtime.handle.runCommand(request: request)
        try await runtime.shutdown()
        return result
    } catch {
        try? await runtime.shutdown()
        throw error
    }
}
```

Verification method: Environment validation (ios-runtime-lifecycle)

<a id="ios-recovery-and-evidence"></a>

## Process identity and acceptance boundary

Stable workspace identity is independent of the immutable process kernel identity. Switching workspaces must preserve the kernel/rootfs configuration. When repair, reset or configuration change returns RestartRequired, persist the requested state and surface a deliberate host restart action; do not loop boot attempts in the same process.

Swift concurrency does not itself grant background execution time. The application owns OS suspension, scene transitions and resource cleanup. An iOS simulator native-support slice deliberately reports unavailable guest execution; API generation and simulator linking cannot replace a signed physical-device run.

SDKDeviceSmoke is the physical-device acceptance source. Validate the exact framework and resource bundle together, including interrupted install, integrity refresh, restart-required behavior, raw byte preservation and whole-tree cancellation. Unit tests with fake native transport establish lifecycle logic only.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/ios/SDK/MobileLinuxRuntime.swift)

- [mobile-linux-runtime/ios/SDK/MobileLinuxRuntime.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/ios/SDK/MobileLinuxRuntime.swift) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)
- [mobile-linux-runtime/ios/Package.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/ios/Package.swift) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/config.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/crates/mobile-linux-ios/src/runtime/config.rs) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/lifecycle.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/crates/mobile-linux-ios/src/runtime/lifecycle.rs) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/implementation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/crates/mobile-linux-ios/src/runtime/implementation.rs) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)
- [mobile-linux-runtime/examples/ios/App/SDKDeviceSmoke.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/examples/ios/App/SDKDeviceSmoke.swift) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)
- [mobile-linux-runtime/scripts/build/prepare-ios-rootfs.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/scripts/build/prepare-ios-rootfs.sh) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)

- mobile-linux-runtime: HEAD a632e878dc597dc41f6a3712c5a5cdebb794c99c; Referenced input fingerprint sha256:de6f69b8671150c8682b938faea726c0360978b776acaa4c077b257b13792174; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
