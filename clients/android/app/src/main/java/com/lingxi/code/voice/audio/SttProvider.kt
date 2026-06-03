package com.lingxi.code.voice.audio

/**
 * Speech-to-text provider.
 *
 * Implementations MUST honor coroutine cancellation by propagating
 * [kotlinx.coroutines.CancellationException]. Transport errors should be
 * mapped into [SttResult.Err] rather than thrown.
 *
 * Extracted from the LingXi Android voice subsystem (`core:voiceclient`),
 * stripped of the Koin/agent framework — a self-contained device-audio seam
 * the thin client wires directly.
 */
interface SttProvider {
    /** Stable id — "openai-whisper", "system", … */
    val id: String

    val capabilities: SttCapabilities

    /**
     * Transcribe [audio] using the active provider. [keyProvider] is invoked on
     * each call so secrets are not retained between calls (least-privilege).
     */
    suspend fun transcribe(
        audio: AudioInput,
        language: String? = null,
        keyProvider: suspend () -> String?,
    ): SttResult
}

/**
 * Audio input handle. [Pcm16] is the raw-frame channel used by the talk-mode
 * assembly path; [EncodedFile] is the file-based channel used by the chat
 * voice-message → STT path.
 */
sealed interface AudioInput {
    /** 16-bit signed PCM, little-endian, mono, [sampleRateHz] Hz. */
    data class Pcm16(val bytes: ByteArray, val sampleRateHz: Int) : AudioInput

    /**
     * A complete encoded audio file. [mime] is an IANA media type such as
     * `audio/mp4` (m4a/aac), `audio/mpeg` (mp3), or `audio/wav`. Providers
     * that don't accept the given mime should map to
     * [SttResult.Err] with code `invalid_audio`.
     *
     * [filename] is a display hint for providers that require one in their
     * multipart payload (Whisper does).
     */
    data class EncodedFile(
        val bytes: ByteArray,
        val mime: String,
        val filename: String = "audio",
    ) : AudioInput
}

data class SttCapabilities(
    /** Streaming partial transcripts supported. */
    val streaming: Boolean,
    /** BCP-47 codes the provider accepts; empty = unrestricted. */
    val languages: Set<String>,
    /** Hard upper-bound on a single transcription request. */
    val maxAudioSeconds: Int,
)

sealed interface SttResult {
    data class Ok(val text: String, val language: String?, val confidence: Float?) : SttResult
    data class Err(val code: String, val message: String, val retriable: Boolean) : SttResult
}
