package com.lingxi.code.voice.audio

import kotlinx.coroutines.flow.Flow

/**
 * Text-to-speech provider.
 *
 * Returns a stream of PCM16 chunks at [TtsCapabilities.sampleRateHz]. The
 * playback layer is responsible for AudioTrack handoff and stream lifecycle.
 */
interface TtsProvider {
    val id: String
    val capabilities: TtsCapabilities

    suspend fun synthesize(
        text: String,
        voice: String? = null,
        keyProvider: suspend () -> String?,
    ): Flow<ByteArray>
}

/** A non-streaming renderer that can report the format of the PCM it produced. */
interface TtsPcmRenderer {
    suspend fun renderPcm(text: String, voice: String?, maxPcmBytes: Int): Pair<ByteArray, Int>
}

data class TtsCapabilities(
    val streaming: Boolean,
    val voices: List<TtsVoice>,
    val sampleRateHz: Int = 24_000,
)

data class TtsVoice(
    val id: String,
    val displayName: String,
    val language: String,
)
