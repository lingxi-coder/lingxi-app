package com.lingxi.code.voice.audio

import android.content.Context
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import kotlinx.coroutines.CancellableContinuation
import kotlinx.coroutines.suspendCancellableCoroutine
import java.io.File
import java.util.Locale
import java.util.UUID
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/**
 * Android system [TextToSpeech] renderer used by the app-scoped AudioService.
 *
 * `TextToSpeech.synthesizeToFile` writes a WAV; we strip the 44-byte
 * header and return the raw PCM body together with its actual WAV sample rate.
 *
 * Cancellation: the underlying engine call doesn't have a clean abort;
 * cancellation shuts down the temporary system TTS engine.
 */
class SystemTextToSpeechTts(private val context: Context) {

    private val defaultSampleRateHz = 22_050 // actual rate is read from the WAV header

    /** Returns (pcmBytes, sampleRateHz) so callers can drive AudioTrack correctly. */
    suspend fun renderToPcm(
        text: String,
        voice: String? = null,
        speed: Float = 1.0f,
        language: String? = null,
        strictVoice: Boolean = voice != null,
        maxPcmBytes: Int = Int.MAX_VALUE,
    ): Pair<ByteArray, Int> {
        if (text.isBlank()) return ByteArray(0) to defaultSampleRateHz
        val wavFile = File(context.cacheDir, "tts/sys-${UUID.randomUUID()}.wav").also {
            it.parentFile?.mkdirs()
        }
        val tts = awaitTtsInit()
        try {
            tts.setSpeechRate(speed.coerceIn(0.5f, 2.0f))
            val requestedVoice = voice
                ?.takeUnless { it == "default" }
                ?.let { voiceId -> tts.voices?.firstOrNull { it.name == voiceId } }
            val languageCompatibleVoice = requestedVoice?.takeIf {
                language == null || languageMatches(it.locale?.toLanguageTag(), language)
            }
            if (voice != null && voice != "default" && languageCompatibleVoice == null && strictVoice) {
                throw AudioOperationException(DeviceAudioErrorKind.VoiceMissing, "The selected system voice is unavailable.")
            }
            val effectiveVoice = languageCompatibleVoice
                ?: if (voice != null && voice != "default") {
                    tts.voices
                        ?.asSequence()
                        ?.filter { languageMatches(it.locale?.toLanguageTag(), language) }
                        ?.sortedWith(
                            compareBy<android.speech.tts.Voice> { it.isNetworkConnectionRequired }
                                .thenByDescending { it == tts.defaultVoice }
                                .thenBy { it.name.lowercase(Locale.US) },
                        )
                        ?.firstOrNull()
                } else {
                    null
                }
            if (effectiveVoice != null) {
                tts.voice = effectiveVoice
            } else if (language != null && (voice == null || voice == "default")) {
                val languageStatus = tts.setLanguage(Locale.forLanguageTag(language))
                if (languageStatus == TextToSpeech.LANG_MISSING_DATA || languageStatus == TextToSpeech.LANG_NOT_SUPPORTED) {
                    throw AudioOperationException(DeviceAudioErrorKind.Unavailable, "System speech does not support the selected language.")
                }
            }
            val utterance = "lingxi-${UUID.randomUUID()}"
            val status = synthesizeToFile(tts, text, utterance, wavFile)
            if (status != TextToSpeech.SUCCESS) {
                throw AudioOperationException(DeviceAudioErrorKind.SynthesisFailed, "system speech synthesis failed")
            }
            return readWav(wavFile, maxPcmBytes)
        } finally {
            runCatching { tts.shutdown() }
            runCatching { wavFile.delete() }
        }
    }

    private fun languageMatches(candidate: String?, target: String?): Boolean {
        if (candidate.isNullOrBlank() || target.isNullOrBlank()) return false
        return candidate.substringBefore('-').equals(target.substringBefore('-'), ignoreCase = true)
    }

    private suspend fun awaitTtsInit(): TextToSpeech =
        suspendCancellableCoroutine { cont ->
            lateinit var tts: TextToSpeech
            tts = TextToSpeech(context.applicationContext) { status ->
                if (status == TextToSpeech.SUCCESS && cont.isActive) {
                    cont.resume(tts)
                } else if (cont.isActive) {
                    runCatching { tts.shutdown() }
                    cont.resumeWithException(AudioOperationException(DeviceAudioErrorKind.Unavailable, "System speech service is unavailable."))
                }
            }
            cont.invokeOnCancellation { runCatching { tts.shutdown() } }
        }

    private suspend fun synthesizeToFile(
        tts: TextToSpeech,
        text: String,
        utterance: String,
        out: File,
    ): Int = suspendCancellableCoroutine { cont ->
        tts.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
            override fun onStart(utteranceId: String?) {}
            override fun onError(utteranceId: String?) { resume(cont, TextToSpeech.ERROR) }
            override fun onError(utteranceId: String?, errorCode: Int) { resume(cont, TextToSpeech.ERROR) }
            override fun onDone(utteranceId: String?) { resume(cont, TextToSpeech.SUCCESS) }
        })
        val rc = tts.synthesizeToFile(text, /* params= */ null, out, utterance)
        if (rc != TextToSpeech.SUCCESS) resume(cont, rc)
        cont.invokeOnCancellation { /* nothing useful to call */ }
    }

    private fun resume(cont: CancellableContinuation<Int>, value: Int) {
        if (cont.isActive) cont.resume(value)
    }

    private fun readWav(file: File, maxPcmBytes: Int): Pair<ByteArray, Int> =
        readPcm16Wav(file, maxPcmBytes)
}
