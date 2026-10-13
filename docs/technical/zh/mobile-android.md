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

<a id="command-example"></a>

## 命令请求

下面的辅助函数摘自 Android 真机 smoke test。默认创建禁用网络的 shell 请求，raw stdio 模式清除超时。调用 execute 或 handle.openRawStdio 前，先安装并启动运行时。

### 辅助函数摘录

```kotlin
private fun command(script: String, network: NetworkPolicyFfi = NetworkPolicyFfi.DISABLED,
                        python: Boolean = false, raw: Boolean = false) = MobileLinuxCommandRequestFfi(
        command = if (python) "/usr/bin/python3" else "/bin/sh",
        args = if (python) listOf("-u", "-c", script) else listOf("-c", script),
        cwd = "/root", env = emptyList(), stdin = null,
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
            cwd = "/root", env = emptyList(), stdin = null,
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

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/android/runtime/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxRuntime.kt)

- [mobile-linux-runtime/android/runtime/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxRuntime.kt](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/android/runtime/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxRuntime.kt) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/android/installer/src/main/kotlin/io/lingxi/mobilelinux/RootfsInstaller.kt](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/android/installer/src/main/kotlin/io/lingxi/mobilelinux/RootfsInstaller.kt) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/android/gradle-plugin/src/main/java/io/lingxi/mobilelinux/gradle/MobileLinuxPlugin.java](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/android/gradle-plugin/src/main/java/io/lingxi/mobilelinux/gradle/MobileLinuxPlugin.java) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/crates/mobile-linux-android/src/runtime/config.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-android/src/runtime/config.rs) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/crates/mobile-linux-ffi/src/runtime.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/crates/mobile-linux-ffi/src/runtime.rs) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)
- [mobile-linux-runtime/examples/android/app/src/androidTest/java/io/lingxi/mobilelinux/sample/RealRuntimeSmokeTest.kt](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/examples/android/app/src/androidTest/java/io/lingxi/mobilelinux/sample/RealRuntimeSmokeTest.kt) — 已提交源码 (eb82dcd73080bc35d0236abc15209a3bbee9f52c)

- mobile-linux-runtime: HEAD eb82dcd73080bc35d0236abc15209a3bbee9f52c; 所据文件指纹 sha256:78374cc6ec701b4b4e082679fb8033d488b6379c3345bd5d03cd383177c3daf4; 产品固定版本 b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
