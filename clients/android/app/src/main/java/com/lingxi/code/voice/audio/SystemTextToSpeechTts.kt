package com.lingxi.code.voice.audio

import android.content.Context
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import com.lingxi.code.bindings.SpeechFfiException
import kotlinx.coroutines.CancellableContinuation
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.suspendCancellableCoroutine
import java.io.File
import java.io.RandomAccessFile
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.Locale
import java.util.UUID
import kotlin.coroutines.resume

/**
 * Android system [TextToSpeech] wrapped as [TtsProvider].
 *
 * `TextToSpeech.synthesizeToFile` writes a WAV; we strip the 44-byte
 * header and emit the raw PCM body as a single Flow chunk. Sample rate is
 * read from the WAV header (most engines emit 22050 or 24000 Hz mono).
 *
 * Cancellation: the underlying engine call doesn't have a clean abort;
 * we let the synthesize complete and the caller's Flow consumer discard.
 * Acceptable because system TTS for chat-length replies finishes in
 * <500 ms.
 */
class SystemTextToSpeechTts(private val context: Context) : TtsProvider {

    override val id: String = "system"

    override val capabilities: TtsCapabilities = TtsCapabilities(
        streaming = false,
        voices = listOf(
            TtsVoice(id = "default", displayName = "System default", language = "auto"),
        ),
        sampleRateHz = 22_050, // most Android voices; actual rate comes from the WAV header
    )

    override suspend fun synthesize(
        text: String,
        voice: String?,
        keyProvider: suspend () -> String?,
    ): Flow<ByteArray> = flow {
        val (pcm, _) = renderToPcm(text, voice = voice, strictVoice = voice != null)
        if (pcm.isNotEmpty()) emit(pcm)
    }

    /** Returns (pcmBytes, sampleRateHz) so callers can drive AudioTrack correctly. */
    suspend fun renderToPcm(
        text: String,
        voice: String? = null,
        speed: Float = 1.0f,
        language: String? = null,
        strictVoice: Boolean = voice != null,
    ): Pair<ByteArray, Int> {
        if (text.isBlank()) return ByteArray(0) to capabilities.sampleRateHz
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
                throw SpeechFfiException.Unavailable()
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
                tts.language = Locale.forLanguageTag(language)
            }
            val utterance = "lingxi-${UUID.randomUUID()}"
            val status = synthesizeToFile(tts, text, utterance, wavFile)
            if (status != TextToSpeech.SUCCESS) return ByteArray(0) to capabilities.sampleRateHz
            return readWav(wavFile)
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
                    cont.resume(tts) // resume with a non-init TTS so synth fails gracefully
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

    /**
     * Strip a standard RIFF/WAV header to recover the raw PCM frames.
     * Works for the typical 44-byte canonical layout the Android engine
     * produces. Falls back to "whole file minus 44 bytes" if the header
     * isn't recognised.
     */
    private fun readWav(file: File): Pair<ByteArray, Int> {
        if (!file.exists() || file.length() < 44) return ByteArray(0) to capabilities.sampleRateHz
        RandomAccessFile(file, "r").use { raf ->
            val header = ByteArray(44)
            raf.readFully(header)
            val buf = ByteBuffer.wrap(header).order(ByteOrder.LITTLE_ENDIAN)
            val riff = String(header, 0, 4, Charsets.US_ASCII)
            val wave = String(header, 8, 4, Charsets.US_ASCII)
            val sampleRate = if (riff == "RIFF" && wave == "WAVE") buf.getInt(24) else capabilities.sampleRateHz
            val dataSizeFromHeader = if (riff == "RIFF" && wave == "WAVE") buf.getInt(40) else (raf.length() - 44).toInt()
            val body = ByteArray(dataSizeFromHeader.coerceAtMost((raf.length() - 44).toInt()).coerceAtLeast(0))
            raf.read(body)
            return body to sampleRate
        }
    }
}
