package com.lingxi.code.voice.audio

import com.lingxi.code.bindings.AndroidStt
import com.lingxi.code.bindings.AndroidTts
import com.lingxi.code.bindings.SpeechFfiException
import com.lingxi.code.bindings.TtsAudioFfi

/**
 * Thin adapters that expose the extracted device-audio providers
 * ([SttProvider] / [TtsProvider]) through the generated UniFFI callback
 * interfaces ([AndroidStt] / [AndroidTts]).
 *
 * The Rust seam (`apps/android-aar`) hands these foreign objects to
 * `build_mobile_engine`, which bridges them onto `traits::SpeechToText` /
 * `traits::TextToSpeech`. We keep the local providers untouched — these
 * adapters only map call shapes and result/error types.
 *
 * Mapping notes:
 *  - The generated interfaces are mic-driven (no audio bytes in/out for STT),
 *    which matches [SystemSpeechRecognizerStt]'s live-mic behaviour: we pass an
 *    empty PCM payload that the system recognizer ignores.
 *  - No API keys are involved for the system providers, so `keyProvider`
 *    always yields `null`.
 *  - [SttResult.Err] / failed synthesis are surfaced as [SpeechFfiException],
 *    which Rust fans back out onto `SttError` / `TtsError`.
 */

/** Adapts an [SttProvider] (e.g. [SystemSpeechRecognizerStt]) to [AndroidStt]. */
class AndroidSttAdapter(private val provider: SttProvider) : AndroidStt {

    override suspend fun transcribe(language: String?): String {
        // System STT consumes the live mic and ignores the audio payload; the
        // Pcm16 channel keeps it off the `invalid_audio` (EncodedFile) branch.
        val result = provider.transcribe(
            audio = AudioInput.Pcm16(bytes = ByteArray(0), sampleRateHz = 16_000),
            language = language,
            keyProvider = { null },
        )
        return when (result) {
            is SttResult.Ok -> result.text
            is SttResult.Err -> throw result.toSpeechFfiException()
        }
    }
}

/** Adapts a [TtsProvider] (e.g. [SystemTextToSpeechTts]) to [AndroidTts]. */
class AndroidTtsAdapter(private val provider: TtsProvider) : AndroidTts {

    override suspend fun synthesize(text: String, voice: String?): TtsAudioFfi {
        // SystemTextToSpeechTts exposes renderToPcm() which already returns the
        // raw PCM body + actual sample rate from the WAV header. Prefer it so
        // we report the true rate rather than the streaming default; fall back
        // to draining the Flow for any other TtsProvider impl.
        return when (provider) {
            is SystemTextToSpeechTts -> {
                val (pcm, sampleRateHz) = provider.renderToPcm(text, voice = voice)
                TtsAudioFfi(pcm = pcm, sampleRateHz = sampleRateHz.toUInt())
            }
            else -> {
                val pcm = try {
                    val buffer = ArrayList<ByteArray>()
                    provider.synthesize(text = text, voice = voice, keyProvider = { null })
                        .collect { buffer.add(it) }
                    val total = buffer.sumOf { it.size }
                    val merged = ByteArray(total)
                    var offset = 0
                    for (chunk in buffer) {
                        chunk.copyInto(merged, offset)
                        offset += chunk.size
                    }
                    merged
                } catch (e: SpeechFfiException) {
                    throw e
                } catch (e: Throwable) {
                    throw SpeechFfiException.Other(e.message ?: "synthesis failed")
                }
                TtsAudioFfi(
                    pcm = pcm,
                    sampleRateHz = provider.capabilities.sampleRateHz.toUInt(),
                )
            }
        }
    }
}

/**
 * Map the local [SttResult.Err] code/retriable surface onto the flat FFI error
 * enum. Unknown codes degrade to [SpeechFfiException.Other].
 */
private fun SttResult.Err.toSpeechFfiException(): SpeechFfiException = when {
    code == "permission_denied" -> SpeechFfiException.PermissionDenied()
    code == "no_speech" -> SpeechFfiException.NoSpeech()
    code == "no_provider_configured" -> SpeechFfiException.Unavailable()
    retriable -> SpeechFfiException.Retriable(message)
    else -> SpeechFfiException.Other(message)
}
