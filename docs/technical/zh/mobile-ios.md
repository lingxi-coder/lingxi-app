# iOS · Swift

接入 SwiftPM 与 XCFramework 产品、提供 iSH fakefs 资源，并通过 Swift 控制生命周期。

<a id="swiftpm"></a>

## SwiftPM 产品

该 package 为 Swift 应用暴露 MobileLinuxRuntime，为已嵌入 Rust 的应用暴露 MobileLinuxNativeSupport。完整产品链接 MobileLinuxRuntimeFFI 与 MobileLinuxNativeSupport；支持产品加入 sqlite3、resolv、z 和 c++ 链接库。

仓库根目录中已提交的 Package.swift 指向 v0.1.0-rc.2 二进制资源，声明源码版本为 9e8e19a473728722183fcbfa0814398c1dd8a8ff，并包含实际 checksums。资源是否可用及发布版本的真机验收需核对该次发布。不要用占位值替换 checksums。

本地 SwiftPM 接入时，将两个构建完成的 frameworks 放到 ios/Artifacts，并把匹配的生成绑定放到 ios/Bindings，然后添加本地 ios package。缺少这些二进制产物时，本地 manifest 无法从干净检出目录解析。

### 本地 target 摘录

```swift
    targets: [
        .binaryTarget(name: "MobileLinuxRuntimeFFI", path: "Artifacts/MobileLinuxRuntimeFFI.xcframework"),
        .binaryTarget(name: "MobileLinuxNativeSupport", path: "Artifacts/MobileLinuxNativeSupport.xcframework"),
        .target(name: "MobileLinuxNativeSupportLink", dependencies: ["MobileLinuxNativeSupport"], path: "NativeSupportLink", linkerSettings: [.linkedLibrary("sqlite3"), .linkedLibrary("resolv"), .linkedLibrary("z"), .linkedLibrary("c++")]),
        .target(name: "MobileLinuxRuntimeBindings", dependencies: ["MobileLinuxRuntimeFFI"], path: "Bindings"),
        .target(name: "MobileLinuxRuntime", dependencies: ["MobileLinuxRuntimeBindings", "MobileLinuxNativeSupportLink"], path: "SDK"),
```

验证方式: 接口摘录

<a id="build-frameworks"></a>

## 从源码构建 frameworks

从同一源码修订版本分别构建原生支持与 FFI。模拟器切片支持链接与 API 检查；真正的 guest 执行要求 arm64 iOS 真机。

### XCFramework 构建摘录

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
bash scripts/build/build-ios-xcframework.sh \
  --output "$SDK_BUILD_DIR/ios-native" --cache "$SDK_BUILD_DIR/ios-cache" \
  --kind native-support --configuration Release
python3 scripts/build/build-ffi.py --platform ios --release \
  --output-dir "$SDK_BUILD_DIR/ios-ffi" --target-dir "$SDK_BUILD_DIR/ios-rust"
```

验证方式: 接口摘录

<a id="ios-rootfs"></a>

## 准备 iSH fakefs 资源

iOS 使用 iSH fakefs ZIP，而不是 Android/Linux tarball。使用 SDK 固定的工具转换经过验证的 aarch64 rootfs，然后分发 ZIP、manifest、RootfsPatch.bundle、libvdso.so.elf 与默认挂载资源。RuntimeConfig 必须使用转换后 ZIP 的摘要。

提供应用私有的绝对 managedRoot 与 appSandboxRoot、workspaceHostPath、持久 stableWorkspaceId、rootfsArchivePath、rootfsPatchPath 与 defaultMountPath。应用还负责选择 protectedHostRoots、allowedMountRoots 与 allowedGuestRoots。

### rootfs 转换摘录

```bash
: "${ROOTFS_ARCHIVE:?Set the aarch64 archive path}"
: "${ROOTFS_ARCHIVE_SHA256:?Set its pinned SHA-256}"
bash scripts/build/prepare-ios-rootfs.sh \
  --archive "$ROOTFS_ARCHIVE" --expected-archive-sha256 "$ROOTFS_ARCHIVE_SHA256" \
  --profile toolchain --native-output "$SDK_BUILD_DIR/ios-native/native" \
  --output "$SDK_BUILD_DIR/ios-rootfs" --cache "$SDK_BUILD_DIR/ios-cache"
```

验证方式: 接口摘录

<a id="swift-wrapper"></a>

## Swift 运行时封装

### 封装源码

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

验证方式: 接口摘录

### MobileLinuxRuntime.init

```text
public init(configuration: RuntimeConfig) throws
```

创建运行时。使用 handle 上的任务、PTY 与 raw stdio 时保留该对象。

### MobileLinuxRuntime.boot

```text
public func boot() async throws -> MobileLinuxStatusFfi
```

安装并启动已配置的后端，返回状态。

### MobileLinuxRuntime.shutdown

```text
public func shutdown() async throws
```

关闭逻辑运行时任务。iSH 内核仍存活到应用进程结束。

### MobileLinuxRuntime.capability

```text
public func capability() async -> MobileLinuxCapabilityFfi
```

执行前探测当前可用性。

<a id="swift-handle"></a>

## 命令与字节流

命令执行通过 runtime.handle 暴露。生成的 Swift 绑定使用 camelCase 名称，并用 Data 表示字节流。以下签名来自已提交的 RuntimeHandleProtocol。

### RuntimeHandle.runCommand

```text
func runCommand(request: MobileLinuxCommandRequestFfi) async throws -> MobileLinuxCommandResultFfi
```

运行命令并收集 stdout/stderr 与执行回执。

### RuntimeHandle.runCommandStreaming

```text
func runCommandStreaming(request: MobileLinuxCommandRequestFfi, sink: RuntimeEventSink) async throws -> MobileLinuxCommandResultFfi
```

命令运行时流式接收结构化运行时事件。

### RuntimeHandle.openRawStdio

```text
func openRawStdio(request: MobileLinuxCommandRequestFfi) async throws -> RawStdioHandle
```

打开精确的二进制 stdin/stdout/stderr；请求中不设置 stdin 或 timeout。

### RuntimeHandle.writeRawStdio

```text
func writeRawStdio(id: String, data: Data) async throws
```

写入二进制 stdin，不做文本转换。

### RuntimeHandle.readRawStdio

```text
func readRawStdio(id: String, maxBytes: UInt32) async throws -> RawStdioOutput
```

读取有上限的输出，并检查 closed 与 exitCode。

### RuntimeHandle.openPty

```text
func openPty(request: MobileLinuxPtyOpenRequestFfi) async throws -> MobileLinuxPtySessionHandleFfi
```

打开交互式 PTY 进程。

### RuntimeHandle.writePty

```text
func writePty(handle: MobileLinuxPtySessionHandleFfi, input: Data) async throws
```

写入终端字节。

<a id="swift-example"></a>

## 命令辅助函数

这段真机 smoke 的部分源码摘录提供禁用网络的命令，以及可选内存与超时限制。把请求传给 handle.runCommand 之前，先创建并启动配置好的运行时。

### 辅助函数摘录

```swift
    private func command(_ script: String, network: NetworkPolicyFfi = .disabled, memory: UInt32? = nil, timeout: UInt64? = 10_000) -> MobileLinuxCommandRequestFfi {
        MobileLinuxCommandRequestFfi(command: "/bin/sh", args: ["-c", script], cwd: nil, env: [], stdin: nil,
            timeoutMs: timeout, network: network,
            resourceLimits: ResourceLimitsFfi(maxCpuSeconds: nil, maxMemoryMb: memory, maxProcesses: nil, maxOpenFiles: nil), mounts: [])
    }
```

验证方式: 接口摘录

<a id="ios-lifetime"></a>

## 内核生命周期与重启

一个应用进程具有一份不可变的 iSH 内核与 rootfs 配置，会话可以使用不同工作区路径和稳定 ID。修改已初始化的内核配置，或在启动后 repair/reset，可能返回 RestartRequired。宿主负责应用重启体验。

<a id="ios-end-to-end"></a>

## 执行并关闭 Swift 会话

此完整函数接收宿主创建的配置，启动逻辑会话、执行有时间上限的请求，并在成功或失败时关闭会话。宿主还须检查 exitCode、timedOut、cancelled 与 enforcement.networkPolicyEnforced 后才接受输出。真机压缩包、overlay、VDSO 与挂载资源必须匹配选定 SDK 构建。

### Swift 生命周期函数

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

验证方式: 需环境验证 (ios-runtime-lifecycle)

<a id="ios-recovery-and-evidence"></a>

## 进程身份与验收边界

稳定工作区身份与进程不可变内核身份相互独立。切换工作区须保留内核／rootfs 配置。当 repair、reset 或配置变更返回 RestartRequired，保存所请求状态并提供明确的宿主重启动作，不要在同一进程内循环 boot。

Swift 并发本身不授予后台执行时间。应用负责系统挂起、scene 切换及资源清理。iOS 模拟器 native-support 切片会明确报告 guest 执行不可用；API 生成和模拟器链接不能替代已签名真机运行。

SDKDeviceSmoke 是真机验收源码。框架与资源包须作为一组验证，包括安装中断、完整性刷新、要求重启的行为、raw 字节保持及完整进程树取消。使用伪原生传输的单元测试仅证明生命周期逻辑。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/ios/SDK/MobileLinuxRuntime.swift)

- [mobile-linux-runtime/ios/SDK/MobileLinuxRuntime.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/ios/SDK/MobileLinuxRuntime.swift) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/ios/Package.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/ios/Package.swift) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/config.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-ios/src/runtime/config.rs) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/lifecycle.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-ios/src/runtime/lifecycle.rs) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/implementation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-ios/src/runtime/implementation.rs) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/examples/ios/App/SDKDeviceSmoke.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/examples/ios/App/SDKDeviceSmoke.swift) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/scripts/build/prepare-ios-rootfs.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/scripts/build/prepare-ios-rootfs.sh) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)

- mobile-linux-runtime: HEAD eb82dcd73080bc35d0236abc15209a3bbee9f52c; 所据文件指纹 sha256:78374cc6ec701b4b4e082679fb8033d488b6379c3345bd5d03cd383177c3daf4; 产品固定版本 b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
