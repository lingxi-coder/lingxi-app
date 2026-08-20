package com.lingxi.code.voice.offline

import android.util.Log
import com.lingxi.code.voice.audio.RealtimeSpeechCallbacks
import com.lingxi.code.voice.audio.RealtimeSpeechSession

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
    private val cacheLock = Any()

    private fun sttEntry(lang: String): OfflineModelEntry? =
        OfflineModelCatalog.packFor(lang).firstOrNull { it.kind == ModelKind.Stt }

    private fun ttsEntry(lang: String, modelId: String? = null): OfflineModelEntry? =
        modelId
            ?.let(OfflineModelCatalog::byId)
            ?.takeIf { it.kind == ModelKind.Tts }
            ?: OfflineModelCatalog.packFor(lang).firstOrNull { it.kind == ModelKind.Tts }

    /** The STT model for [lang] is downloaded and on disk. */
    fun sttReady(lang: String): Boolean = sttEntry(lang)?.let { VoiceModelDownloader.isReady(it) } == true

    /** The TTS model for [lang] is downloaded and on disk. */
    fun ttsReady(lang: String, modelId: String? = null): Boolean =
        ttsEntry(lang, modelId)?.let { VoiceModelDownloader.isReady(it) } == true

    /** Record one utterance and transcribe it offline. Returns null if unavailable. */
    suspend fun transcribe(lang: String): String? {
        val entry = sttEntry(lang) ?: return null
        if (!VoiceModelDownloader.isReady(entry)) return null
        val rec = synchronized(cacheLock) {
            sttCache.getOrPut(entry.id) {
                runCatching { SherpaStt.load(entry, VoiceModelDownloader.modelDir(entry.id)) }
                    .onFailure { Log.w(TAG, "STT load failed for ${entry.id}: ${it.message}") }
                    .getOrNull()
            }
        } ?: return null
        return runCatching { rec.transcribeOnce() }.getOrNull()
    }

    fun openRealtimeSession(
        lang: String,
        callbacks: RealtimeSpeechCallbacks,
    ): RealtimeSpeechSession {
        val entry = checkNotNull(sttEntry(lang)) { "No offline speech model matches $lang" }
        check(VoiceModelDownloader.isReady(entry)) { "Offline speech model for $lang is unavailable." }
        val recognizer = synchronized(cacheLock) {
            sttCache.getOrPut(entry.id) {
                runCatching { SherpaStt.load(entry, VoiceModelDownloader.modelDir(entry.id)) }
                    .onFailure { Log.w(TAG, "STT load failed for ${entry.id}: ${it.message}") }
                    .getOrNull()
            }
        } ?: error("Offline speech model for $lang is unavailable.")
        return checkNotNull(recognizer) {
            "Offline speech model for $lang is unavailable."
        }.openRealtimeSession(callbacks)
    }

    suspend fun renderToPcm(
        language: String,
        modelId: String? = null,
        voiceId: String? = null,
        text: String,
        speed: Float = 1.0f,
    ): Pair<ByteArray, Int>? {
        val entry = ttsEntry(language, modelId) ?: return null
        if (!VoiceModelDownloader.isReady(entry)) return null
        val tts = synchronized(cacheLock) {
            ttsCache.getOrPut(entry.id) {
                runCatching { SherpaTts.load(entry, VoiceModelDownloader.modelDir(entry.id)) }
                    .onFailure { Log.w(TAG, "TTS load failed for ${entry.id}: ${it.message}") }
                    .getOrNull()
            }
        } ?: return null
        val sid = entry.voices.indexOfFirst { it.id == voiceId }.takeIf { it != null && it >= 0 } ?: 0
        return runCatching { tts.renderToPcm(text, sid = sid, speed = speed) }.getOrNull()
    }

    /** Speak [text] offline. No-op if the pack is unavailable. */
    suspend fun speak(lang: String, text: String) {
        val entry = ttsEntry(lang) ?: return
        if (!VoiceModelDownloader.isReady(entry)) return
        val tts = synchronized(cacheLock) {
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
