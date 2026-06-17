package com.lingxi.code.voice.offline

import android.util.Log
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Facade over the offline sherpa-onnx STT/TTS for a chosen language pack. The
 * 心流 orb uses this when a pack is downloaded; otherwise it falls back to the
 * system SpeechRecognizer / TextToSpeech. Recognizers are heavy to build, so
 * they're loaded lazily and cached. Any native failure (missing .so / bad model)
 * is swallowed → the caller falls back to the system voice.
 */
object SherpaVoice {
    private const val TAG = "SherpaVoice"
    private val sttCache = mutableMapOf<String, SherpaStt?>()
    private val ttsCache = mutableMapOf<String, SherpaTts?>()
    private val mutex = Mutex()

    private fun sttEntry(lang: String): OfflineModelEntry? =
        OfflineModelCatalog.packFor(lang).firstOrNull { it.kind == ModelKind.Stt }

    private fun ttsEntry(lang: String): OfflineModelEntry? =
        OfflineModelCatalog.packFor(lang).firstOrNull { it.kind == ModelKind.Tts }

    /** The STT model for [lang] is downloaded and on disk. */
    fun sttReady(lang: String): Boolean = sttEntry(lang)?.let { VoiceModelDownloader.isReady(it) } == true

    /** The TTS model for [lang] is downloaded and on disk. */
    fun ttsReady(lang: String): Boolean = ttsEntry(lang)?.let { VoiceModelDownloader.isReady(it) } == true

    /** Record one utterance and transcribe it offline. Returns null if unavailable. */
    suspend fun transcribe(lang: String): String? {
        val entry = sttEntry(lang) ?: return null
        if (!VoiceModelDownloader.isReady(entry)) return null
        val rec = mutex.withLock {
            sttCache.getOrPut(entry.id) {
                runCatching { SherpaStt.load(entry, VoiceModelDownloader.modelDir(entry.id)) }
                    .onFailure { Log.w(TAG, "STT load failed for ${entry.id}: ${it.message}") }
                    .getOrNull()
            }
        } ?: return null
        return runCatching { rec.transcribeOnce() }.getOrNull()
    }

    /** Speak [text] offline. No-op if the pack is unavailable. */
    suspend fun speak(lang: String, text: String) {
        val entry = ttsEntry(lang) ?: return
        if (!VoiceModelDownloader.isReady(entry)) return
        val tts = mutex.withLock {
            ttsCache.getOrPut(entry.id) {
                runCatching { SherpaTts.load(entry, VoiceModelDownloader.modelDir(entry.id)) }
                    .onFailure { Log.w(TAG, "TTS load failed for ${entry.id}: ${it.message}") }
                    .getOrNull()
            }
        } ?: return
        runCatching { tts.speak(text) }
    }

    fun stopSpeak() { ttsCache.values.forEach { it?.stop() } }
}
