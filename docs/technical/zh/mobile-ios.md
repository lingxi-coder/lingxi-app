# iOS · Swift

接入 SwiftPM 与 XCFramework 产品、提供 iSH fakefs 资源，并通过 Swift 控制生命周期。

<a id="swiftpm"></a>

## SwiftPM 产品

该 package 为 Swift 应用暴露 MobileLinuxRuntime，为已嵌入 Rust 的应用暴露 MobileLinuxNativeSupport。完整产品链接 MobileLinuxRuntimeFFI 与 MobileLinuxNativeSupport；支持产品加入 sqlite3、resolv、z 和 c++ 链接库。

仓库根目录中已提交的 Package.swift 指向 v0.1.0-rc.3 二进制资源，声明源码版本为 6b033d41a2b6b736fda1e8f996f55a4b20bd885d，并包含实际 checksums。资源是否可用及发布版本的真机验收需核对该次发布。不要用占位值替换 checksums。

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

<a id="ios-config-fields"></a>

## 选择稳定的 Swift 配置值

| 输入 | 规则 | 作用 |
| --- | --- | --- |
| managedRoot／appSandboxRoot | 应用私有的宿主绝对路径，创建会话前进行规范化。 | 分离可替换镜像状态与持久 guest 用户存储。 |
| workspaceHostPath／stableWorkspaceId | 已配置的宿主目录与非空稳定 ID；ID 不能含 /，也不能为 . 或 ..。 | guest 项目路径为 /home/lingxi/lingxi/<stable-id>；修改页面标题不应改变项目身份。 |
| rootfsArchivePath／archiveSha256 | 转换后的 fakefs ZIP 的显式路径及其摘要。 | 源 tarball 的摘要不能校验转换后的 ZIP。 |
| rootfsPatchPath／defaultMountPath | 与 framework 构建匹配的显式资源位置。 | 原生桥接不会推断应用 bundle 或通过环境变量查找它们。 |
| protectedHostRoots／allowedMountRoots／allowedGuestRoots | 受保护宿主目录，以及额外挂载允许使用的 host／guest 前缀。 | 额外白名单为空时，不允许额外的非工作区挂载。 |

<a id="ios-session-identity"></a>

## 区分内核身份与工作区会话

进程级 iSH registry 将不可变配置与 workspaceHostPath、stableWorkspaceId 分开比较。因此，第二个会话可以使用另一个已配置工作区，同时保持相同内核／rootfs 身份。构建时发现不同不可变配置会返回 InvalidRequest，不会静默替换已存在内核。

内核启动后，repair／reset 无法卸载它，会返回 RestartRequired。shutdown 关闭逻辑会话及其临时任务，并不会创建新进程。保存用户请求的恢复动作，由宿主主动重启，不要反复创建新的包装对象。

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

<a id="ios-terminal-and-protocols"></a>

## 在 Swift 中处理终端与子进程协议

| 接口 | 输入／输出 | 生命周期细节 |
| --- | --- | --- |
| PTY | 生成绑定使用 Data；输出既包含文字，也包含终端控制序列。 | iSH 运行时同时支持一个 PTY；新建前关闭现有会话。 |
| resizePty | 终端尺寸是字符单元的行列数，不是像素尺寸。 | 使用非零尺寸；关闭后丢弃旧句柄。 |
| Raw stdio | stdout 与 stderr 使用分开的 Data 缓冲；有界读取 maxBytes 为 1...1048576。 | 不会执行 PTY 字节转换；跨读取累积协议帧，并等待 closeRawStdio。 |
| 后台进程 | spawnBackground 返回进程句柄，不代表获得操作系统后台执行权限。 | 应用负责 scene／操作系统生命周期，停止任务时显式等待 killProcess。 |

终端渲染和 UI 状态在 UI 执行器上处理，执行所有权应位于临时视图之外。取消 Swift Task 不能替代检查 SDK close／kill 的返回结果。

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

应用进程只能使用一个不可变 iSH 内核／rootfs 配置，会话可以使用不同工作区路径与稳定 ID。构建冲突的不可变配置会返回 InvalidRequest；内核启动后修复或重置会返回 RestartRequired。应用重启体验由宿主负责。

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

验证方式: 需环境验证 (ios-runtime-lifecycle)

<a id="ios-recovery-and-evidence"></a>

## 进程身份与验收边界

稳定工作区身份与进程不可变内核身份相互独立。切换工作区须保留内核／rootfs 配置。构建冲突配置返回 InvalidRequest；内核初始化后的 repair／reset 返回 RestartRequired。保存所请求的恢复状态，提供明确的宿主重启动作，不要在同一进程内循环 boot。

Swift 并发本身不授予后台执行时间。应用负责系统挂起、scene 切换及资源清理。iOS 模拟器 native-support 切片会明确报告 guest 执行不可用；API 生成和模拟器链接不能替代已签名真机运行。

SDKDeviceSmoke 是真机验收源码。框架与资源包须作为一组验证，包括安装中断、完整性刷新、要求重启的行为、raw 字节保持及完整进程树取消。使用伪原生传输的单元测试仅证明生命周期逻辑。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/ios/SDK/MobileLinuxRuntime.swift)

- [mobile-linux-runtime/ios/SDK/MobileLinuxRuntime.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/ios/SDK/MobileLinuxRuntime.swift) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/ios/Package.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/ios/Package.swift) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/config.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/config.rs) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/lifecycle.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/lifecycle.rs) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/implementation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/implementation.rs) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/examples/ios/App/SDKDeviceSmoke.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/examples/ios/App/SDKDeviceSmoke.swift) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/scripts/build/prepare-ios-rootfs.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/scripts/build/prepare-ios-rootfs.sh) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ios/src/runtime/validation.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ios/src/runtime/validation.rs) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-api/src/mobile_linux.rs) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/Package.swift](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/Package.swift) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)

- mobile-linux-runtime: HEAD d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9; 所据文件指纹 sha256:c85076a3035f8a036929b8e4a66cfae06a1e6eac952ef5315ba1cba3470d2139; 产品固定版本 d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
