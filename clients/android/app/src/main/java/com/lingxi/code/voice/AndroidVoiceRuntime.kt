package com.lingxi.code.voice

import android.content.Context
import android.speech.SpeechRecognizer
import com.lingxi.code.bindings.SpeechFfiException
import com.lingxi.code.model.VoiceConfig
import com.lingxi.code.settings.VoiceSettingsRepository
import com.lingxi.code.settings.VoiceRecognitionBackend as SettingsRecognitionBackend
import com.lingxi.code.settings.VoiceSpeechBackend as SettingsSpeechBackend
import com.lingxi.code.settings.resolveVoiceExecutionRoute
import com.lingxi.code.settings.normalizeVoiceSelection
import com.lingxi.code.settings.parseSherpaVoiceSelection
import com.lingxi.code.voice.audio.AudioInput
import com.lingxi.code.voice.audio.RealtimeSpeechCallbacks
import com.lingxi.code.voice.audio.RealtimeSpeechSession
import com.lingxi.code.voice.audio.SttCapabilities
import com.lingxi.code.voice.audio.SttProvider
import com.lingxi.code.voice.audio.SttResult
import com.lingxi.code.voice.audio.SystemSpeechRecognizerStt
import com.lingxi.code.voice.audio.SystemTextToSpeechTts
import com.lingxi.code.voice.audio.TtsCapabilities
import com.lingxi.code.voice.audio.TtsProvider
import com.lingxi.code.voice.offline.SherpaVoice
import com.lingxi.code.voice.offline.VoiceModelDownloader
import com.lingxi.code.voice.offline.ModelKind
import com.lingxi.code.voice.offline.ModelState
import com.lingxi.code.voice.offline.OfflineModelCatalog
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import java.util.Locale

internal enum class RecognitionBackend {
    System,
    Sherpa,
}

internal enum class SpeechBackend {
    System,
    Sherpa,
}

internal data class SherpaVoiceSelection(
    val modelId: String,
    val voiceId: String,
)

internal data class RecognitionResolution(
    val backend: RecognitionBackend,
    val languageTag: String,
    val sherpaLanguage: String?,
    val available: Boolean,
)

internal data class SpeechResolution(
    val backend: SpeechBackend,
    val languageTag: String,
    val systemVoiceId: String?,
    val sherpaLanguage: String?,
    val sherpaVoice: SherpaVoiceSelection?,
    val speed: Float,
)

internal interface SherpaVoiceRuntimeBridge {
    suspend fun transcribe(language: String): String?
    fun openRealtimeSession(language: String, callbacks: RealtimeSpeechCallbacks): RealtimeSpeechSession
    suspend fun renderSpeech(
        language: String,
        selection: SherpaVoiceSelection?,
        text: String,
        speed: Float,
    ): Pair<ByteArray, Int>?
}

internal object DefaultSherpaVoiceRuntimeBridge : SherpaVoiceRuntimeBridge {
    override suspend fun transcribe(language: String): String? =
        SherpaVoice.transcribe(language)

    override fun openRealtimeSession(
        language: String,
        callbacks: RealtimeSpeechCallbacks,
    ): RealtimeSpeechSession = SherpaVoice.openRealtimeSession(language, callbacks)

    override suspend fun renderSpeech(
        language: String,
        selection: SherpaVoiceSelection?,
        text: String,
        speed: Float,
    ): Pair<ByteArray, Int>? = SherpaVoice.renderToPcm(
        language = language,
        modelId = selection?.modelId,
        voiceId = selection?.voiceId,
        text = text,
        speed = speed,
    )
}

internal class AndroidVoiceRuntime(
    context: Context,
    private val voiceSettingsRepository: VoiceSettingsRepository = VoiceSettingsRepository(context),
    private val systemSttFactory: (Context) -> SystemSpeechRecognizerStt = ::SystemSpeechRecognizerStt,
    private val systemTtsFactory: (Context) -> SystemTextToSpeechTts = ::SystemTextToSpeechTts,
    private val sherpaBridge: SherpaVoiceRuntimeBridge = DefaultSherpaVoiceRuntimeBridge,
) {
    private val appContext = context.applicationContext

    suspend fun transcribe(languageOverride: String? = null): SttResult {
        val resolution = resolveRecognition(languageOverride)
        return when (resolution.backend) {
            RecognitionBackend.System -> {
                if (!resolution.available) {
                    unavailableRecognitionResult(resolution)
                } else {
                    systemSttFactory(appContext).transcribe(
                        audio = AudioInput.Pcm16(ByteArray(0), 16_000),
                        language = resolution.languageTag,
                        keyProvider = { null },
                    )
                }
            }

            RecognitionBackend.Sherpa -> {
                val sherpaLanguage = resolution.sherpaLanguage
                if (!resolution.available || sherpaLanguage == null) {
                    unavailableRecognitionResult(resolution)
                } else {
                    val text = sherpaBridge.transcribe(sherpaLanguage)
                    if (text.isNullOrBlank()) {
                        SttResult.Err(
                            code = "no_speech",
                            message = "No speech recognized.",
                            retriable = false,
                        )
                    } else {
                        SttResult.Ok(
                            text = text,
                            language = resolution.languageTag,
                            confidence = null,
                        )
                    }
                }
            }
        }
    }

    fun openRealtimeSession(
        languageOverride: String? = null,
        callbacks: RealtimeSpeechCallbacks,
    ): RealtimeSpeechSession {
        val resolution = resolveRecognition(languageOverride)
        return when (resolution.backend) {
            RecognitionBackend.System -> {
                check(resolution.available) { unavailableRecognitionMessage(resolution) }
                systemSttFactory(appContext).openRealtimeSession(resolution.languageTag, callbacks)
            }

            RecognitionBackend.Sherpa -> {
                val sherpaLanguage = checkNotNull(resolution.sherpaLanguage) {
                    unavailableRecognitionMessage(resolution)
                }
                check(resolution.available) { unavailableRecognitionMessage(resolution) }
                sherpaBridge.openRealtimeSession(sherpaLanguage, callbacks)
            }
        }
    }

    suspend fun renderSpeech(
        text: String,
        languageOverride: String? = null,
        voiceOverride: String? = null,
        speedOverride: Float? = null,
    ): Pair<ByteArray, Int> {
        if (text.isBlank()) return ByteArray(0) to 22_050
        voiceOverride?.let(::normalizeVoiceSelection)
            ?.takeIf { it.startsWith(VoiceConfig.SHERPA_VOICE_PREFIX) }
            ?.let { selection ->
                val parsed = parseSherpaVoiceSelection(selection)
                val model = parsed?.modelId?.let(OfflineModelCatalog::byId)
                check(
                    parsed != null && model?.kind == ModelKind.Tts &&
                        model.voices.any { it.id == parsed.voiceId } &&
                        VoiceModelDownloader.states.value[model.id] is ModelState.Ready,
                ) { "Requested offline voice is unavailable." }
            }
        val resolution = resolveSpeech(
            explicitLanguage = languageOverride,
            explicitVoice = voiceOverride,
            explicitSpeed = speedOverride,
        )
        if (
            voiceOverride != null &&
            normalizeVoiceSelection(voiceOverride).startsWith(VoiceConfig.SHERPA_VOICE_PREFIX) &&
            resolution.backend != SpeechBackend.Sherpa
        ) {
            throw SpeechFfiException.Unavailable()
        }
        return when (resolution.backend) {
            SpeechBackend.System -> systemTtsFactory(appContext).renderToPcm(
                text = text,
                voice = resolution.systemVoiceId,
                speed = resolution.speed,
                language = resolution.languageTag,
                strictVoice = voiceOverride != null,
            )

            SpeechBackend.Sherpa -> {
                val sherpaLanguage = checkNotNull(resolution.sherpaLanguage)
                checkNotNull(
                    sherpaBridge.renderSpeech(
                        language = sherpaLanguage,
                        selection = resolution.sherpaVoice,
                        text = text,
                        speed = resolution.speed,
                    ),
                ) { "Selected offline voice could not render speech." }
            }
        }
    }

    private fun resolveRecognition(explicitLanguage: String?): RecognitionResolution {
        val preferences = voiceSettingsRepository.load()
        return resolveVoiceExecutionRoute(
            preferences = preferences,
            localeTag = deviceLanguageTag(),
            platformRecognizerAvailable = SpeechRecognizer.isRecognitionAvailable(appContext),
            modelStates = VoiceModelDownloader.states.value,
            languageOverride = explicitLanguage,
        ).let { route ->
            RecognitionResolution(
                backend = when (route.recognitionBackend) {
                    SettingsRecognitionBackend.Sherpa -> RecognitionBackend.Sherpa
                    SettingsRecognitionBackend.System -> RecognitionBackend.System
                    SettingsRecognitionBackend.Unavailable ->
                        if (preferences.recognitionMode == VoiceConfig.MODE_LOCAL_ONLY) {
                            RecognitionBackend.Sherpa
                        } else {
                            RecognitionBackend.System
                        }
                },
                languageTag = route.effectiveLanguage,
                sherpaLanguage = route.sherpaLanguage,
                available = route.recognitionBackend != SettingsRecognitionBackend.Unavailable,
            )
        }
    }

    private fun resolveSpeech(
        explicitLanguage: String?,
        explicitVoice: String?,
        explicitSpeed: Float?,
    ): SpeechResolution = resolveVoiceExecutionRoute(
        preferences = voiceSettingsRepository.load(),
        localeTag = deviceLanguageTag(),
        platformRecognizerAvailable = SpeechRecognizer.isRecognitionAvailable(appContext),
        modelStates = VoiceModelDownloader.states.value,
        languageOverride = explicitLanguage,
        voiceOverride = explicitVoice,
        rateOverride = explicitSpeed,
    ).let { route ->
        SpeechResolution(
            backend = when (route.speechBackend) {
                SettingsSpeechBackend.System -> SpeechBackend.System
                SettingsSpeechBackend.Sherpa -> SpeechBackend.Sherpa
            },
            languageTag = route.effectiveLanguage,
            systemVoiceId = route.systemVoiceId,
            sherpaLanguage = route.sherpaLanguage,
            sherpaVoice = route.sherpaModelId?.let { modelId ->
                route.sherpaVoiceId?.let { voiceId ->
                    SherpaVoiceSelection(modelId = modelId, voiceId = voiceId)
                }
            },
            speed = route.rate,
        )
    }

    private fun deviceLanguageTag(): String =
        Locale.getDefault().toLanguageTag().ifBlank { "en-US" }

    private fun unavailableRecognitionResult(resolution: RecognitionResolution): SttResult.Err =
        SttResult.Err(
            code = "no_provider_configured",
            message = unavailableRecognitionMessage(resolution),
            retriable = false,
        )

    private fun unavailableRecognitionMessage(resolution: RecognitionResolution): String =
        when (resolution.backend) {
            RecognitionBackend.System ->
                "Device has no SpeechRecognizer service installed."
            RecognitionBackend.Sherpa -> when (resolution.sherpaLanguage) {
                null -> "No offline speech model matches ${resolution.languageTag}."
                else -> "Offline speech model for ${resolution.sherpaLanguage} is unavailable."
            }
        }
}

internal class RuntimeSpeechRecognizerStt(
    private val runtime: AndroidVoiceRuntime,
) : SttProvider {
    override val id: String = "runtime"

    override val capabilities: SttCapabilities = SttCapabilities(
        streaming = false,
        languages = emptySet(),
        maxAudioSeconds = 60,
    )

    override suspend fun transcribe(
        audio: AudioInput,
        language: String?,
        keyProvider: suspend () -> String?,
    ): SttResult = runtime.transcribe(languageOverride = language)

    fun openRealtimeSession(
        language: String?,
        callbacks: RealtimeSpeechCallbacks,
    ): RealtimeSpeechSession = runtime.openRealtimeSession(language, callbacks)
}

internal class RuntimeTextToSpeechTts(
    private val runtime: AndroidVoiceRuntime,
) : TtsProvider {
    override val id: String = "runtime"

    override val capabilities: TtsCapabilities = TtsCapabilities(
        streaming = false,
        voices = emptyList(),
        sampleRateHz = 24_000,
    )

    override suspend fun synthesize(
        text: String,
        voice: String?,
        keyProvider: suspend () -> String?,
    ): Flow<ByteArray> = flow {
        val (pcm, _) = renderToPcm(text = text, voice = voice)
        if (pcm.isNotEmpty()) emit(pcm)
    }

    suspend fun renderToPcm(
        text: String,
        voice: String? = null,
        speed: Float? = null,
    ): Pair<ByteArray, Int> = runtime.renderSpeech(
        text = text,
        voiceOverride = voice,
        speedOverride = speed,
    )
}
