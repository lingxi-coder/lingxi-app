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
import com.lingxi.code.model.VoiceConfig
import com.lingxi.code.voice.offline.ModelKind
import com.lingxi.code.voice.offline.ModelState
import com.lingxi.code.voice.offline.OfflineModelCatalog
import com.lingxi.code.voice.offline.VOICE_PACKS
import com.lingxi.code.voice.offline.VoiceModelDownloader
import com.lingxi.code.voice.offline.aggregatePackState
import kotlinx.coroutines.suspendCancellableCoroutine
import java.util.Locale
import kotlin.coroutines.resume

enum class VoicePermissionStatus {
    Granted,
    Denied,
    Unknown,
}

enum class VoiceOptionSource {
    System,
    Sherpa,
}

enum class VoiceRecognitionBackend {
    System,
    Sherpa,
    Unavailable,
}

enum class VoiceSpeechBackend {
    System,
    Sherpa,
}

enum class VoiceBlockingIssue {
    MicrophonePermissionRequired,
    OfflineLanguageUnsupported,
    OfflineRecognitionModelRequired,
    AutomaticRecognizerUnavailable,
    RequestedVoiceUnavailable,
    PlaybackVoiceUnavailable,
}

data class VoiceOption(
    val id: String,
    val label: String,
    val languageTag: String,
    val source: VoiceOptionSource,
    val familyId: String,
    val details: String? = null,
    val isDefault: Boolean = false,
    val networkRequired: Boolean = false,
    val missingData: Boolean = false,
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
    val requestedRecognitionBackend: String = VoiceConfig.MODE_AUTOMATIC,
    val effectiveRecognitionBackend: String = "unavailable",
    val effectiveLanguage: String = VoiceConfig.LANGUAGE_AUTO,
    val voiceOptions: List<VoiceOption> = emptyList(),
    val requestedVoice: VoiceOption? = null,
    val effectiveVoice: VoiceOption? = null,
    val modelPackStates: List<VoiceModelPackStatus> = emptyList(),
    val blockingIssues: List<VoiceBlockingIssue> = emptyList(),
    val fallbackReason: String? = null,
) {
    val systemVoiceOptions: List<VoiceOption>
        get() = voiceOptions.filter { it.source == VoiceOptionSource.System }

    val offlineVoiceOptions: List<VoiceOption>
        get() = voiceOptions.filter { it.source == VoiceOptionSource.Sherpa }
}

data class VoiceExecutionRoute(
    val effectiveLanguage: String,
    val sherpaLanguage: String?,
    val recognitionBackend: VoiceRecognitionBackend,
    val speechBackend: VoiceSpeechBackend,
    val systemVoiceId: String?,
    val sherpaModelId: String?,
    val sherpaVoiceId: String?,
    val rate: Float,
    val fallbackReason: String? = null,
)

data class VoicePlatformSnapshot(
    val localeTag: String,
    val microphonePermission: VoicePermissionStatus,
    val platformRecognizerAvailable: Boolean,
    val systemVoices: List<VoiceOption>,
    val defaultSystemVoiceId: String,
    val modelStates: Map<String, ModelState>,
)

object VoiceSettingsCapabilityResolver {
    fun resolve(preferences: VoiceConfig, platform: VoicePlatformSnapshot): VoiceCapabilitySnapshot {
        val effectiveLanguage = resolveLanguage(preferences.language, platform.localeTag)
        val effectiveLanguageBase = languageBase(effectiveLanguage)
        val packStates = VOICE_PACKS.map { pack ->
            val aggregateState = aggregatePackState(platform.modelStates, pack)
            val recognitionModels = pack.models.filter { it.kind == ModelKind.Stt }
            val speechModels = pack.models.filter { it.kind == ModelKind.Tts }
            val recognitionReady = recognitionModels.isNotEmpty() && recognitionModels
                .all { model -> platform.modelStates[model.id] is ModelState.Ready }
            val speechReady = speechModels.isNotEmpty() && speechModels
                .all { model -> platform.modelStates[model.id] is ModelState.Ready }
            VoiceModelPackStatus(
                language = pack.language,
                title = pack.title,
                subtitle = pack.subtitle,
                state = aggregateState,
                recognitionReady = recognitionReady,
                speechReady = speechReady,
            )
        }
        val matchingPack = packStates.firstOrNull { it.language == effectiveLanguageBase }
        val offlineRecognitionReady = matchingPack?.recognitionReady == true
        val voiceOptions = buildList {
            addAll(
                platform.systemVoices.sortedWith(
                    compareByDescending<VoiceOption> { voiceLanguageMatches(it.languageTag, effectiveLanguageBase) }
                        .thenBy { it.networkRequired }
                        .thenByDescending { it.isDefault }
                        .thenBy { it.label.lowercase(Locale.US) },
                ),
            )
            addAll(
                OfflineModelCatalog.all
                    .filter {
                        it.kind == ModelKind.Tts &&
                            it.languages.any { language -> languageBase(language) == effectiveLanguageBase } &&
                            platform.modelStates[it.id] is ModelState.Ready
                    }
                    .flatMap { model ->
                        model.voices.map { voice ->
                            VoiceOption(
                                id = VoiceConfig.sherpaVoiceSelection(model.id, voice.id),
                                label = voice.displayName,
                                languageTag = canonicalVoiceLanguage(voice.language),
                                source = VoiceOptionSource.Sherpa,
                                familyId = model.id,
                                details = model.localizedDisplayName(if (effectiveLanguageBase == "zh") "zh" else "en"),
                            )
                        }
                    }
                    .sortedWith(
                        compareByDescending<VoiceOption> { voiceLanguageMatches(it.languageTag, effectiveLanguageBase) }
                            .thenBy { it.label.lowercase(Locale.US) },
                    ),
            )
        }

        val requestedVoice = voiceOptions.firstOrNull { it.id == preferences.voiceSelection }
            ?: unresolvedVoiceSelection(preferences.voiceSelection, effectiveLanguage)
        val effectiveVoice = resolveEffectiveVoice(
            requestedSelection = preferences.voiceSelection,
            requestedVoice = requestedVoice,
            allVoices = voiceOptions,
            defaultSystemVoiceId = platform.defaultSystemVoiceId,
            effectiveLanguage = effectiveLanguage,
        )

        val micGranted = platform.microphonePermission == VoicePermissionStatus.Granted
        val effectiveRecognitionBackend = when {
            !micGranted -> "unavailable"
            preferences.recognitionMode == VoiceConfig.MODE_LOCAL_ONLY && offlineRecognitionReady -> "sherpa"
            preferences.recognitionMode == VoiceConfig.MODE_LOCAL_ONLY -> "unavailable"
            platform.platformRecognizerAvailable -> "system"
            offlineRecognitionReady -> "sherpa"
            else -> "unavailable"
        }

        val blockingIssues = buildList {
            if (!micGranted) add(VoiceBlockingIssue.MicrophonePermissionRequired)
            if (preferences.recognitionMode == VoiceConfig.MODE_LOCAL_ONLY && matchingPack == null) {
                add(VoiceBlockingIssue.OfflineLanguageUnsupported)
            }
            if (preferences.recognitionMode == VoiceConfig.MODE_LOCAL_ONLY && matchingPack != null && !offlineRecognitionReady) {
                add(VoiceBlockingIssue.OfflineRecognitionModelRequired)
            }
            if (preferences.recognitionMode == VoiceConfig.MODE_AUTOMATIC && !platform.platformRecognizerAvailable && !offlineRecognitionReady) {
                add(VoiceBlockingIssue.AutomaticRecognizerUnavailable)
            }
            if (requestedVoice != null && effectiveVoice != null && requestedVoice.id != effectiveVoice.id) {
                add(VoiceBlockingIssue.RequestedVoiceUnavailable)
            }
            if (effectiveVoice == null) add(VoiceBlockingIssue.PlaybackVoiceUnavailable)
        }

        val fallbackReason = when {
            preferences.recognitionMode == VoiceConfig.MODE_LOCAL_ONLY && matchingPack == null ->
                "selected language has no offline Sherpa pack"
            preferences.recognitionMode == VoiceConfig.MODE_LOCAL_ONLY && !offlineRecognitionReady ->
                "selected offline recognition pack is not ready"
            preferences.recognitionMode == VoiceConfig.MODE_AUTOMATIC && !platform.platformRecognizerAvailable && offlineRecognitionReady ->
                "system recognizer unavailable, using verified offline model"
            requestedVoice != null && effectiveVoice != null && requestedVoice.id != effectiveVoice.id ->
                "requested voice unavailable, using nearest fallback"
            else -> null
        }

        return VoiceCapabilitySnapshot(
            microphonePermission = platform.microphonePermission,
            platformRecognizerAvailable = platform.platformRecognizerAvailable,
            requestedRecognitionBackend = preferences.recognitionMode,
            effectiveRecognitionBackend = effectiveRecognitionBackend,
            effectiveLanguage = effectiveLanguage,
            voiceOptions = voiceOptions,
            requestedVoice = requestedVoice,
            effectiveVoice = effectiveVoice,
            modelPackStates = packStates,
            blockingIssues = blockingIssues,
            fallbackReason = fallbackReason,
        )
    }
}

suspend fun probeVoiceCapabilitySnapshot(
    context: Context,
    preferences: VoiceConfig,
    modelStates: Map<String, ModelState> = VoiceModelDownloader.states.value,
): VoiceCapabilitySnapshot {
    val systemTtsSnapshot = readSystemTtsSnapshot(context)
    val platform = VoicePlatformSnapshot(
        localeTag = currentLocaleTag(context),
        microphonePermission = microphonePermissionStatus(context),
        platformRecognizerAvailable = SpeechRecognizer.isRecognitionAvailable(context),
        systemVoices = systemTtsSnapshot.first,
        defaultSystemVoiceId = systemTtsSnapshot.second,
        modelStates = modelStates,
    )
    return VoiceSettingsCapabilityResolver.resolve(preferences, platform)
}

private fun currentLocaleTag(context: Context): String =
    context.resources.configuration.locales.get(0)?.toLanguageTag()
        ?: Locale.getDefault().toLanguageTag()

private fun microphonePermissionStatus(context: Context): VoicePermissionStatus = when {
    ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED ->
        VoicePermissionStatus.Granted
    Build.VERSION.SDK_INT >= 23 -> VoicePermissionStatus.Denied
    else -> VoicePermissionStatus.Unknown
}

private suspend fun readSystemTtsSnapshot(context: Context): Pair<List<VoiceOption>, String> {
    val tts = awaitTts(context)
    return try {
        val defaultVoiceName = tts.defaultVoice?.name
        val voices = tts.voices
            ?.map { voice ->
                val features = voice.features.orEmpty()
                VoiceOption(
                    id = VoiceConfig.systemVoiceSelection(voice.name),
                    label = voice.name,
                    languageTag = voice.locale?.toLanguageTag().orEmpty().ifBlank { currentLocaleTag(context) },
                    source = VoiceOptionSource.System,
                    familyId = "system",
                    details = buildSystemVoiceDetails(voice, features),
                    isDefault = voice.name == defaultVoiceName,
                    networkRequired = voice.isNetworkConnectionRequired,
                    missingData = features.contains(Engine.KEY_FEATURE_NOT_INSTALLED),
                )
            }
            ?.sortedBy { it.label.lowercase(Locale.US) }
            .orEmpty()
        voices to (defaultVoiceName?.let(VoiceConfig::systemVoiceSelection) ?: VoiceConfig.DEFAULT_VOICE_SELECTION)
    } finally {
        runCatching { tts.shutdown() }
    }
}

private fun buildSystemVoiceDetails(voice: AndroidTtsVoice, features: Set<String>): String? {
    val detailParts = buildList {
        if (voice.isNetworkConnectionRequired) {
            add("network")
        }
        if (features.contains(Engine.KEY_FEATURE_NOT_INSTALLED)) {
            add("missing data")
        }
        add(voice.locale?.toLanguageTag().orEmpty())
    }.filter { it.isNotBlank() }
    return detailParts.joinToString(" · ").ifBlank { null }
}

private suspend fun awaitTts(context: Context): TextToSpeech =
    suspendCancellableCoroutine { continuation ->
        lateinit var tts: TextToSpeech
        tts = TextToSpeech(context.applicationContext) { status ->
            if (continuation.isActive) {
                continuation.resume(tts)
            }
        }
        continuation.invokeOnCancellation { runCatching { tts.shutdown() } }
    }

private fun resolveLanguage(selected: String, localeTag: String): String =
    normalizeLanguage(selected).let { language ->
        if (language == VoiceConfig.LANGUAGE_AUTO) localeTag else language
    }

private fun canonicalVoiceLanguage(raw: String): String = when (raw.lowercase(Locale.US)) {
    "zh" -> "zh-CN"
    "en" -> "en-US"
    else -> raw
}

private fun resolveEffectiveVoice(
    requestedSelection: String,
    requestedVoice: VoiceOption?,
    allVoices: List<VoiceOption>,
    defaultSystemVoiceId: String,
    effectiveLanguage: String,
): VoiceOption? {
    val requestedExact = allVoices.firstOrNull { it.id == requestedSelection }
    if (requestedExact != null && voiceSupportsLanguage(requestedExact, effectiveLanguage)) {
        return requestedExact
    }
    val requested = requestedVoice ?: return allVoices.firstOrNull { it.id == defaultSystemVoiceId } ?: allVoices.firstOrNull()
    return allVoices.firstOrNull {
        it.source == requested.source &&
            it.familyId == requested.familyId &&
            voiceSupportsLanguage(it, effectiveLanguage)
    } ?: allVoices.firstOrNull {
        it.source == requested.source && voiceSupportsLanguage(it, effectiveLanguage)
    } ?: VoiceOption(
        id = VoiceConfig.DEFAULT_VOICE_SELECTION,
        label = "System default",
        languageTag = effectiveLanguage,
        source = VoiceOptionSource.System,
        familyId = "system",
        details = "system default",
        isDefault = true,
    )
}

private fun voiceSupportsLanguage(option: VoiceOption, effectiveLanguage: String): Boolean {
    val target = languageBase(effectiveLanguage)
    return when (option.source) {
        VoiceOptionSource.System -> voiceLanguageMatches(option.languageTag, target)
        VoiceOptionSource.Sherpa -> (
            parseSherpaVoiceSelection(option.id)
                ?.modelId
                ?.let(OfflineModelCatalog::byId)
                ?.languages
                ?.any { languageBase(it) == target }
                == true
            )
    }
}

private fun unresolvedVoiceSelection(selection: String, fallbackLanguage: String): VoiceOption? {
    if (selection.isBlank()) return null
    return VoiceOption(
        id = selection,
        label = selection.substringAfterLast(':').ifBlank { selection },
        languageTag = fallbackLanguage,
        source = if (selection.startsWith(VoiceConfig.SHERPA_VOICE_PREFIX)) VoiceOptionSource.Sherpa else VoiceOptionSource.System,
        familyId = selection.substringAfter(':').substringBefore(':').ifBlank { "system" },
        details = "requested",
    )
}

internal fun languageBase(tag: String): String =
    tag.substringBefore('-').lowercase(Locale.US)

internal fun voiceLanguageMatches(languageTag: String, targetBase: String): Boolean =
    languageBase(languageTag.ifBlank { targetBase }) == targetBase

fun resolveVoiceExecutionRoute(
    preferences: VoiceConfig,
    localeTag: String,
    platformRecognizerAvailable: Boolean,
    modelStates: Map<String, ModelState>,
    languageOverride: String? = null,
    voiceOverride: String? = null,
    rateOverride: Float? = null,
): VoiceExecutionRoute {
    val effectiveLanguage = resolveLanguage(languageOverride ?: preferences.language, localeTag)
    val sherpaLanguage = sherpaPackLanguageFor(effectiveLanguage)
    val recognitionReady = sherpaLanguage?.let { language ->
        OfflineModelCatalog.packFor(language)
            .filter { it.kind == ModelKind.Stt }
            .let { models ->
                models.isNotEmpty() && models.all { modelStates[it.id] is ModelState.Ready }
            }
    } == true
    val recognitionBackend = when {
        preferences.recognitionMode == VoiceConfig.MODE_LOCAL_ONLY && recognitionReady ->
            VoiceRecognitionBackend.Sherpa
        preferences.recognitionMode == VoiceConfig.MODE_LOCAL_ONLY ->
            VoiceRecognitionBackend.Unavailable
        platformRecognizerAvailable ->
            VoiceRecognitionBackend.System
        recognitionReady ->
            VoiceRecognitionBackend.Sherpa
        else ->
            VoiceRecognitionBackend.Unavailable
    }
    val selectedVoice = normalizeVoiceSelection(voiceOverride ?: preferences.voiceSelection)
    val sherpaSelection = parseSherpaVoiceSelection(selectedVoice)
    val requestedSherpaModel = sherpaSelection?.modelId
    val requestedSherpaVoice = sherpaSelection?.voiceId
    val requestedModel = requestedSherpaModel?.let(OfflineModelCatalog::byId)
    val effectiveSherpaVoice = requestedModel?.let { model ->
        if (
            model.kind != ModelKind.Tts ||
            sherpaLanguage == null || !model.languages.contains(sherpaLanguage) ||
            modelStates[model.id] !is ModelState.Ready
        ) {
            null
        } else {
            model.voices.firstOrNull { it.id == requestedSherpaVoice }
                ?: model.voices.firstOrNull { languageBase(it.language) == sherpaLanguage }
                ?: model.voices.firstOrNull()
        }
    }
    val requestedSherpaReady = requestedModel?.let { model ->
        model.kind == ModelKind.Tts &&
            sherpaLanguage != null && model.languages.contains(sherpaLanguage) &&
            effectiveSherpaVoice != null && modelStates[model.id] is ModelState.Ready
    } == true
    val speechBackend = if (requestedSherpaModel != null && requestedSherpaReady) {
        VoiceSpeechBackend.Sherpa
    } else {
        VoiceSpeechBackend.System
    }
    val fallbackReason = when {
        preferences.recognitionMode == VoiceConfig.MODE_LOCAL_ONLY && sherpaLanguage == null ->
            "selected language has no offline Sherpa pack"
        preferences.recognitionMode == VoiceConfig.MODE_LOCAL_ONLY && !recognitionReady ->
            "selected offline recognition pack is not ready"
        requestedSherpaModel != null && requestedSherpaReady && effectiveSherpaVoice?.id != requestedSherpaVoice ->
            "requested offline voice is unavailable, using the nearest offline voice"
        requestedSherpaModel != null && !requestedSherpaReady ->
            "requested offline voice is unavailable, using system playback"
        !platformRecognizerAvailable && recognitionReady ->
            "system recognizer unavailable, using verified offline model"
        else ->
            null
    }
    return VoiceExecutionRoute(
        effectiveLanguage = effectiveLanguage,
        sherpaLanguage = sherpaLanguage,
        recognitionBackend = recognitionBackend,
        speechBackend = speechBackend,
        systemVoiceId = resolveSystemVoiceId(selectedVoice),
        sherpaModelId = requestedSherpaModel,
        sherpaVoiceId = effectiveSherpaVoice?.id,
        rate = (rateOverride ?: preferences.rate).coerceIn(0.5f, 2.0f),
        fallbackReason = fallbackReason,
    )
}

internal fun sherpaPackLanguageFor(languageTag: String): String? = when (languageBase(languageTag)) {
    "zh" -> "zh"
    "en" -> "en"
    else -> null
}

internal data class SherpaVoiceSelection(
    val modelId: String,
    val voiceId: String,
)

internal fun parseSherpaVoiceSelection(selection: String): SherpaVoiceSelection? {
    if (!selection.startsWith(VoiceConfig.SHERPA_VOICE_PREFIX)) return null
    val payload = selection.removePrefix(VoiceConfig.SHERPA_VOICE_PREFIX)
    val splitIndex = payload.lastIndexOf(':')
    if (splitIndex <= 0 || splitIndex >= payload.lastIndex) return null
    return SherpaVoiceSelection(
        modelId = payload.substring(0, splitIndex),
        voiceId = payload.substring(splitIndex + 1),
    )
}

internal fun resolveSystemVoiceId(selection: String): String? = when {
    selection == VoiceConfig.DEFAULT_VOICE_SELECTION -> null
    selection.startsWith(VoiceConfig.SYSTEM_VOICE_PREFIX) ->
        selection.removePrefix(VoiceConfig.SYSTEM_VOICE_PREFIX).takeUnless {
            it.isBlank() || it == VoiceConfig.DEFAULT_VOICE_ID
        }
    else -> null
}
