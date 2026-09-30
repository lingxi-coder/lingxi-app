package com.lingxi.code.voice

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.speech.SpeechRecognizer
import android.speech.tts.TextToSpeech
import androidx.core.content.ContextCompat
import com.lingxi.code.settings.AudioConfigurationRepository
import com.lingxi.code.settings.VersionedAudioConfiguration
import com.lingxi.code.voice.audio.AudioConfigurationV3
import com.lingxi.code.voice.audio.AudioOperationException
import com.lingxi.code.voice.audio.AudioOfflineModelAvailability
import com.lingxi.code.voice.audio.AudioProviderKind
import com.lingxi.code.voice.audio.AudioReadiness
import com.lingxi.code.voice.audio.AudioRouteRequest
import com.lingxi.code.voice.audio.AudioRouteResolution
import com.lingxi.code.voice.audio.AudioRouteStatus
import com.lingxi.code.voice.audio.AudioSource
import com.lingxi.code.voice.audio.AudioSpeechPreference
import com.lingxi.code.voice.audio.AudioVoiceSelection
import com.lingxi.code.voice.audio.AudioInput
import com.lingxi.code.voice.audio.AudioDriverException
import com.lingxi.code.voice.audio.DeviceAudioError
import com.lingxi.code.voice.audio.DeviceAudioErrorKind
import com.lingxi.code.voice.audio.RealtimeSpeechCallbacks
import com.lingxi.code.voice.audio.RealtimeSpeechSession
import com.lingxi.code.voice.audio.SttResult
import com.lingxi.code.voice.audio.SystemSpeechRecognizerStt
import com.lingxi.code.voice.audio.SystemTextToSpeechTts
import com.lingxi.code.voice.audio.resolveAudioLanguage
import com.lingxi.code.voice.audio.resolveAudioRoute
import com.lingxi.code.voice.offline.ModelKind
import com.lingxi.code.voice.offline.ModelState
import com.lingxi.code.voice.offline.OfflineModelCatalog
import com.lingxi.code.voice.offline.SherpaVoice
import com.lingxi.code.voice.offline.VoiceModelDownloader
import java.util.Locale

internal data class SherpaVoiceSelection(val modelId: String, val voiceId: String)

internal data class AudioOperationResolution(
    val route: AudioRouteResolution,
    val languageTag: String,
    val speed: Float,
)

internal data class SpeechVoiceOverrideResolution(
    val preference: AudioSpeechPreference,
    val voiceOverride: AudioVoiceSelection?,
)

internal interface SherpaVoiceRuntimeBridge {
    suspend fun transcribe(modelId: String, language: String): String?
    fun openRealtimeSession(modelId: String, language: String, callbacks: RealtimeSpeechCallbacks): RealtimeSpeechSession
    suspend fun renderSpeech(
        language: String,
        selection: SherpaVoiceSelection,
        text: String,
        speed: Float,
        maxPcmBytes: Int,
    ): Pair<ByteArray, Int>?
}

internal object DefaultSherpaVoiceRuntimeBridge : SherpaVoiceRuntimeBridge {
    override suspend fun transcribe(modelId: String, language: String): String? =
        SherpaVoice.transcribe(language, modelId)

    override fun openRealtimeSession(
        modelId: String,
        language: String,
        callbacks: RealtimeSpeechCallbacks,
    ): RealtimeSpeechSession = SherpaVoice.openRealtimeSession(language, modelId, callbacks)

    override suspend fun renderSpeech(
        language: String,
        selection: SherpaVoiceSelection,
        text: String,
        speed: Float,
        maxPcmBytes: Int,
    ): Pair<ByteArray, Int>? = SherpaVoice.renderToPcm(
        language = language,
        modelId = selection.modelId,
        voiceId = selection.voiceId,
        text = text,
        speed = speed,
        maxPcmBytes = maxPcmBytes,
    )
}

/** Resolves each operation from one immutable device-local v3 settings snapshot. */
internal class AndroidVoiceRuntime(
    context: Context,
    private val audioConfigurationRepository: AudioConfigurationRepository = AudioConfigurationRepository(context),
    private val systemSttFactory: (Context) -> SystemSpeechRecognizerStt = ::SystemSpeechRecognizerStt,
    private val systemTtsFactory: (Context) -> SystemTextToSpeechTts = ::SystemTextToSpeechTts,
    private val sherpaBridge: SherpaVoiceRuntimeBridge = DefaultSherpaVoiceRuntimeBridge,
) {
    private val appContext = context.applicationContext

    fun configurationSnapshot(): VersionedAudioConfiguration = audioConfigurationRepository.load().snapshot

    suspend fun transcribe(
        languageOverride: String? = null,
        configuration: AudioConfigurationV3? = null,
    ): SttResult {
        if (!hasMicrophonePermission()) {
            return SttResult.Err("permission_denied", "Microphone permission is not granted.", retriable = false)
        }
        val preferences = configuration ?: configurationSnapshot().configuration
        val resolution = resolveRecognition(preferences, languageOverride)
        return when (resolution.route.effective?.source) {
            AudioSource.SYSTEM -> systemSttFactory(appContext).transcribe(
                audio = AudioInput.Pcm16(ByteArray(0), 16_000),
                language = resolution.languageTag,
                keyProvider = { null },
            )
            AudioSource.OFFLINE -> {
                val modelId = resolution.route.effective.modelId
                if (modelId == null) return unavailableRecognitionResult(resolution.route)
                val text = sherpaBridge.transcribe(modelId, baseLanguage(resolution.languageTag))
                if (text.isNullOrBlank()) {
                    SttResult.Err("no_speech", "No speech recognized.", retriable = false)
                } else {
                    SttResult.Ok(text = text, language = resolution.languageTag, confidence = null)
                }
            }
            else -> unavailableRecognitionResult(resolution.route)
        }
    }

    fun openRealtimeSession(
        languageOverride: String? = null,
        callbacks: RealtimeSpeechCallbacks,
        configuration: AudioConfigurationV3? = null,
    ): RealtimeSpeechSession {
        if (!hasMicrophonePermission()) {
            throw AudioOperationException(DeviceAudioErrorKind.PermissionDenied, "Microphone permission is not granted.")
        }
        val preferences = configuration ?: configurationSnapshot().configuration
        val resolution = resolveRecognition(preferences, languageOverride)
        return when (resolution.route.effective?.source) {
            AudioSource.SYSTEM -> systemSttFactory(appContext).openRealtimeSession(resolution.languageTag, callbacks)
            AudioSource.OFFLINE -> {
                val modelId = resolution.route.effective.modelId
                    ?: throw routeException(resolution.route)
                sherpaBridge.openRealtimeSession(modelId, baseLanguage(resolution.languageTag), callbacks)
            }
            else -> throw routeException(resolution.route)
        }
    }

    suspend fun renderSpeech(
        text: String,
        languageOverride: String? = null,
        voiceOverride: String? = null,
        speedOverride: Float? = null,
        maxPcmBytes: Int,
        configuration: AudioConfigurationV3? = null,
    ): Pair<ByteArray, Int> {
        if (text.isBlank()) throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "speech text must not be blank")
        if (maxPcmBytes < 0) throw AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "audio payload limit must not be negative")
        val preferences = configuration ?: configurationSnapshot().configuration
        val resolution = resolveSpeech(preferences, languageOverride, voiceOverride, speedOverride)
        val route = resolution.route
        if (route.status != AudioRouteStatus.READY) throw routeException(route)
        val effective = checkNotNull(route.effective) { routeException(route) }
        val rendered = when (effective.source) {
            AudioSource.SYSTEM -> systemTtsFactory(appContext).renderToPcm(
                text = text,
                voice = effective.voiceId,
                speed = resolution.speed,
                language = resolution.languageTag,
                strictVoice = effective.voiceId != null,
                maxPcmBytes = maxPcmBytes,
            )
            AudioSource.OFFLINE -> {
                val modelId = checkNotNull(effective.modelId) { "offline speech model is missing" }
                val model = OfflineModelCatalog.byId(modelId)
                if (model?.kind != ModelKind.Tts || VoiceModelDownloader.states.value[modelId] !is ModelState.Ready) {
                    throw AudioOperationException(DeviceAudioErrorKind.ModelMissing, "selected offline speech model is unavailable")
                }
                val voiceId = effective.voiceId ?: model.voices.firstOrNull()?.id
                    ?: throw AudioOperationException(DeviceAudioErrorKind.VoiceMissing, "selected offline speech model has no available voice")
                sherpaBridge.renderSpeech(
                    language = baseLanguage(resolution.languageTag),
                    selection = SherpaVoiceSelection(modelId, voiceId),
                    text = text,
                    speed = resolution.speed,
                    maxPcmBytes = maxPcmBytes,
                ) ?: throw AudioOperationException(DeviceAudioErrorKind.NativeFailure, "offline speech renderer returned no result")
            }
            else -> throw routeException(route)
        }
        val (pcm, sampleRateHz) = rendered
        if (pcm.size > maxPcmBytes) {
            throw AudioDriverException(DeviceAudioError(DeviceAudioErrorKind.MediaTooLarge, "synthesized audio exceeds the payload limit"))
        }
        if (pcm.isEmpty()) throw AudioOperationException(DeviceAudioErrorKind.SynthesisFailed, "synthesis returned no audio")
        if (pcm.size % 2 != 0) throw AudioOperationException(DeviceAudioErrorKind.SynthesisFailed, "synthesis returned unaligned PCM16 audio")
        if (sampleRateHz !in 1..MAX_AUDIO_SAMPLE_RATE_HZ) {
            throw AudioOperationException(DeviceAudioErrorKind.SynthesisFailed, "synthesis returned an invalid sample rate")
        }
        return rendered
    }

    internal fun resolveRecognition(
        preferences: AudioConfigurationV3,
        languageOverride: String? = null,
        systemStatusOverride: AudioReadiness? = null,
    ): AudioOperationResolution {
        val language = resolveAudioLanguage(languageOverride ?: preferences.language, deviceLanguageTag())
        val route = resolveAudioRoute(
            AudioRouteRequest(
                kind = AudioProviderKind.RECOGNITION,
                preference = preferences.recognition,
                language = language,
                systemStatus = systemStatusOverride ?: if (SpeechRecognizer.isRecognitionAvailable(appContext)) {
                    AudioReadiness.AVAILABLE
                } else {
                    AudioReadiness.UNAVAILABLE
                },
                offlineModels = offlineModelAvailability(),
            ),
        )
        return AudioOperationResolution(route, language, preferences.rate.toFloat())
    }

    internal fun resolveSpeech(
        preferences: AudioConfigurationV3,
        languageOverride: String? = null,
        voiceOverride: String? = null,
        speedOverride: Float? = null,
        systemStatusOverride: AudioReadiness? = null,
    ): AudioOperationResolution {
        val language = resolveAudioLanguage(languageOverride ?: preferences.language, deviceLanguageTag())
        val voiceSelection = resolveSpeechVoiceOverride(voiceOverride, preferences)
        val route = resolveAudioRoute(
            AudioRouteRequest(
                kind = AudioProviderKind.SPEECH,
                preference = voiceSelection.preference,
                language = language,
                systemStatus = systemStatusOverride ?: if (systemTtsServiceAvailable()) {
                    AudioReadiness.AVAILABLE
                } else {
                    AudioReadiness.UNAVAILABLE
                },
                offlineModels = offlineModelAvailability(),
                voiceOverride = voiceSelection.voiceOverride,
            ),
        )
        val speed = (speedOverride?.toDouble() ?: preferences.rate).coerceIn(0.5, 2.0).toFloat()
        return AudioOperationResolution(route, language, speed)
    }

    private fun offlineModelAvailability(): List<AudioOfflineModelAvailability> =
        OfflineModelCatalog.all.map { model ->
            AudioOfflineModelAvailability(
                id = model.id,
                kind = when (model.kind) {
                    ModelKind.Stt -> AudioProviderKind.RECOGNITION
                    ModelKind.Tts -> AudioProviderKind.SPEECH
                },
                languages = model.languages.toList(),
                installed = VoiceModelDownloader.states.value[model.id] is ModelState.Ready,
                voiceIds = model.voices.map { it.id }.takeIf { it.isNotEmpty() },
            )
        }

    internal fun microphonePermissionGranted(): Boolean =
        ContextCompat.checkSelfPermission(appContext, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

    private fun systemTtsServiceAvailable(): Boolean = runCatching {
        appContext.packageManager.queryIntentServices(
            Intent(TextToSpeech.Engine.INTENT_ACTION_TTS_SERVICE),
            PackageManager.MATCH_DEFAULT_ONLY,
        ).isNotEmpty()
    }.getOrDefault(false)

    private fun hasMicrophonePermission(): Boolean = microphonePermissionGranted()

    private fun deviceLanguageTag(): String = Locale.getDefault().toLanguageTag().ifBlank { "en-US" }

    private fun baseLanguage(language: String): String = language.substringBefore('-')

    private companion object {
        const val MAX_AUDIO_SAMPLE_RATE_HZ = 768_000
    }

    private fun unavailableRecognitionResult(route: AudioRouteResolution): SttResult.Err = SttResult.Err(
        code = when (route.status) {
            AudioRouteStatus.INVALID_REQUEST -> "invalid_request"
            else -> if (route.reason.startsWith("offlineModel")) "model_missing" else "no_provider_configured"
        },
        message = unavailableRecognitionMessage(route),
        retriable = false,
    )

    private fun unavailableRecognitionMessage(route: AudioRouteResolution): String =
        when (route.reason) {
            "offlineModelUnknown" -> "The selected offline speech model is unknown."
            "offlineModelNotInstalled" -> "The selected offline speech model is not installed."
            "offlineModelUnsupportedLanguage", "noCompatibleOfflineModel" -> "No offline speech model matches the selected language."
            "systemUnavailable", "systemDenied" -> "Device has no SpeechRecognizer service installed."
            else -> "The selected speech recognition route is unavailable (${route.reason})."
        }

    private fun routeException(route: AudioRouteResolution): AudioOperationException = when {
        route.status == AudioRouteStatus.INVALID_REQUEST ->
            AudioOperationException(DeviceAudioErrorKind.InvalidRequest, "audio route is invalid (${route.reason})")
        route.requested.voice != null ->
            AudioOperationException(DeviceAudioErrorKind.VoiceMissing, "selected speech voice is unavailable (${route.reason})")
        route.requested.source == AudioSource.OFFLINE || route.reason == "noCompatibleOfflineModel" ->
            AudioOperationException(DeviceAudioErrorKind.ModelMissing, "selected offline speech model is unavailable (${route.reason})")
        else -> AudioOperationException(DeviceAudioErrorKind.Unavailable, "selected speech provider is unavailable (${route.reason})")
    }
}

internal fun resolveSpeechVoiceOverride(
    raw: String?,
    preferences: AudioConfigurationV3,
): SpeechVoiceOverrideResolution {
    val text = raw?.trim()?.takeIf { it.isNotEmpty() }
        ?: return SpeechVoiceOverrideResolution(preferences.speech, null)
    if (text.equals("default", ignoreCase = true) || text.equals("auto", ignoreCase = true)) {
        return SpeechVoiceOverrideResolution(
            preference = preferences.speech.copy(voice = null),
            voiceOverride = null,
        )
    }
    val selection = parseExplicitVoiceSelection(text) ?: when (preferences.speech.source) {
        AudioSource.OFFLINE -> AudioVoiceSelection(
            source = AudioSource.OFFLINE,
            id = text,
            modelId = preferences.speech.offlineModelId ?: preferences.speech.voice?.modelId,
        )
        else -> AudioVoiceSelection(AudioSource.SYSTEM, text)
    }
    return SpeechVoiceOverrideResolution(preferences.speech, selection)
}

/** Parses the explicit keys emitted by audio settings and Computer Use requests. */
internal fun parseExplicitVoiceSelection(raw: String): AudioVoiceSelection? {
    val text = raw.trim()
    if (text.startsWith("system:")) {
        return text.removePrefix("system:")
            .takeIf(String::isNotEmpty)
            ?.let { AudioVoiceSelection(AudioSource.SYSTEM, it) }
    }
    val payload = when {
        text.startsWith("offline:") -> text.removePrefix("offline:")
        text.startsWith("sherpa:") -> text.removePrefix("sherpa:")
        else -> return null
    }
    val split = payload.lastIndexOf(':')
    if (split <= 0 || split == payload.lastIndex) return null
    val modelId = payload.substring(0, split)
    val voiceId = payload.substring(split + 1)
    return AudioVoiceSelection(AudioSource.OFFLINE, voiceId, modelId)
}
