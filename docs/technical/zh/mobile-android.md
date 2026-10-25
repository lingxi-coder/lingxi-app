# Android · Kotlin

安装 Maven 分发包、暂存经过验证的 rootfs，并使用持久 Kotlin 运行时。

<a id="maven-bundle"></a>

## 安装发布分发包

如果发布版本提供 mobile-linux-maven-<version>.zip，请使用发布的 SHA-256、精确版本与完整二进制源码修订号。分发包安装器会验证发布身份与文件清单；validation-only 分发包会被拒绝。

根据对应发布版本设置 SDK_MAVEN_ARCHIVE、SDK_MAVEN_SHA256、SDK_VERSION 和 SDK_SOURCE_REVISION，并把 SDK_BUILD_DIR 放在源码目录以外。下载时使用 --url 指定明确的 HTTPS 发布地址。

### Maven 安装摘录

```bash
python3 scripts/release/install-maven-bundle.py \
  --archive "$SDK_MAVEN_ARCHIVE" --sha256 "$SDK_MAVEN_SHA256" \
  --version "$SDK_VERSION" --source-revision "$SDK_SOURCE_REVISION" \
  --cache-dir "$SDK_BUILD_DIR/download-cache" \
  --output-dir "$SDK_BUILD_DIR/maven"
```

验证方式: 接口摘录

<a id="gradle"></a>

## 配置 Gradle

在 settings.gradle.kts 的 pluginManagement 与 dependencyResolutionManagement 中加入已安装的 Maven 目录或发布 Maven 地址。源码指南中的 0.1.0 坐标是本地构建示例，并不表示已发布到 Maven Central。

- 设置 minSdk = 26，并包含 arm64-v8a 与 x86_64。在应用 manifest 中设置 android:extractNativeLibs="true"。
- 打包插件开启 legacy JNI packaging，并在 APK 中校验原生辅助程序存在及 ELF ABI。可选的旧版宿主 shell API 仍要求最低 API 29。
- 已有 Rust 宿主使用 installer 与 native-support 坐标，并设置 mobileLinuxPackaging=native-support。显式提供 applicationInfo.nativeLibraryDir。

### 本地坐标摘录

```kotlin
plugins {
    id("io.github.lingxi-coder.mobile-linux") version "0.1.0"
}
dependencies {
    implementation("io.github.lingxi-coder:mobile-linux-runtime:0.1.0")
}
```

验证方式: 接口摘录

<a id="install-rootfs"></a>

## 暂存经过验证的 rootfs

在 IO 工作线程中调用 RootfsInstaller.stage，提供应用私有 managedRoot、固定的压缩包摘要、匹配的完整 manifest 与 SBOM，以及 copyArchive 回调。manifest ABI 使用 arm64 或 x86_64；Android RuntimeConfig 使用 arm64-v8a 或 x86_64。

### 部分暂存摘录

```kotlin
val result = RootfsInstaller.stage(
            managedRoot = managed, expectedRootfsVersion = version,
            expectedArchiveSha = archiveSha, expectedManifestAbi = manifest.getString("abi"),
            archiveName = archiveRecord.getString("filename"), manifestJson = manifestText,
            sbomJson = File(inputs, "rootfs.spdx.json").readText(),
            copyArchive = { destination -> archive.copyTo(destination, overwrite = true) },
        )
```

验证方式: 接口摘录

### RootfsInstaller.stage

```text
fun stage(managedRoot: File, expectedRootfsVersion: String, expectedArchiveSha: String, expectedManifestAbi: String, archiveName: String, manifestJson: String, sbomJson: String, copyArchive: (File) -> Unit, persistManifest: ((String) -> Unit)? = null): StageResult
```

验证压缩包、不可变清单、符号链接与 SBOM，然后原子发布暂存内容和完整 manifest。StageResult 返回 stagedRoot、manifestPath、verifiedFiles 与 verifiedBytes。

<a id="wrapper"></a>

## Kotlin 运行时封装

### 封装源码

```kotlin
package io.lingxi.mobilelinux

import io.lingxi.mobilelinux.bindings.*

/** One persistent runtime. The embedding app owns policy, paths and rootfs distribution. */
class MobileLinuxRuntime private constructor(val handle: RuntimeHandle) {
    companion object {
        fun create(config: RuntimeConfig): MobileLinuxRuntime = MobileLinuxRuntime(createRuntime(config))
    }
    suspend fun boot(): MobileLinuxStatusFfi = handle.boot()
    suspend fun shutdown() = handle.shutdown()
    suspend fun capability(): MobileLinuxCapabilityFfi = handle.capability()
    suspend fun execute(request: MobileLinuxCommandRequestFfi): MobileLinuxCommandResultFfi = handle.runCommand(request)
    // Raw stdio and PTY are deliberately exposed on handle without text conversion.
}
```

验证方式: 接口摘录

### MobileLinuxRuntime.create

```text
fun create(config: RuntimeConfig): MobileLinuxRuntime
```

通过调用方拥有的配置创建运行时。

### MobileLinuxRuntime.boot

```text
suspend fun boot(): MobileLinuxStatusFfi
```

通过 handle.repairRootfs() 激活暂存 rootfs 后启动。

### MobileLinuxRuntime.execute

```text
suspend fun execute(request: MobileLinuxCommandRequestFfi): MobileLinuxCommandResultFfi
```

运行命令并收集结果。

### MobileLinuxRuntime.shutdown

```text
suspend fun shutdown()
```

宿主生命周期结束时关闭运行时操作。

### MobileLinuxRuntime.capability

```text
suspend fun capability(): MobileLinuxCapabilityFfi
```

检查当前后端可用性与支持的能力。

<a id="android-config-fields"></a>

## 从 Android 宿主填写 RuntimeConfig

跨平台 FFI 记录也包含 iOS 资源和挂载策略字段。Android create_runtime 不消费 iOS authorization_file 或 iOS 白名单字段。应通过 Android 运行时路径配置挂载和请求策略，不要依赖这些字段。

| 字段 | 宿主值 | 常见问题 |
| --- | --- | --- |
| managedRoot | filesDir 下稳定的应用私有运行时目录。 | 把可能被系统清理的 cache 目录作为持久化运行时数据的唯一副本。 |
| appSandboxRoot | 用于校验托管路径的应用文件根目录。 | 传入 /home/lingxi 等 guest 路径。 |
| abi | arm64-v8a 或 x86_64，与打包的原生辅助程序一致。 | 未经映射便把 installer manifest 的 arm64 填入此字段。 |
| rootfsVersion／archiveSha256 | 暂存时使用的精确镜像版本与预期压缩包摘要。 | 把版本标签当作压缩包内容一致的证明。 |
| nativeLibraryDir | Context.applicationInfo.nativeLibraryDir。 | 遗漏提取后的原生辅助库，或打包了不同 ABI。 |

<a id="android-handle-lifetime"></a>

## 跨 Activity 重建管理执行归属

将 MobileLinuxRuntime 保存在生命周期与任务匹配的应用级组件中。进程存活期间 Activity 也可能重建；新建页面时，不能丢失用于清理的唯一进程／PTY／raw 句柄。宿主 coroutine scope 和 foreground-service 策略仍由应用决定。

1. 保留 runtime 与返回的会话或进程句柄，并将输出路由到有界状态容器。
2. 页面重建后使用 listTasks 或 taskStatus 对齐状态；监控运行时事件时，沿用后端游标调用 readEvents。
3. 主动停止时，等待对应句柄的 killProcess、closePty 或 closeRawStdio，再释放 UI 订阅。
4. 应用不再需要任何运行时任务时，等待 shutdown。Android 进程死亡是独立的操作系统事件，不会保留内存中的 SDK 句柄。

<a id="command-example"></a>

## 命令请求

下面的辅助函数摘自 Android 真机 smoke test。默认创建禁用网络的 shell 请求，raw stdio 模式清除超时。调用 execute 或 handle.openRawStdio 前，先安装并启动运行时。

### 辅助函数摘录

```kotlin
private fun command(script: String, network: NetworkPolicyFfi = NetworkPolicyFfi.DISABLED,
                        python: Boolean = false, raw: Boolean = false) = MobileLinuxCommandRequestFfi(
        command = if (python) "/usr/bin/python3" else "/bin/sh",
        args = if (python) listOf("-u", "-c", script) else listOf("-c", script),
        cwd = "/home/lingxi", env = emptyList(), stdin = null,
        timeoutMs = if (raw) null else 15_000uL, network = network,
        resourceLimits = ResourceLimitsFfi(null, null, null, null), mounts = emptyList(),
    )
```

验证方式: 接口摘录

<a id="advanced-handle"></a>

## 流式输出、会话与原生支持

通过 runtime.handle 使用 runCommandStreaming、spawnBackground/killProcess、PTY、raw stdio、事件与任务快照。字节数组不经过文本转换。每个返回句柄的整个生命周期内都应保留 runtime。

Android 原生支持包含网络策略启动器、PRoot overlay 与固定版本的 talloc 源码。它不包含 Rust runtime core 或 FFI cdylib。ARM64 辅助程序构建强制 16 KiB ELF 页对齐，并在发布前检查 PT_LOAD 段。

MainActivity 仅演示对象创建。RealRuntimeSmokeTest 提供经验证的资源并测试 guest 执行；本次文档构建不声称运行过这两个示例。

<a id="android-end-to-end"></a>

## 执行并释放 Kotlin 运行时

传入已由 RootfsInstaller 暂存 rootfs 的 RuntimeConfig，nativeLibraryDir 来自 Context.applicationInfo.nativeLibraryDir。此完整消费者函数包含 boot、有超时上限的命令、结果检查及清理。通过协程在 UI 线程之外执行；展示类型化完整性或策略错误，不要静默重试被拒绝的请求。

### Kotlin 生命周期函数

```kotlin
import io.lingxi.mobilelinux.MobileLinuxRuntime
import io.lingxi.mobilelinux.bindings.*

suspend fun executeOnce(config: RuntimeConfig): MobileLinuxCommandResultFfi {
    val runtime = MobileLinuxRuntime.create(config)
    runtime.boot()
    try {
        val result = runtime.execute(MobileLinuxCommandRequestFfi(
            command = "/bin/sh", args = listOf("-c", "printf 'ready\n'"),
            cwd = "/home/lingxi", env = emptyList(), stdin = null,
            timeoutMs = 10_000uL, network = NetworkPolicyFfi.DISABLED,
            resourceLimits = ResourceLimitsFfi(null, null, null, null),
            mounts = emptyList(),
        ))
        check(result.enforcement.networkPolicyEnforced) { "Network isolation not proven" }
        check(result.exitCode == 0 && !result.timedOut && !result.cancelled)
        return result
    } finally {
        runtime.shutdown()
    }
}
```

验证方式: 需环境验证 (android-runtime-lifecycle)

<a id="android-recovery"></a>

## 打包、恢复与设备验收

辅助库缺失或 ABI 不匹配属于打包故障，应先于模型或 shell 排查。确认选定 APK flavor 含有原生库和已提取的可执行辅助程序。rootfs 安装错误属于压缩包／manifest／SBOM 边界；repair 使用已验证的暂存镜像，reset 必须保留外部用户工作区。

SDK 暴露后台进程，但 Android 可能终止应用进程。宿主前台服务与应用生命周期策略独立于 Linux 任务状态。Activity 重建时保持资源路径稳定，在应用拥有的组件中保留句柄，并读取任务快照，不能根据页面状态推断任务存活。

RealRuntimeSmokeTest 覆盖真实设备请求和策略用例，是验收入口，不表示此文档修改已经执行 instrumentation。对待分发产物验证支持的 arm64-v8a 与 x86_64、超时后代进程、二进制 stdio、终端尺寸调整及拒绝网络／内存策略。

<a id="android-troubleshooting"></a>

## 排查 Android 接入失败

| 现象 | 先检查 | 处理方式 |
| --- | --- | --- |
| 运行时不可用或 PRoot 无法启动 | capability.reason、运行时 ABI、nativeLibraryDir 与已打包的辅助文件。 | 先修正打包／ABI，再排查命令或模型配置。 |
| rootfs 缺失或损坏 | status.state、status.last_error 以及暂存镜像版本／摘要。 | 暂存预期的已校验镜像，再 repairRootfs 与 boot；保留外部工作区。 |
| 命令已返回但功能失败 | exitCode、stderr、timedOut 与 cancelled。 | 分别展示命令结果与 FFI 异常；返回记录不等于退出成功。 |
| 终端文字或协议帧损坏 | PTY／raw stdio 的选择，以及跨 chunk 的解码缓冲。 | PTY 使用终端解码器；raw stdout 使用按字节分帧的解析器。 |
| 取消后仍有任务运行 | 所有者是否等待了对应句柄的 close／kill，并检查其错误。 | 在实际打包的设备版本上验证整个进程树的取消。 |

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/android/runtime/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxRuntime.kt)

- [mobile-linux-runtime/android/runtime/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxRuntime.kt](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/android/runtime/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxRuntime.kt) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/android/installer/src/main/kotlin/io/lingxi/mobilelinux/RootfsInstaller.kt](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/android/installer/src/main/kotlin/io/lingxi/mobilelinux/RootfsInstaller.kt) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/android/gradle-plugin/src/main/java/io/lingxi/mobilelinux/gradle/MobileLinuxPlugin.java](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/android/gradle-plugin/src/main/java/io/lingxi/mobilelinux/gradle/MobileLinuxPlugin.java) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-android/src/runtime/config.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-android/src/runtime/config.rs) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-ffi/src/runtime.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-ffi/src/runtime.rs) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/examples/android/app/src/androidTest/java/io/lingxi/mobilelinux/sample/RealRuntimeSmokeTest.kt](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/examples/android/app/src/androidTest/java/io/lingxi/mobilelinux/sample/RealRuntimeSmokeTest.kt) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/crates/mobile-linux-android/src/runtime/process.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-android/src/runtime/process.rs) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)
- [mobile-linux-runtime/android/installer/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxGuestPaths.kt](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/android/installer/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxGuestPaths.kt) — 已提交源码 (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)

- mobile-linux-runtime: HEAD d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9; 所据文件指纹 sha256:c85076a3035f8a036929b8e4a66cfae06a1e6eac952ef5315ba1cba3470d2139; 产品固定版本 d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
