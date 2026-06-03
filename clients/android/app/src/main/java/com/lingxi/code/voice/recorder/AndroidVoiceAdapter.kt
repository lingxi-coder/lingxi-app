package com.lingxi.code.voice.recorder

import com.lingxi.code.bindings.AndroidVoice
import com.lingxi.code.bindings.VoiceFfiException
import com.lingxi.code.bindings.VoiceRecordingFfi

/**
 * Adapts the native [RecorderController] to the generated UniFFI callback
 * interface [AndroidVoice].
 *
 * The Rust seam (`apps/android-aar`) hands this foreign object to
 * `build_android_engine`, which bridges it onto `traits::VoiceRecorder` (via
 * `AndroidVoiceBridge`). We only map call shapes + result/error types here; the
 * real `MediaRecorder` session lives in [RecorderController]. The engine drives
 * start/stop/is_recording through `tool-voice`.
 *
 * Mirrors [com.lingxi.code.share.AndroidShareAdapter]: a thin wrapper that
 * translates the local controller's success/failure surface onto the flat FFI
 * types Rust fans back out onto the richer `traits::VoiceRecording` /
 * `traits::VoiceError`.
 */
class AndroidVoiceAdapter(
    private val controller: RecorderController = RecorderController,
) : AndroidVoice {

    override suspend fun startRecording(sampleRateHz: UInt, format: String) {
        try {
            controller.start(sampleRateHz.toInt())
        } catch (e: RecorderException) {
            throw e.failure.toFfi()
        } catch (e: VoiceFfiException) {
            throw e
        } catch (t: Throwable) {
            throw VoiceFfiException.Other(t.message ?: "recorder error")
        }
    }

    override suspend fun stopRecording(): VoiceRecordingFfi {
        val recording = try {
            controller.stop()
        } catch (e: RecorderException) {
            throw e.failure.toFfi()
        } catch (e: VoiceFfiException) {
            throw e
        } catch (t: Throwable) {
            throw VoiceFfiException.Other(t.message ?: "recorder error")
        }
        return VoiceRecordingFfi(
            audioBytes = recording.audioBytes,
            mimeType = recording.mimeType,
        )
    }

    override suspend fun isRecording(): Boolean = controller.isRecording()
}

/** Map the local [RecorderFailure] surface onto the flat FFI error enum. */
private fun RecorderFailure.toFfi(): VoiceFfiException = when (this) {
    is RecorderFailure.PermissionDenied -> VoiceFfiException.PermissionDenied()
    is RecorderFailure.NotRecording -> VoiceFfiException.NotRecording()
    is RecorderFailure.Other -> VoiceFfiException.Other(message)
}
