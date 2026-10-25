# iOS · Swift

Consume SwiftPM and XCFramework products, provide iSH fakefs resources, and control lifecycle through Swift.

<a id="swiftpm"></a>

## SwiftPM products

The package exposes MobileLinuxRuntime for Swift applications and MobileLinuxNativeSupport for applications already embedding Rust. The full product links MobileLinuxRuntimeFFI and MobileLinuxNativeSupport; the support product adds sqlite3, resolv, z and c++ linker libraries.

The checked-in repository-root Package.swift points to v0.1.0-rc.3 binary assets built from source 6b033d41a2b6b736fda1e8f996f55a4b20bd885d and contains their actual checksums. Asset availability and release device acceptance must be checked against that release. Do not replace checksums with placeholder values.

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

<a id="ios-config-fields"></a>

## Choose stable Swift configuration values

| Input | Rule | Why it matters |
| --- | --- | --- |
| managedRoot / appSandboxRoot | App-private absolute host paths, normalized before the session is created. | Separate replaceable image state from persistent guest user storage. |
| workspaceHostPath / stableWorkspaceId | A configured host directory and a non-empty ID with no /. The values . and .. are rejected. | The guest project path is /home/lingxi/lingxi/<stable-id>; changing a screen title should not change project identity. |
| rootfsArchivePath / archiveSha256 | The explicit converted fakefs ZIP path and its digest. | The original tarball digest does not authenticate a converted ZIP. |
| rootfsPatchPath / defaultMountPath | Explicit resource locations matching the framework build. | The native bridge does not infer the application bundle or search environment variables for them. |
| protectedHostRoots / allowedMountRoots / allowedGuestRoots | Protected host trees plus explicit host and guest prefixes for additional mounts. | An empty extra allowlist admits no extra non-workspace mounts. |

<a id="ios-session-identity"></a>

## Understand kernel identity and workspace sessions

The process-wide iSH registry compares immutable configuration separately from workspaceHostPath and stableWorkspaceId. A second session can therefore target another configured workspace while keeping the same kernel/rootfs identity. A different immutable configuration is rejected at construction with InvalidRequest; it is not a request to silently replace an existing kernel.

After the kernel starts, repair/reset cannot unload it and return RestartRequired. shutdown closes the logical session and its transient work; it does not create a fresh process. Persist the user’s requested recovery action and let the host restart deliberately rather than repeatedly constructing a new wrapper.

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

<a id="ios-terminal-and-protocols"></a>

## Handle terminals and child protocols in Swift

| Surface | Input / output | Lifecycle detail |
| --- | --- | --- |
| PTY | Generated bindings use Data; output contains terminal control sequences as well as text. | The iSH runtime supports one PTY at a time. Close the existing session before opening another. |
| resizePty | Terminal dimensions are rows and columns in cells, not pixel sizes. | Use nonzero dimensions and discard the old handle after close. |
| Raw stdio | Separate stdout and stderr Data buffers; bounded read maxBytes is 1...1048576. | No PTY byte translation; accumulate protocol frames across reads and await closeRawStdio. |
| Background process | spawnBackground returns a process handle, not an OS background-execution entitlement. | The app owns scene/OS lifetime and explicitly awaits killProcess when stopping the task. |

Keep terminal rendering and UI state on the UI executor, and keep execution ownership outside a transient view. A Swift Task cancellation is not a substitute for checking the SDK close/kill result.

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

An app process has one immutable iSH kernel/rootfs configuration, while sessions may use different workspace paths and stable IDs. Constructing a conflicting immutable configuration returns InvalidRequest. Repairing or resetting after the kernel has started returns RestartRequired; the host owns the application restart experience.

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
            command: "/bin/sh", args: ["-c", "printf 'ready\n'"], cwd: "/home/lingxi",
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

Stable workspace identity is independent of the immutable process kernel identity. Switching workspaces must preserve the kernel/rootfs configuration. A conflicting constructor configuration is InvalidRequest; repair/reset after kernel initialization is RestartRequired. Persist the requested recovery state and surface a deliberate host restart action instead of looping boot attempts in the same process.

Swift concurrency does not itself grant background execution time. The application owns OS suspension, scene transitions and resource cleanup. An iOS simulator native-support slice deliberately reports unavailable guest execution; API generation and simulator linking cannot replace a signed physical-device run.

SDKDeviceSmoke is the physical-device acceptance source. Validate the exact framework and resource bundle together, including interrupted install, integrity refresh, restart-required behavior, raw byte preservation and whole-tree cancellation. Unit tests with fake native transport establish lifecycle logic only.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/ios/SDK/MobileLinuxRuntime.swift)

- [mobile-linux-runtime/ios/SDK/MobileLinuxRuntime.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/ios/SDK/MobileLinuxRuntime.swift) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/ios/Package.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/ios/Package.swift) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/config.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/config.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/lifecycle.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/lifecycle.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/implementation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/implementation.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/examples/ios/App/SDKDeviceSmoke.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/examples/ios/App/SDKDeviceSmoke.swift) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/scripts/build/prepare-ios-rootfs.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/scripts/build/prepare-ios-rootfs.sh) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/validation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/validation.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/Package.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/Package.swift) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)

- mobile-linux-runtime: HEAD d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9; Referenced input fingerprint sha256:c85076a3035f8a036929b8e4a66cfae06a1e6eac952ef5315ba1cba3470d2139; Product pin d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
