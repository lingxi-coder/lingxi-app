// M8-P12 skeleton — public Kotlin entry point.
//
// `com.lingxi.code.bindings` is the UniFFI-generated package (MobileEngineHandle
// object, PlatformImpls record, buildMobileEngine() fn, and the CameraControl /
// VoiceRecorder / SharingService callback-interface interfaces Kotlin
// implements). Rust calls back into those Kotlin objects — the bidirectional
// UniFFI seam.
package com.lingxi.code

import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.bindings.MobileLinuxApiErrorFfi
import com.lingxi.code.bindings.MobileLinuxCapabilityFfi
import com.lingxi.code.bindings.MobileLinuxCommandRequestFfi
import com.lingxi.code.bindings.MobileLinuxCommandResultFfi
import com.lingxi.code.bindings.MobileLinuxMountSpecFfi
import com.lingxi.code.bindings.MobileLinuxProcessHandleFfi
import com.lingxi.code.bindings.MobileLinuxPtyOpenRequestFfi
import com.lingxi.code.bindings.MobileLinuxPtySessionHandleFfi
import com.lingxi.code.bindings.MobileLinuxPtySizeFfi
import com.lingxi.code.bindings.MobileLinuxStatusFfi
import com.lingxi.code.bindings.MobileLinuxTaskSnapshotFfi
import com.lingxi.code.bindings.AndroidMobileLinuxEventSink
import com.lingxi.code.bindings.AndroidMobileLinuxRuntimeHandle
import com.lingxi.code.bindings.PlatformImpls
import com.lingxi.code.bindings.AndroidMobileLinuxConfigFfi
import com.lingxi.code.bindings.androidMobileLinuxCapability
import com.lingxi.code.bindings.androidMobileLinuxReadEvents
import com.lingxi.code.bindings.buildMobileEngine
import com.lingxi.code.bindings.buildAndroidMobileLinuxRuntimeHandle
import com.lingxi.code.bindings.androidMobileLinuxBoot
import com.lingxi.code.bindings.androidMobileLinuxClosePty
import com.lingxi.code.bindings.androidMobileLinuxConfigureMounts
import com.lingxi.code.bindings.androidMobileLinuxKillProcess
import com.lingxi.code.bindings.androidMobileLinuxListTasks
import com.lingxi.code.bindings.androidMobileLinuxOpenPty
import com.lingxi.code.bindings.androidMobileLinuxResizePty
import com.lingxi.code.bindings.androidMobileLinuxRunCommand
import com.lingxi.code.bindings.androidMobileLinuxRunCommandStreaming
import com.lingxi.code.bindings.androidMobileLinuxShutdown
import com.lingxi.code.bindings.androidMobileLinuxSpawnBackground
import com.lingxi.code.bindings.androidMobileLinuxStatus
import com.lingxi.code.bindings.androidMobileLinuxTaskStatus
import com.lingxi.code.bindings.androidMobileLinuxRepairRootfs
import com.lingxi.code.bindings.androidMobileLinuxResetRootfs
import com.lingxi.code.bindings.androidMobileLinuxVerifyRootfs
import com.lingxi.code.bindings.androidMobileLinuxWritePty

/** Ergonomic wrapper over the UniFFI `MobileEngineHandle`. */
class LingxiCodeEngine private constructor(private val handle: MobileEngineHandle) {

    /** Number of builtin mobile skills the engine assembled. */
    val skillCount: UInt get() = handle.skillCount()

    companion object {
        /** Construct the engine, wiring the native Android capability impls. */
        fun make(
            appFilesRoot: String,
            mobileLinux: AndroidMobileLinuxConfigFfi? = null,
        ): LingxiCodeEngine {
            val impls = PlatformImpls(
                camera = AndroidCameraImpl(),
                voice = AndroidVoiceImpl(),
                share = AndroidShareImpl(),
                appFilesRoot = appFilesRoot,
                mobileLinux = mobileLinux,
            )
            return LingxiCodeEngine(buildMobileEngine(impls))
        }
    }
}

/** Mobile Linux runtime status/management wrapper for Android hosts. */
class LingxiMobileLinux private constructor(
    private val handle: AndroidMobileLinuxRuntimeHandle?,
) {
    companion object {
        @Throws(MobileLinuxApiErrorFfi::class)
        fun make(config: AndroidMobileLinuxConfigFfi): LingxiMobileLinux =
            LingxiMobileLinux(buildAndroidMobileLinuxRuntimeHandle(config))

        fun compatibility(): LingxiMobileLinux = LingxiMobileLinux(null)

        fun probe(config: AndroidMobileLinuxConfigFfi? = null): MobileLinuxCapabilityFfi =
            androidMobileLinuxCapability(config)

        fun status(config: AndroidMobileLinuxConfigFfi? = null): MobileLinuxStatusFfi =
            androidMobileLinuxStatus(config)

        fun verify(config: AndroidMobileLinuxConfigFfi? = null): MobileLinuxStatusFfi =
            androidMobileLinuxVerifyRootfs(config)

        fun repair(config: AndroidMobileLinuxConfigFfi? = null): MobileLinuxStatusFfi =
            androidMobileLinuxRepairRootfs(config)

        fun reset(config: AndroidMobileLinuxConfigFfi? = null): MobileLinuxStatusFfi =
            androidMobileLinuxResetRootfs(config)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun boot(config: AndroidMobileLinuxConfigFfi? = null): MobileLinuxStatusFfi =
            androidMobileLinuxBoot(config)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun shutdown(config: AndroidMobileLinuxConfigFfi? = null) =
            androidMobileLinuxShutdown(config)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun run(
            request: MobileLinuxCommandRequestFfi,
            config: AndroidMobileLinuxConfigFfi? = null,
        ): MobileLinuxCommandResultFfi = androidMobileLinuxRunCommand(config, request)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun runStreaming(
            request: MobileLinuxCommandRequestFfi,
            sink: AndroidMobileLinuxEventSink,
            config: AndroidMobileLinuxConfigFfi? = null,
        ): MobileLinuxCommandResultFfi = androidMobileLinuxRunCommandStreaming(config, request, sink)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun spawnBackground(
            request: MobileLinuxCommandRequestFfi,
            config: AndroidMobileLinuxConfigFfi? = null,
        ): MobileLinuxProcessHandleFfi = androidMobileLinuxSpawnBackground(config, request)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun kill(
            handle: MobileLinuxProcessHandleFfi,
            config: AndroidMobileLinuxConfigFfi? = null,
        ) = androidMobileLinuxKillProcess(config, handle)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun openPty(
            request: MobileLinuxPtyOpenRequestFfi,
            config: AndroidMobileLinuxConfigFfi? = null,
        ): MobileLinuxPtySessionHandleFfi = androidMobileLinuxOpenPty(config, request)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun writePty(
            handle: MobileLinuxPtySessionHandleFfi,
            input: ByteArray,
            config: AndroidMobileLinuxConfigFfi? = null,
        ) = androidMobileLinuxWritePty(config, handle, input)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun resizePty(
            handle: MobileLinuxPtySessionHandleFfi,
            size: MobileLinuxPtySizeFfi,
            config: AndroidMobileLinuxConfigFfi? = null,
        ) = androidMobileLinuxResizePty(config, handle, size)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun closePty(
            handle: MobileLinuxPtySessionHandleFfi,
            config: AndroidMobileLinuxConfigFfi? = null,
        ) = androidMobileLinuxClosePty(config, handle)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun configureMounts(
            mounts: List<MobileLinuxMountSpecFfi>,
            config: AndroidMobileLinuxConfigFfi? = null,
        ): MobileLinuxStatusFfi = androidMobileLinuxConfigureMounts(config, mounts)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun listTasks(config: AndroidMobileLinuxConfigFfi? = null): List<MobileLinuxTaskSnapshotFfi> =
            androidMobileLinuxListTasks(config)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun readEvents(
            afterSequence: ULong? = null,
            limit: UInt? = null,
            config: AndroidMobileLinuxConfigFfi? = null,
        ) = androidMobileLinuxReadEvents(config, afterSequence, limit)

        @Throws(MobileLinuxApiErrorFfi::class)
        fun taskStatus(
            taskId: String,
            config: AndroidMobileLinuxConfigFfi? = null,
        ) = androidMobileLinuxTaskStatus(config, taskId)
    }

    suspend fun capability(): MobileLinuxCapabilityFfi =
        requireHandle().capability()

    suspend fun status(): MobileLinuxStatusFfi =
        requireHandle().status()

    suspend fun boot(): MobileLinuxStatusFfi =
        requireHandle().boot()

    suspend fun shutdown() =
        requireHandle().shutdown()

    suspend fun verifyRootfs(): MobileLinuxStatusFfi =
        requireHandle().verifyRootfs()

    suspend fun repairRootfs(): MobileLinuxStatusFfi =
        requireHandle().repairRootfs()

    suspend fun resetRootfs(): MobileLinuxStatusFfi =
        requireHandle().resetRootfs()

    suspend fun run(request: MobileLinuxCommandRequestFfi): MobileLinuxCommandResultFfi =
        requireHandle().runCommand(request)

    suspend fun runStreaming(
        request: MobileLinuxCommandRequestFfi,
        sink: AndroidMobileLinuxEventSink,
    ): MobileLinuxCommandResultFfi = requireHandle().runCommandStreaming(request, sink)

    suspend fun spawnBackground(request: MobileLinuxCommandRequestFfi): MobileLinuxProcessHandleFfi =
        requireHandle().spawnBackground(request)

    suspend fun kill(handle: MobileLinuxProcessHandleFfi) =
        requireHandle().killProcess(handle)

    suspend fun openPty(request: MobileLinuxPtyOpenRequestFfi): MobileLinuxPtySessionHandleFfi =
        requireHandle().openPty(request)

    suspend fun writePty(handle: MobileLinuxPtySessionHandleFfi, input: ByteArray) =
        requireHandle().writePty(handle, input)

    suspend fun resizePty(handle: MobileLinuxPtySessionHandleFfi, size: MobileLinuxPtySizeFfi) =
        requireHandle().resizePty(handle, size)

    suspend fun closePty(handle: MobileLinuxPtySessionHandleFfi) =
        requireHandle().closePty(handle)

    suspend fun configureMounts(mounts: List<MobileLinuxMountSpecFfi>): MobileLinuxStatusFfi =
        requireHandle().configureMounts(mounts)

    suspend fun readEvents(afterSequence: ULong? = null, limit: UInt? = null) =
        requireHandle().readEvents(afterSequence, limit)

    suspend fun listTasks(): List<MobileLinuxTaskSnapshotFfi> =
        requireHandle().listTasks()

    suspend fun taskStatus(taskId: String) =
        requireHandle().taskStatus(taskId)

    private fun requireHandle(): AndroidMobileLinuxRuntimeHandle =
        handle ?: throw MobileLinuxApiErrorFfi.LegacySelected()
}
