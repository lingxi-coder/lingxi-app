package com.lingxi.code.voice.offline

import com.lingxi.code.voice.audio.AudioDriverException
import com.lingxi.code.voice.audio.DeviceAudioError
import com.lingxi.code.voice.audio.DeviceAudioErrorKind
import com.lingxi.code.voice.audio.RealtimeSpeechCallbacks
import com.lingxi.code.voice.audio.RealtimeSpeechSession
import kotlinx.coroutines.CancellationException

/**
 * Facade over the offline sherpa-onnx STT/TTS for a chosen language pack. The
 * 心流 orb uses this when a pack is downloaded; otherwise it falls back to the
 * system SpeechRecognizer / TextToSpeech. Recognizers are heavy to build, so
 * they're loaded lazily and cached. Any native failure (missing .so / bad model)
 * is swallowed → the caller falls back to the system voice.
 */
object SherpaVoice {
    private val sttCache = mutableMapOf<String, SherpaStt>()
    private val ttsCache = mutableMapOf<String, SherpaTts>()
    private val cacheLock = Any()

    private fun sttEntry(lang: String): GeneratedOfflineModelEntry? =
        GeneratedVoiceModelCatalog.packFor(lang).firstOrNull { it.kind == GeneratedModelKind.Stt }

    private fun ttsEntry(lang: String, modelId: String? = null): GeneratedOfflineModelEntry? =
        modelId
            ?.let(GeneratedVoiceModelCatalog::byId)
            ?.takeIf { it.kind == GeneratedModelKind.Tts }
            ?: GeneratedVoiceModelCatalog.packFor(lang).firstOrNull { it.kind == GeneratedModelKind.Tts }

    /** The STT model for [lang] is downloaded and on disk. */
    fun sttReady(lang: String): Boolean = sttEntry(lang)?.let { VoiceModelDownloader.isReady(it) } == true

    /** The TTS model for [lang] is downloaded and on disk. */
    fun ttsReady(lang: String, modelId: String? = null): Boolean =
        ttsEntry(lang, modelId)?.let { VoiceModelDownloader.isReady(it) } == true

    /** Record one live utterance with the selected offline model; null means no speech was decoded. */
    suspend fun transcribe(lang: String, modelId: String? = null): String? {
        val entry = modelId?.let(GeneratedVoiceModelCatalog::byId)?.takeIf { it.kind == GeneratedModelKind.Stt }
            ?: sttEntry(lang)?.takeIf { modelId == null }
            ?: throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.ModelMissing, "selected offline speech model is unavailable"))
        if (entry.languages.none { it.equals(lang, ignoreCase = true) }) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.Unsupported, "selected offline model does not support the language"))
        }
        if (!VoiceModelDownloader.isReady(entry)) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.ModelMissing, "selected offline speech model is not installed"))
        }
        val recognizer = loadStt(entry)
        return recognizer.transcribeOnce()
    }

    suspend fun transcribePcm(lang: String, modelId: String, pcm: ByteArray, sampleRate: Int): String? {
        val entry = GeneratedVoiceModelCatalog.byId(modelId)?.takeIf { it.kind == GeneratedModelKind.Stt && lang in it.languages }
            ?: throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.ModelMissing, "Selected offline recognition model is unavailable."))
        if (!VoiceModelDownloader.isReady(entry)) throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.ModelMissing, "Selected offline recognition model is not installed."))
        return loadStt(entry).transcribePcm(pcm, sampleRate)
    }

    fun openRealtimeSession(
        lang: String,
        modelId: String? = null,
        callbacks: RealtimeSpeechCallbacks,
    ): RealtimeSpeechSession {
        val entry = modelId?.let(GeneratedVoiceModelCatalog::byId)?.takeIf { it.kind == GeneratedModelKind.Stt }
            ?: sttEntry(lang)?.takeIf { modelId == null }
            ?: throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.ModelMissing, "No offline speech model matches $lang"))
        if (entry.languages.none { it.equals(lang, ignoreCase = true) }) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.Unsupported, "offline speech model does not match $lang"))
        }
        if (!VoiceModelDownloader.isReady(entry)) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.ModelMissing, "offline speech model for $lang is unavailable"))
        }
        return loadStt(entry).openRealtimeSession(callbacks)
    }

    suspend fun renderToPcm(
        language: String,
        modelId: String? = null,
        voiceId: String? = null,
        text: String,
        speed: Float = 1.0f,
        maxPcmBytes: Int,
    ): Pair<ByteArray, Int>? {
        val entry = ttsEntry(language, modelId)
            ?: throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.ModelMissing, "selected offline speech model is unavailable"))
        if (!VoiceModelDownloader.isReady(entry)) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.ModelMissing, "selected offline speech model is not installed"))
        }
        val tts = loadTts(entry)
        val sid = entry.voices.indexOfFirst { it.id == voiceId }.takeIf { it != null && it >= 0 } ?: 0
        return tts.renderToPcm(text, sid = sid, speed = speed, maxPcmBytes = maxPcmBytes)
    }

    private fun loadStt(entry: GeneratedOfflineModelEntry): SherpaStt = synchronized(cacheLock) {
        sttCache[entry.id] ?: try {
            SherpaStt.load(entry, VoiceModelDownloader.modelDir(entry.id)).also { sttCache[entry.id] = it }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Throwable) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NativeFailure, error.message ?: "offline speech model could not be loaded"))
        }
    }

    private fun loadTts(entry: GeneratedOfflineModelEntry): SherpaTts = synchronized(cacheLock) {
        ttsCache[entry.id] ?: try {
            SherpaTts.load(entry, VoiceModelDownloader.modelDir(entry.id)).also { ttsCache[entry.id] = it }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Throwable) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.NativeFailure, error.message ?: "offline speech model could not be loaded"))
        }
    }
}
