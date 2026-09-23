package com.lingxi.code.settings

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.speech.SpeechRecognizer
import android.speech.tts.TextToSpeech
import android.speech.tts.TextToSpeech.Engine
import android.speech.tts.Voice as AndroidTtsVoice
import androidx.core.content.ContextCompat
import com.lingxi.code.voice.audio.AudioConfigurationV3
import com.lingxi.code.voice.audio.AudioOfflineModelAvailability
import com.lingxi.code.voice.audio.AudioProviderKind
import com.lingxi.code.voice.audio.AudioReadiness
import com.lingxi.code.voice.audio.AudioRecognitionPreference
import com.lingxi.code.voice.audio.AudioRouteRequest
import com.lingxi.code.voice.audio.AudioRouteResolution
import com.lingxi.code.voice.audio.AudioRouteStatus
import com.lingxi.code.voice.audio.AudioSource
import com.lingxi.code.voice.audio.AudioSpeechPreference
import com.lingxi.code.voice.audio.AudioVoiceSelection
import com.lingxi.code.voice.audio.resolveAudioLanguage
import com.lingxi.code.voice.audio.resolveAudioRoute
import com.lingxi.code.voice.offline.ModelKind
import com.lingxi.code.voice.offline.ModelState
import com.lingxi.code.voice.offline.OfflineModelCatalog
import com.lingxi.code.voice.offline.VOICE_PACKS
import com.lingxi.code.voice.offline.VoiceModelDownloader
import com.lingxi.code.voice.offline.aggregatePackState
import kotlinx.coroutines.suspendCancellableCoroutine
import java.util.Locale
import kotlin.coroutines.resume

enum class VoicePermissionStatus { Granted, Denied, Unknown }
enum class VoiceOptionSource { System, Sherpa }

enum class VoiceBlockingIssue {
    MicrophonePermissionRequired,
    OfflineLanguageUnsupported,
    OfflineRecognitionModelRequired,
    AutomaticRecognizerUnavailable,
    RequestedVoiceUnavailable,
    PlaybackVoiceUnavailable,
}

data class VoiceOption(
    /** Stable key for settings selection. */
    val id: String,
    val label: String,
    val languageTag: String,
    val source: VoiceOptionSource,
    val familyId: String,
    val details: String? = null,
    val isDefault: Boolean = false,
    val networkRequired: Boolean = false,
    val missingData: Boolean = false,
    val selection: AudioVoiceSelection? = null,
)

data class VoiceModelPackStatus(
    val language: String,
    val title: String,
    val subtitle: String,
    val state: ModelState,
    val recognitionReady: Boolean,
    val speechReady: Boolean,
)

data class VoiceCapabilitySnapshot(
    val microphonePermission: VoicePermissionStatus = VoicePermissionStatus.Unknown,
    val platformRecognizerAvailable: Boolean = false,
    val requestedRecognitionBackend: String = AudioSource.AUTOMATIC.value,
    val effectiveRecognitionBackend: String = "unavailable",
    val effectiveLanguage: String = "auto",
    val voiceOptions: List<VoiceOption> = emptyList(),
    val requestedVoice: VoiceOption? = null,
    val effectiveVoice: VoiceOption? = null,
    val modelPackStates: List<VoiceModelPackStatus> = emptyList(),
    val blockingIssues: List<VoiceBlockingIssue> = emptyList(),
    val fallbackReason: String? = null,
    val recognitionSupported: Boolean = true,
    val speechSupported: Boolean = true,
    val recognitionReadiness: AudioReadiness = AudioReadiness.UNAVAILABLE,
    val speechReadiness: AudioReadiness = AudioReadiness.UNAVAILABLE,
    val recognitionReason: String? = null,
    val speechReason: String? = null,
    val recognitionRoute: AudioRouteResolution? = null,
    val speechRoute: AudioRouteResolution? = null,
) {
    val systemVoiceOptions: List<VoiceOption>
        get() = voiceOptions.filter { it.source == VoiceOptionSource.System }

    val offlineVoiceOptions: List<VoiceOption>
        get() = voiceOptions.filter { it.source == VoiceOptionSource.Sherpa }
}

data class VoicePlatformSnapshot(
    val localeTag: String,
    val microphonePermission: VoicePermissionStatus,
    val platformRecognizerAvailable: Boolean,
    val systemVoices: List<VoiceOption>,
    val defaultSystemVoiceId: String,
    val modelStates: Map<String, ModelState>,
)

object VoiceSettingsCapabilityResolver {
    fun resolve(preferences: AudioConfigurationV3, platform: VoicePlatformSnapshot): VoiceCapabilitySnapshot {
        val language = resolveAudioLanguage(preferences.language, platform.localeTag)
        val offlineModels = OfflineModelCatalog.all.map { model ->
            AudioOfflineModelAvailability(
                id = model.id,
                kind = when (model.kind) {
                    ModelKind.Stt -> AudioProviderKind.RECOGNITION
                    ModelKind.Tts -> AudioProviderKind.SPEECH
                },
                languages = model.languages.toList(),
                installed = platform.modelStates[model.id] is ModelState.Ready,
                voiceIds = model.voices.map { it.id }.takeIf { it.isNotEmpty() },
            )
        }
        val systemReadiness = if (platform.platformRecognizerAvailable) {
            AudioReadiness.AVAILABLE
        } else {
            AudioReadiness.UNAVAILABLE
        }
        val systemTtsAvailable = platform.systemVoices.isNotEmpty()
        val speechSystemReadiness = if (systemTtsAvailable) AudioReadiness.AVAILABLE else AudioReadiness.UNAVAILABLE
        val recognitionRoute = resolveAudioRoute(
            AudioRouteRequest(
                kind = AudioProviderKind.RECOGNITION,
                preference = preferences.recognition,
                language = language,
                systemStatus = systemReadiness,
                offlineModels = offlineModels,
            ),
        )
        val speechRoute = resolveAudioRoute(
            AudioRouteRequest(
                kind = AudioProviderKind.SPEECH,
                preference = preferences.speech,
                language = language,
                systemStatus = speechSystemReadiness,
                offlineModels = offlineModels,
                systemVoiceIds = platform.systemVoices.mapNotNull { it.selection?.id },
            ),
        )

        val voices = platform.systemVoices + OfflineModelCatalog.all
            .filter { it.kind == ModelKind.Tts }
            .flatMap { model ->
                model.voices.map { voice ->
                    VoiceOption(
                        id = AudioVoiceSelection(AudioSource.OFFLINE, voice.id, model.id).settingsKey(),
                        label = voice.displayName,
                        languageTag = canonicalVoiceLanguage(voice.language),
                        source = VoiceOptionSource.Sherpa,
                        familyId = model.id,
                        details = model.localizedDisplayName(if (languageBase(language) == "zh") "zh" else "en"),
                        selection = AudioVoiceSelection(AudioSource.OFFLINE, voice.id, model.id),
                    )
                }
            }
        val requestedVoice = preferences.speech.voice?.let { requested ->
            voices.firstOrNull { it.selection == requested } ?: unresolvedVoiceOption(requested, language)
        }
        val effectiveVoice = speechRoute.effective?.let { route ->
            if (route.source == AudioSource.SYSTEM) {
                val selectedId = route.voiceId ?: platform.defaultSystemVoiceId
                platform.systemVoices.firstOrNull { it.selection?.id == selectedId }
                    ?: platform.systemVoices.firstOrNull { it.isDefault }
            } else {
                voices.firstOrNull {
                    it.selection?.source == AudioSource.OFFLINE &&
                        it.selection.modelId == route.modelId &&
                        (route.voiceId == null || it.selection.id == route.voiceId)
                }
            }
        }

        val matchingLanguage = languageBase(language)
        val modelPacks = VOICE_PACKS.map { pack ->
            val packModels = pack.models
            val recognitionModels = packModels.filter { it.kind == ModelKind.Stt }
            val speechModels = packModels.filter { it.kind == ModelKind.Tts }
            VoiceModelPackStatus(
                language = pack.language,
                title = pack.title,
                subtitle = pack.subtitle,
                state = aggregatePackState(platform.modelStates, pack),
                recognitionReady = recognitionModels.isNotEmpty() && recognitionModels.all { platform.modelStates[it.id] is ModelState.Ready },
                speechReady = speechModels.isNotEmpty() && speechModels.all { platform.modelStates[it.id] is ModelState.Ready },
            )
        }
        val blockingIssues = buildList {
            if (platform.microphonePermission != VoicePermissionStatus.Granted) {
                add(VoiceBlockingIssue.MicrophonePermissionRequired)
            }
            when (recognitionRoute.reason) {
                "noCompatibleOfflineModel", "offlineModelUnsupportedLanguage" -> add(VoiceBlockingIssue.OfflineLanguageUnsupported)
                "offlineModelNotInstalled", "offlineModelUnknown" -> add(VoiceBlockingIssue.OfflineRecognitionModelRequired)
                "systemUnavailable" -> add(VoiceBlockingIssue.AutomaticRecognizerUnavailable)
            }
            if (preferences.speech.voice != null && speechRoute.effective == null) {
                add(VoiceBlockingIssue.RequestedVoiceUnavailable)
            }
            if (speechRoute.effective == null) add(VoiceBlockingIssue.PlaybackVoiceUnavailable)
        }
        val recognitionReadiness = when {
            platform.microphonePermission != VoicePermissionStatus.Granted -> AudioReadiness.PERMISSION_REQUIRED
            recognitionRoute.status == AudioRouteStatus.READY -> AudioReadiness.AVAILABLE
            recognitionRoute.status == AudioRouteStatus.PERMISSION_REQUIRED -> AudioReadiness.PERMISSION_REQUIRED
            else -> AudioReadiness.UNAVAILABLE
        }
        val speechReadiness = when (speechRoute.status) {
            AudioRouteStatus.READY -> AudioReadiness.AVAILABLE
            AudioRouteStatus.PERMISSION_REQUIRED -> AudioReadiness.PERMISSION_REQUIRED
            AudioRouteStatus.UNAVAILABLE, AudioRouteStatus.INVALID_REQUEST -> AudioReadiness.UNAVAILABLE
        }
        val recognizerBackend = recognitionRoute.effective?.source?.value ?: "unavailable"
        return VoiceCapabilitySnapshot(
            microphonePermission = platform.microphonePermission,
            platformRecognizerAvailable = platform.platformRecognizerAvailable,
            requestedRecognitionBackend = preferences.recognition.source.value,
            effectiveRecognitionBackend = recognizerBackend,
            effectiveLanguage = language,
            voiceOptions = voices,
            requestedVoice = requestedVoice,
            effectiveVoice = effectiveVoice,
            modelPackStates = modelPacks,
            blockingIssues = blockingIssues,
            fallbackReason = recognitionRoute.fallbackReason ?: speechRoute.fallbackReason,
            recognitionSupported = true,
            speechSupported = true,
            recognitionReadiness = recognitionReadiness,
            speechReadiness = speechReadiness,
            recognitionReason = recognitionRoute.reason,
            speechReason = speechRoute.reason,
            recognitionRoute = recognitionRoute,
            speechRoute = speechRoute,
        )
    }
}

suspend fun probeVoiceCapabilitySnapshot(
    context: Context,
    preferences: AudioConfigurationV3,
    modelStates: Map<String, ModelState> = VoiceModelDownloader.states.value,
): VoiceCapabilitySnapshot {
    val (systemVoices, defaultSystemVoiceId) = readSystemTtsSnapshot(context)
    return VoiceSettingsCapabilityResolver.resolve(
        preferences,
        VoicePlatformSnapshot(
            localeTag = currentLocaleTag(context),
            microphonePermission = microphonePermissionStatus(context),
            platformRecognizerAvailable = SpeechRecognizer.isRecognitionAvailable(context),
            systemVoices = systemVoices,
            defaultSystemVoiceId = defaultSystemVoiceId,
            modelStates = modelStates,
        ),
    )
}

private suspend fun readSystemTtsSnapshot(context: Context): Pair<List<VoiceOption>, String> {
    val tts = awaitTts(context)
    return try {
        val defaultVoiceName = tts.defaultVoice?.name
        val voices = tts.voices.orEmpty().map { voice ->
            val features = voice.features.orEmpty()
            val selection = AudioVoiceSelection(AudioSource.SYSTEM, voice.name)
            VoiceOption(
                id = selection.settingsKey(),
                label = voice.name,
                languageTag = voice.locale?.toLanguageTag().orEmpty().ifBlank { currentLocaleTag(context) },
                source = VoiceOptionSource.System,
                familyId = "system",
                details = systemVoiceDetails(voice, features),
                isDefault = voice.name == defaultVoiceName,
                networkRequired = voice.isNetworkConnectionRequired,
                missingData = features.contains(Engine.KEY_FEATURE_NOT_INSTALLED),
                selection = selection,
            )
        }.sortedBy { it.label.lowercase(Locale.US) }
        voices to (defaultVoiceName ?: "default")
    } finally {
        runCatching { tts.shutdown() }
    }
}

private fun systemVoiceDetails(voice: AndroidTtsVoice, features: Set<String>): String? = buildList {
    if (voice.isNetworkConnectionRequired) add("network")
    if (features.contains(Engine.KEY_FEATURE_NOT_INSTALLED)) add("missing data")
    voice.locale?.toLanguageTag()?.takeIf(String::isNotBlank)?.let(::add)
}.joinToString(" · ").ifBlank { null }

private suspend fun awaitTts(context: Context): TextToSpeech =
    suspendCancellableCoroutine { continuation ->
        lateinit var tts: TextToSpeech
        tts = TextToSpeech(context.applicationContext) { status ->
            if (continuation.isActive) continuation.resume(tts)
        }
        continuation.invokeOnCancellation { runCatching { tts.shutdown() } }
    }

private fun microphonePermissionStatus(context: Context): VoicePermissionStatus = when {
    ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED ->
        VoicePermissionStatus.Granted
    Build.VERSION.SDK_INT >= 23 -> VoicePermissionStatus.Denied
    else -> VoicePermissionStatus.Unknown
}

private fun currentLocaleTag(context: Context): String =
    context.resources.configuration.locales[0]?.toLanguageTag()
        ?: Locale.getDefault().toLanguageTag()

private fun canonicalVoiceLanguage(raw: String): String = when (raw.lowercase(Locale.US)) {
    "zh" -> "zh-CN"
    "en" -> "en-US"
    else -> raw
}

private fun unresolvedVoiceOption(selection: AudioVoiceSelection, language: String): VoiceOption =
    VoiceOption(
        id = selection.settingsKey(),
        label = selection.id,
        languageTag = language,
        source = if (selection.source == AudioSource.OFFLINE) VoiceOptionSource.Sherpa else VoiceOptionSource.System,
        familyId = selection.modelId ?: "system",
        details = "requested selection is unavailable",
        selection = selection,
    )

fun AudioVoiceSelection.settingsKey(): String = when (source) {
    AudioSource.SYSTEM -> "system:$id"
    AudioSource.OFFLINE -> "offline:${modelId.orEmpty()}:$id"
    else -> "${source.value}:$id"
}

internal fun languageBase(tag: String): String = tag.substringBefore('-').lowercase(Locale.US)

internal fun voiceLanguageMatches(languageTag: String, targetBase: String): Boolean =
    languageBase(languageTag.ifBlank { targetBase }) == targetBase
