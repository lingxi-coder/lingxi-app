package com.lingxi.code.voice.offline

// Offline voice model catalog — ported from ~/lingxi/android
// (core/voiceclient/offline/{OfflineModelEntry,OfflineModelCatalog}.kt). These are
// sherpa-onnx models downloaded on demand; the user picks a LANGUAGE PACK in the
// setup wizard and the matching STT+TTS models are fetched. `runtimeParams`
// selects the matching native sherpa recognizer/synthesizer configuration.

enum class ModelKind { Stt, Tts }

/**
 * [displayName] is provably dead: neither [OfflineModelEntry.voices] nor any
 * [TtsVoiceEntry] is read anywhere outside this file (no voice picker UI
 * consumes it), so its zh-Hans-literal fields (e.g. "Melo 中英女声") are never
 * rendered and are left untranslated.
 */
data class TtsVoiceEntry(val id: String, val displayName: String, val language: String)

sealed interface SherpaRuntimeParams {
    sealed interface Asr : SherpaRuntimeParams {
        data class OnlineTransducer(val numThreads: Int, val decoding: String) : Asr
        data class OfflineMoonshine(val numThreads: Int) : Asr
    }
    sealed interface Tts : SherpaRuntimeParams {
        data class Vits(val numThreads: Int) : Tts
        data class Kitten(val numThreads: Int) : Tts
    }
}

/**
 * [displayName]'s "zh" bucket (e.g. "Zipformer 中文 14M") stays the literal
 * zh-Hans copy: this file is a plain, engine-agnostic catalog with no
 * Android/Compose dependency (kept that way so
 * [com.lingxi.code.voice.offline.VoiceModelDownloaderTest] stays pure JVM).
 * The two call sites (both hardcode `language = "zh"`, in
 * `onboarding/SetupWizardOverlay.kt`'s `VoicePackRow`) resolve the real
 * localized text via an `id`-keyed `stringResource` lookup
 * (`voiceModelZhLabel`) defined there instead of here.
 */
data class OfflineModelEntry(
    val id: String,
    val kind: ModelKind,
    val displayName: Map<String, String>,
    val languages: Set<String>,
    val streaming: Boolean,
    val sampleRateHz: Int,
    val approxSizeBytes: Long,
    val sha256: String,
    val files: List<String>,
    val requiredDirectories: List<String> = emptyList(),
    val sourceUrl: String,
    val runtimeParams: SherpaRuntimeParams,
    val voices: List<TtsVoiceEntry> = emptyList(),
    val license: String,
) {
    fun localizedDisplayName(language: String): String =
        displayName[language] ?: displayName["en"] ?: displayName.values.first()
}

private fun GeneratedModelKind.toLegacyKind(): ModelKind = when (this) {
    GeneratedModelKind.Stt -> ModelKind.Stt
    GeneratedModelKind.Tts -> ModelKind.Tts
}

private fun GeneratedSherpaRuntimeParams.toLegacyRuntimeParams(): SherpaRuntimeParams = when (this) {
    is GeneratedSherpaRuntimeParams.Asr.OnlineTransducer ->
        SherpaRuntimeParams.Asr.OnlineTransducer(
            numThreads = numThreads,
            decoding = decoding,
        )
    is GeneratedSherpaRuntimeParams.Asr.OfflineMoonshine ->
        SherpaRuntimeParams.Asr.OfflineMoonshine(numThreads = numThreads)
    is GeneratedSherpaRuntimeParams.Tts.Vits ->
        SherpaRuntimeParams.Tts.Vits(numThreads = numThreads)
    is GeneratedSherpaRuntimeParams.Tts.Kitten ->
        SherpaRuntimeParams.Tts.Kitten(numThreads = numThreads)
}

private fun GeneratedTtsVoiceEntry.toLegacyVoice(): TtsVoiceEntry =
    TtsVoiceEntry(id = id, displayName = displayName, language = language)

private fun GeneratedOfflineModelEntry.toLegacyEntry(): OfflineModelEntry =
    OfflineModelEntry(
        id = id,
        kind = kind.toLegacyKind(),
        displayName = displayName,
        languages = languages,
        streaming = streaming,
        sampleRateHz = sampleRateHz,
        approxSizeBytes = approxSizeBytes,
        sha256 = sha256,
        files = files,
        requiredDirectories = requiredDirectories,
        sourceUrl = sourceUrl,
        runtimeParams = runtimeParams.toLegacyRuntimeParams(),
        voices = voices.map(GeneratedTtsVoiceEntry::toLegacyVoice),
        license = license,
    )

/** Single source of truth for the offline voice models (sha256 = of the .tar.bz2). */
object OfflineModelCatalog {
    val all: List<OfflineModelEntry> = GeneratedVoiceModelCatalog.all.map(
        GeneratedOfflineModelEntry::toLegacyEntry,
    )

    fun byId(id: String): OfflineModelEntry? = all.firstOrNull { it.id == id }

    /**
     * The STT+TTS model pair that backs a spoken language. Keep the packs small
     * enough for onboarding over a mobile connection.
     */
    fun packFor(language: String): List<OfflineModelEntry> =
        GeneratedVoiceModelCatalog.packFor(language).map(GeneratedOfflineModelEntry::toLegacyEntry)
}

/**
 * One language pack the wizard offers.
 *
 * The "zh" pack's [title]/[subtitle] stay the literal zh-Hans copy for the
 * same reason as [OfflineModelEntry.displayName] (this file has no
 * Android/Compose dependency); `SetupWizardOverlay.kt`'s `VoicePackRow`
 * resolves the real localized text via `VoicePack.localizedTitle()`/
 * `.localizedSubtitle()`, defined there. The "en" pack's copy has no Han
 * literal, so it renders as-is either way.
 */
data class VoicePack(val language: String, val title: String, val subtitle: String) {
    val models: List<OfflineModelEntry> get() = OfflineModelCatalog.packFor(language)
    val totalBytes: Long get() = models.sumOf { it.approxSizeBytes }
}

val VOICE_PACKS = GeneratedVoiceModelCatalog.packs.map {
    VoicePack(language = it.language, title = it.title, subtitle = it.subtitle)
}

/** Download lifecycle for one model (or an aggregate pack). */
sealed interface ModelState {
    data object NotInstalled : ModelState
    data class Queued(val bytes: Long, val total: Long) : ModelState
    data class Downloading(val bytes: Long, val total: Long) : ModelState
    data object Verifying : ModelState
    data object Extracting : ModelState
    data object Ready : ModelState
    data class Failed(val message: String) : ModelState
}

internal data class VoicePackProgress(
    val state: ModelState,
    val downloadedBytes: Long,
    val totalBytes: Long,
    val activeModel: OfflineModelEntry?,
    val activeModelIndex: Int,
    val nextModel: OfflineModelEntry?,
)

/** Combine the STT + TTS states into the progress shown for one language pack. */
internal fun aggregatePackState(
    states: Map<String, ModelState>,
    pack: VoicePack,
): ModelState = voicePackProgress(states, pack).state

internal fun voicePackProgress(
    states: Map<String, ModelState>,
    pack: VoicePack,
): VoicePackProgress {
    val modelStates = pack.models.map { states[it.id] ?: ModelState.NotInstalled }
    val total = pack.models.sumOf { model ->
        when (val state = states[model.id]) {
            is ModelState.Queued -> state.total.coerceAtLeast(model.approxSizeBytes)
            is ModelState.Downloading -> state.total.coerceAtLeast(model.approxSizeBytes)
            else -> model.approxSizeBytes
        }
    }
    val bytes = pack.models.sumOf { model ->
        when (val state = states[model.id]) {
            is ModelState.Queued -> state.bytes
            is ModelState.Downloading -> state.bytes
            is ModelState.Verifying, is ModelState.Extracting, is ModelState.Ready -> model.approxSizeBytes
            else -> 0L
        }
    }
    val activeIndex = modelStates.indexOfFirst { state ->
        state is ModelState.Downloading ||
            state is ModelState.Verifying ||
            state is ModelState.Extracting ||
            state is ModelState.Failed
    }.takeIf { it >= 0 } ?: modelStates.indexOfFirst { it is ModelState.Queued }
    val activeState = modelStates.getOrNull(activeIndex)
    val aggregateState = when {
        modelStates.all { it is ModelState.Ready } -> ModelState.Ready
        activeState is ModelState.Failed -> activeState
        activeState is ModelState.Verifying -> ModelState.Verifying
        activeState is ModelState.Extracting -> ModelState.Extracting
        activeState is ModelState.Downloading -> ModelState.Downloading(bytes, total.coerceAtLeast(bytes))
        activeState is ModelState.Queued -> ModelState.Queued(bytes, total.coerceAtLeast(bytes))
        else -> ModelState.NotInstalled
    }
    val nextIndex = if (activeIndex >= 0) {
        (activeIndex + 1 until pack.models.size).firstOrNull { modelStates[it] !is ModelState.Ready }
    } else {
        null
    }
    return VoicePackProgress(
        state = aggregateState,
        downloadedBytes = bytes,
        totalBytes = total.coerceAtLeast(bytes),
        activeModel = pack.models.getOrNull(activeIndex),
        activeModelIndex = activeIndex,
        nextModel = nextIndex?.let(pack.models::get),
    )
}
