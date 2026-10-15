# Android · Kotlin

Install the Maven bundle, stage a verified rootfs, and operate a persistent Kotlin runtime.

<a id="maven-bundle"></a>

## Install a release bundle

When a release provides mobile-linux-maven-<version>.zip, use its published SHA-256, exact version and full binary source revision. The bundle installer verifies release identity and the recorded file inventory; validation-only bundles are rejected.

Set SDK_MAVEN_ARCHIVE, SDK_MAVEN_SHA256, SDK_VERSION and SDK_SOURCE_REVISION from that release, with SDK_BUILD_DIR outside the checkout. Use --url with an explicit HTTPS release URL when downloading.

### SDK-INTEGRATION.md · Maven installer excerpt

```bash
python3 scripts/release/install-maven-bundle.py \
  --archive "$SDK_MAVEN_ARCHIVE" --sha256 "$SDK_MAVEN_SHA256" \
  --version "$SDK_VERSION" --source-revision "$SDK_SOURCE_REVISION" \
  --cache-dir "$SDK_BUILD_DIR/download-cache" \
  --output-dir "$SDK_BUILD_DIR/maven"
```

Verification method: API excerpt

<a id="gradle"></a>

## Configure Gradle

Add the installed Maven directory or release Maven URL to both pluginManagement and dependencyResolutionManagement in settings.gradle.kts. The 0.1.0 coordinate in the source guide is a local build example; it does not claim a Maven Central publication.

- Set minSdk = 26 and include arm64-v8a and x86_64. Set android:extractNativeLibs="true" in the application manifest.
- The packaging plugin enables legacy JNI packaging and verifies native helper presence and ELF ABI in the APK. Optional legacy host-shell APIs retain their API 29 minimum.
- An existing Rust host uses installer and native-support coordinates with mobileLinuxPackaging=native-support. Supply applicationInfo.nativeLibraryDir explicitly.

### SDK-INTEGRATION.md · local-coordinate excerpt

```kotlin
plugins {
    id("io.github.lingxi-coder.mobile-linux") version "0.1.0"
}
dependencies {
    implementation("io.github.lingxi-coder:mobile-linux-runtime:0.1.0")
}
```

Verification method: API excerpt

<a id="install-rootfs"></a>

## Stage a verified rootfs

Call RootfsInstaller.stage on an I/O worker with an app-private managedRoot, the pinned archive digest, matching full manifest and SBOM, and a copyArchive callback. The manifest ABI is arm64 or x86_64; Android RuntimeConfig uses arm64-v8a or x86_64.

### RealRuntimeSmokeTest.kt · partial staging excerpt

```kotlin
val result = RootfsInstaller.stage(
            managedRoot = managed, expectedRootfsVersion = version,
            expectedArchiveSha = archiveSha, expectedManifestAbi = manifest.getString("abi"),
            archiveName = archiveRecord.getString("filename"), manifestJson = manifestText,
            sbomJson = File(inputs, "rootfs.spdx.json").readText(),
            copyArchive = { destination -> archive.copyTo(destination, overwrite = true) },
        )
```

Verification method: API excerpt

### RootfsInstaller.stage

```text
fun stage(managedRoot: File, expectedRootfsVersion: String, expectedArchiveSha: String, expectedManifestAbi: String, archiveName: String, manifestJson: String, sbomJson: String, copyArchive: (File) -> Unit, persistManifest: ((String) -> Unit)? = null): StageResult
```

Verifies the archive, immutable inventory, symlinks and SBOM, then atomically publishes staging and the full manifest. StageResult reports stagedRoot, manifestPath, verifiedFiles and verifiedBytes.

<a id="wrapper"></a>

## Kotlin runtime wrapper

### MobileLinuxRuntime.kt · wrapper source

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

Verification method: API excerpt

### MobileLinuxRuntime.create

```text
fun create(config: RuntimeConfig): MobileLinuxRuntime
```

Create the runtime with caller-owned configuration.

### MobileLinuxRuntime.boot

```text
suspend fun boot(): MobileLinuxStatusFfi
```

Boot after activating staged rootfs with handle.repairRootfs().

### MobileLinuxRuntime.execute

```text
suspend fun execute(request: MobileLinuxCommandRequestFfi): MobileLinuxCommandResultFfi
```

Run a command and collect its result.

### MobileLinuxRuntime.shutdown

```text
suspend fun shutdown()
```

Close runtime operations when the host lifecycle ends.

### MobileLinuxRuntime.capability

```text
suspend fun capability(): MobileLinuxCapabilityFfi
```

Inspect live backend availability and supported features.

<a id="command-example"></a>

## Command request

This helper is copied from the Android device smoke test. It creates a disabled-network shell request by default and clears the timeout for raw stdio. Install and boot the runtime before calling execute or handle.openRawStdio.

### RealRuntimeSmokeTest.kt · helper excerpt

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

Verification method: API excerpt

<a id="advanced-handle"></a>

## Streaming, sessions and native support

Use runtime.handle for runCommandStreaming, spawnBackground/killProcess, PTY, raw stdio, events and task snapshots. Byte arrays pass through without text conversion. Retain runtime for the lifetime of every returned handle.

Android native support contains the network policy launcher, PRoot overlay and pinned talloc sources. It contains no Rust runtime core or FFI cdylib. ARM64 helper builds enforce 16 KiB ELF page alignment and verify PT_LOAD segments before publication.

MainActivity demonstrates object creation only. RealRuntimeSmokeTest supplies verified assets and exercises guest execution; running neither has been claimed by this documentation build.

<a id="android-end-to-end"></a>

## Execute and release a Kotlin runtime

Pass a RuntimeConfig whose rootfs has already been staged by RootfsInstaller and whose nativeLibraryDir is Context.applicationInfo.nativeLibraryDir. This complete consumer function handles boot, a bounded command, result checks and cleanup. Run it from a coroutine off the UI thread; display typed integrity or policy errors instead of silently retrying a rejected request.

### Kotlin lifecycle function

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

Verification method: Environment validation (android-runtime-lifecycle)

<a id="android-recovery"></a>

## Packaging, recovery and device acceptance

Missing helper libraries or an ABI mismatch are packaging failures, before model or shell diagnosis. Verify the selected APK flavor contains its native libraries and extracted executable helpers. Rootfs installation errors belong to the archive/manifest/SBOM boundary; repair uses validated staging, and reset must preserve external user workspaces.

The SDK exposes background processes but Android can stop an application process. Host foreground-service and app lifecycle policy are separate from Linux task state. Keep resource paths stable across activity recreation, retain handles in an application-owned component, and inspect task snapshots rather than inferring task survival from a screen.

RealRuntimeSmokeTest exercises actual device requests and policy cases. It is the acceptance entry point, not evidence that this documentation edit ran instrumentation. Validate both arm64-v8a and x86_64 where supported, timeout descendants, binary stdio, terminal resize and denied network/memory policies on the artifact to be distributed.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/mobile-linux-runtime/tree/main/android/runtime/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxRuntime.kt)

- [mobile-linux-runtime/android/runtime/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxRuntime.kt](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/android/runtime/src/main/kotlin/io/lingxi/mobilelinux/MobileLinuxRuntime.kt) — Committed source (b65f8856d773d27ebf73b0cade907884d3080704)
- [mobile-linux-runtime/android/installer/src/main/kotlin/io/lingxi/mobilelinux/RootfsInstaller.kt](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/android/installer/src/main/kotlin/io/lingxi/mobilelinux/RootfsInstaller.kt) — Committed source (b65f8856d773d27ebf73b0cade907884d3080704)
- [mobile-linux-runtime/android/gradle-plugin/src/main/java/io/lingxi/mobilelinux/gradle/MobileLinuxPlugin.java](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/android/gradle-plugin/src/main/java/io/lingxi/mobilelinux/gradle/MobileLinuxPlugin.java) — Committed source (b65f8856d773d27ebf73b0cade907884d3080704)
- [mobile-linux-runtime/crates/mobile-linux-android/src/runtime/config.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/crates/mobile-linux-android/src/runtime/config.rs) — Committed source (b65f8856d773d27ebf73b0cade907884d3080704)
- [mobile-linux-runtime/crates/mobile-linux-ffi/src/runtime.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/crates/mobile-linux-ffi/src/runtime.rs) — Committed source (b65f8856d773d27ebf73b0cade907884d3080704)
- [mobile-linux-runtime/examples/android/app/src/androidTest/java/io/lingxi/mobilelinux/sample/RealRuntimeSmokeTest.kt](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/examples/android/app/src/androidTest/java/io/lingxi/mobilelinux/sample/RealRuntimeSmokeTest.kt) — Committed source (b65f8856d773d27ebf73b0cade907884d3080704)

- mobile-linux-runtime: HEAD b65f8856d773d27ebf73b0cade907884d3080704; Referenced input fingerprint sha256:de6f69b8671150c8682b938faea726c0360978b776acaa4c077b257b13792174; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
