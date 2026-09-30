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
import com.lingxi.code.bindings.buildMobileEngine
import com.lingxi.code.bindings.buildAndroidMobileLinuxRuntimeHandle

/** Ergonomic wrapper over the UniFFI `MobileEngineHandle`. */
class LingxiCodeEngine private constructor(private val handle: MobileEngineHandle) {

    /** Number of builtin mobile skills the engine assembled. */
    val skillCount: UInt get() = handle.skillCount()

    companion object {
        /** Construct the engine, wiring the native Android capability impls. */
        fun make(
            appFilesRoot: String,
            mobileLinux: AndroidMobileLinuxConfigFfi,
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
    private val handle: AndroidMobileLinuxRuntimeHandle,
) {
    companion object {
        @Throws(MobileLinuxApiErrorFfi::class)
        fun make(config: AndroidMobileLinuxConfigFfi): LingxiMobileLinux =
            LingxiMobileLinux(buildAndroidMobileLinuxRuntimeHandle(config))
    }

    suspend fun capability(): MobileLinuxCapabilityFfi =
        handle.capability()

    suspend fun status(): MobileLinuxStatusFfi =
        handle.status()

    suspend fun boot(): MobileLinuxStatusFfi =
        handle.boot()

    suspend fun shutdown() =
        handle.shutdown()

    suspend fun verifyRootfs(): MobileLinuxStatusFfi =
        handle.verifyRootfs()

    suspend fun repairRootfs(): MobileLinuxStatusFfi =
        handle.repairRootfs()

    suspend fun resetRootfs(): MobileLinuxStatusFfi =
        handle.resetRootfs()

    suspend fun run(request: MobileLinuxCommandRequestFfi): MobileLinuxCommandResultFfi =
        handle.runCommand(request)

    suspend fun runStreaming(
        request: MobileLinuxCommandRequestFfi,
        sink: AndroidMobileLinuxEventSink,
    ): MobileLinuxCommandResultFfi = handle.runCommandStreaming(request, sink)

    suspend fun spawnBackground(request: MobileLinuxCommandRequestFfi): MobileLinuxProcessHandleFfi =
        handle.spawnBackground(request)

    suspend fun kill(handle: MobileLinuxProcessHandleFfi) =
        handle.killProcess(handle)

    suspend fun openPty(request: MobileLinuxPtyOpenRequestFfi): MobileLinuxPtySessionHandleFfi =
        handle.openPty(request)

    suspend fun writePty(handle: MobileLinuxPtySessionHandleFfi, input: ByteArray) =
        handle.writePty(handle, input)

    suspend fun resizePty(handle: MobileLinuxPtySessionHandleFfi, size: MobileLinuxPtySizeFfi) =
        handle.resizePty(handle, size)

    suspend fun closePty(handle: MobileLinuxPtySessionHandleFfi) =
        handle.closePty(handle)

    suspend fun configureMounts(mounts: List<MobileLinuxMountSpecFfi>): MobileLinuxStatusFfi =
        handle.configureMounts(mounts)

    suspend fun readEvents(afterSequence: ULong? = null, limit: UInt? = null) =
        handle.readEvents(afterSequence, limit)

    suspend fun listTasks(): List<MobileLinuxTaskSnapshotFfi> =
        handle.listTasks()

    suspend fun taskStatus(taskId: String) =
        handle.taskStatus(taskId)

}
