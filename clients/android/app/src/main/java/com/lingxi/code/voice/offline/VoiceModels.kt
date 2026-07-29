package com.lingxi.code.voice.offline

// Offline voice model catalog — ported from ~/lingxi/android
// (core/voiceclient/offline/{OfflineModelEntry,OfflineModelCatalog}.kt). These are
// sherpa-onnx models downloaded on demand; the user picks a LANGUAGE PACK in the
// setup wizard and the matching STT+TTS models are fetched. `runtimeParams`
// selects the matching native sherpa recognizer/synthesizer configuration.

enum class ModelKind { Stt, Tts }

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

/** Single source of truth for the offline voice models (sha256 = of the .tar.bz2). */
object OfflineModelCatalog {
    val all: List<OfflineModelEntry> = listOf(
        OfflineModelEntry(
            id = "sherpa.zipformer-zh-14m-mobile",
            kind = ModelKind.Stt,
            displayName = mapOf("zh" to "Zipformer 中文 14M", "en" to "Zipformer Chinese 14M"),
            languages = setOf("zh"),
            streaming = true,
            sampleRateHz = 16_000,
            approxSizeBytes = 54_344_380L,
            sha256 = "d394cab72b17f788b8b09ffc610f5f070e610fecf022eedca2eae9e38be4f20a",
            files = listOf(
                "encoder-epoch-99-avg-1.int8.onnx",
                "decoder-epoch-99-avg-1.onnx",
                "joiner-epoch-99-avg-1.int8.onnx",
                "tokens.txt",
            ),
            sourceUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-zh-14M-2023-02-23-mobile.tar.bz2",
            runtimeParams = SherpaRuntimeParams.Asr.OnlineTransducer(numThreads = 2, decoding = "greedy_search"),
            license = "Apache-2.0",
        ),
        OfflineModelEntry(
            id = "sherpa.moonshine-tiny-en",
            kind = ModelKind.Stt,
            displayName = mapOf("zh" to "Moonshine 英文 Tiny", "en" to "Moonshine Tiny (English)"),
            languages = setOf("en"),
            streaming = false,
            sampleRateHz = 16_000,
            approxSizeBytes = 107_600_538L,
            sha256 = "d5fe6ec4334fef36255b2a4010412cad4c007e33103fec62fb5d17cad88086f2",
            files = listOf(
                "preprocess.onnx", "encode.int8.onnx", "uncached_decode.int8.onnx",
                "cached_decode.int8.onnx", "tokens.txt",
            ),
            sourceUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-tiny-en-int8.tar.bz2",
            runtimeParams = SherpaRuntimeParams.Asr.OfflineMoonshine(numThreads = 2),
            license = "MIT",
        ),
        OfflineModelEntry(
            id = "sherpa.melo-zh-en",
            kind = ModelKind.Tts,
            displayName = mapOf("zh" to "MeloTTS 中英双语", "en" to "MeloTTS Chinese + English"),
            languages = setOf("zh", "en"),
            streaming = true,
            sampleRateHz = 44_100,
            approxSizeBytes = 167_006_755L,
            sha256 = "e58351ed7149f290a54534538badd4077cdbe6fddc964b24d0bee870415d1514",
            files = listOf(
                "model.int8.onnx", "tokens.txt", "lexicon.txt",
                "date.fst", "phone.fst", "number.fst",
            ),
            requiredDirectories = listOf("dict"),
            sourceUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-melo-tts-zh_en.tar.bz2",
            runtimeParams = SherpaRuntimeParams.Tts.Vits(numThreads = 2),
            voices = listOf(
                TtsVoiceEntry("melo-zh-en", "Melo 中英女声", "zh"),
            ),
            license = "MIT",
        ),
        OfflineModelEntry(
            id = "sherpa.kitten-nano-en",
            kind = ModelKind.Tts,
            displayName = mapOf("zh" to "Kitten Nano 英文", "en" to "Kitten Nano (English)"),
            languages = setOf("en"),
            streaming = true,
            sampleRateHz = 24_000,
            approxSizeBytes = 26_586_708L,
            sha256 = "0345a8a2f4a710cb8f7912c9a731ded8b3e1e69b33a871efa95c2e64651518fe",
            files = listOf("model.fp16.onnx", "voices.bin", "tokens.txt"),
            requiredDirectories = listOf("espeak-ng-data"),
            sourceUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kitten-nano-en-v0_2-fp16.tar.bz2",
            runtimeParams = SherpaRuntimeParams.Tts.Kitten(numThreads = 2),
            voices = listOf(
                TtsVoiceEntry("expr-voice-2-f", "Kitten Female", "en"),
            ),
            license = "Apache-2.0",
        ),
    )

    fun byId(id: String): OfflineModelEntry? = all.firstOrNull { it.id == id }

    /**
     * The STT+TTS model pair that backs a spoken language. Keep the packs small
     * enough for onboarding over a mobile connection.
     */
    fun packFor(language: String): List<OfflineModelEntry> = when (language) {
        "zh" -> listOf(byId("sherpa.zipformer-zh-14m-mobile")!!, byId("sherpa.melo-zh-en")!!)
        "en" -> listOf(byId("sherpa.moonshine-tiny-en")!!, byId("sherpa.kitten-nano-en")!!)
        else -> emptyList()
    }
}

/** One language pack the wizard offers. */
data class VoicePack(val language: String, val title: String, val subtitle: String) {
    val models: List<OfflineModelEntry> get() = OfflineModelCatalog.packFor(language)
    val totalBytes: Long get() = models.sumOf { it.approxSizeBytes }
}

val VOICE_PACKS = listOf(
    VoicePack("zh", "中文（普通话）", "Zipformer 14M 流式识别 + MeloTTS"),
    VoicePack("en", "English", "Moonshine Tiny ASR + Kitten TTS"),
)

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
