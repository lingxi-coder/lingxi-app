// M8-P12 skeleton — public Kotlin entry point.
//
// `com.lingxi.code.bindings` is the UniFFI-generated package (MobileEngineHandle
// object, PlatformImpls record, buildMobileEngine() fn, and the CameraControl /
// VoiceRecorder / SharingService callback-interface interfaces Kotlin
// implements). Rust calls back into those Kotlin objects — the bidirectional
// UniFFI seam.
package com.lingxi.code

import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.bindings.PlatformImpls
import com.lingxi.code.bindings.buildMobileEngine

/** Ergonomic wrapper over the UniFFI `MobileEngineHandle`. */
class LingxiCodeEngine private constructor(private val handle: MobileEngineHandle) {

    /** Number of builtin mobile skills the engine assembled. */
    val skillCount: UInt get() = handle.skillCount()

    companion object {
        /** Construct the engine, wiring the native Android capability impls. */
        fun make(appFilesRoot: String): LingxiCodeEngine {
            val impls = PlatformImpls(
                camera = AndroidCameraImpl(),
                voice = AndroidVoiceImpl(),
                share = AndroidShareImpl(),
                appFilesRoot = appFilesRoot,
            )
            return LingxiCodeEngine(buildMobileEngine(impls))
        }
    }
}
