// M8-P12 skeleton — Kotlin impl of the Rust-declared `VoiceRecorder` callback
// interface. M9 backs it with MediaRecorder.
package com.lingxi.code

import com.lingxi.code.bindings.VoiceError
import com.lingxi.code.bindings.VoiceRecorder
import com.lingxi.code.bindings.VoiceRecording
import com.lingxi.code.bindings.VoiceRecordingOpts

class AndroidVoiceImpl : VoiceRecorder {
    override suspend fun startRecording(opts: VoiceRecordingOpts) {
        // TODO(M9): MediaRecorder at opts.sampleRateHz.
        throw VoiceError.Other("Unimplemented (M8 skeleton)")
    }

    override suspend fun stopRecording(): VoiceRecording {
        // TODO(M9): stop + return encoded audio.
        throw VoiceError.Other("Unimplemented (M8 skeleton)")
    }

    override suspend fun isRecording(): Boolean = false
}
