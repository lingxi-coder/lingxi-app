package com.lingxi.code.voice.offline

// Offline voice model catalog — ported from ~/lingxi/android
// (core/voiceclient/offline/{OfflineModelEntry,OfflineModelCatalog}.kt). These are
// sherpa-onnx models downloaded on demand; the user picks a LANGUAGE PACK in the
// setup wizard and the matching STT+TTS models are fetched. `runtimeParams` is
// carried verbatim for the (later) native sherpa runtime; the download path only
// needs id/url/sha256/files/size.

enum class ModelKind { Stt, Tts }

data class TtsVoiceEntry(val id: String, val displayName: String, val language: String)

sealed interface SherpaRuntimeParams {
    sealed interface Asr : SherpaRuntimeParams {
        data class OnlineParaformer(val numThreads: Int, val decoding: String) : Asr
        data class OfflineMoonshine(val numThreads: Int) : Asr
    }
    sealed interface Tts : SherpaRuntimeParams {
        data class Kokoro(val numThreads: Int) : Tts
        data class Matcha(val numThreads: Int) : Tts
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
            id = "sherpa.paraformer-zh-en-stream",
            kind = ModelKind.Stt,
            displayName = mapOf("zh" to "Paraformer 中英流式", "en" to "Paraformer zh+en streaming"),
            languages = setOf("zh", "en"),
            streaming = true,
            sampleRateHz = 16_000,
            approxSizeBytes = 125L * 1024 * 1024,
            sha256 = "f8f9d727efc9eb4853589717974c80d840f3f2240b5ff78ab6c41ed2ac242e5d",
            files = listOf("encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"),
            sourceUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-paraformer-bilingual-zh-en.tar.bz2",
            runtimeParams = SherpaRuntimeParams.Asr.OnlineParaformer(numThreads = 2, decoding = "greedy_search"),
            license = "Apache-2.0",
        ),
        OfflineModelEntry(
            id = "sherpa.moonshine-base-en",
            kind = ModelKind.Stt,
            displayName = mapOf("zh" to "Moonshine 英文 Base", "en" to "Moonshine Base (English)"),
            languages = setOf("en"),
            streaming = false,
            sampleRateHz = 16_000,
            approxSizeBytes = 135L * 1024 * 1024,
            sha256 = "c2254d0c2055fbd81d23db63e6f2120dcc365473bb632f7559ebf18270b278da",
            files = listOf(
                "preprocess.onnx", "encode.int8.onnx", "uncached_decode.int8.onnx",
                "cached_decode.int8.onnx", "tokens.txt",
            ),
            sourceUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-base-en-int8.tar.bz2",
            runtimeParams = SherpaRuntimeParams.Asr.OfflineMoonshine(numThreads = 2),
            license = "MIT",
        ),
        OfflineModelEntry(
            id = "sherpa.kokoro-multi-zh-en",
            kind = ModelKind.Tts,
            displayName = mapOf("zh" to "Kokoro 中英多语", "en" to "Kokoro multi-lingual"),
            languages = setOf("zh", "en"),
            streaming = true,
            sampleRateHz = 24_000,
            approxSizeBytes = 350L * 1024 * 1024,
            sha256 = "c133d26353d776da730870dac7da07dbfc9a5e3bc80cc5e8e83ab6e823be7046",
            files = listOf("model.onnx", "voices.bin", "tokens.txt"),
            sourceUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_0.tar.bz2",
            runtimeParams = SherpaRuntimeParams.Tts.Kokoro(numThreads = 2),
            voices = listOf(
                TtsVoiceEntry("zf_xiaobei", "晓贝 (女)", "zh"),
                TtsVoiceEntry("zm_yunjian", "云健 (男)", "zh"),
                TtsVoiceEntry("af_heart", "Heart (F)", "en"),
                TtsVoiceEntry("am_michael", "Michael (M)", "en"),
            ),
            license = "Apache-2.0",
        ),
        OfflineModelEntry(
            id = "sherpa.matcha-en",
            kind = ModelKind.Tts,
            displayName = mapOf("zh" to "Matcha-TTS 英文", "en" to "Matcha-TTS (English)"),
            languages = setOf("en"),
            streaming = true,
            sampleRateHz = 22_050,
            approxSizeBytes = 80L * 1024 * 1024,
            sha256 = "18e03e4c2d3497ca4bef342e795682cd32eefa7d37b6ff36266155f2ce30e5fe",
            files = listOf("model-steps-3.onnx", "vocos-22khz-univ.onnx", "tokens.txt"),
            sourceUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/matcha-icefall-en_US-ljspeech.tar.bz2",
            runtimeParams = SherpaRuntimeParams.Tts.Matcha(numThreads = 2),
            voices = listOf(TtsVoiceEntry("ljspeech", "LJSpeech (F)", "en")),
            license = "MIT",
        ),
    )

    fun byId(id: String): OfflineModelEntry? = all.firstOrNull { it.id == id }

    /**
     * The STT+TTS model pair that backs a spoken LANGUAGE. zh uses the bilingual
     * Paraformer (STT) + Kokoro (TTS); en uses Moonshine (STT) + Matcha (TTS).
     */
    fun packFor(language: String): List<OfflineModelEntry> = when (language) {
        "zh" -> listOf(byId("sherpa.paraformer-zh-en-stream")!!, byId("sherpa.kokoro-multi-zh-en")!!)
        "en" -> listOf(byId("sherpa.moonshine-base-en")!!, byId("sherpa.matcha-en")!!)
        else -> emptyList()
    }
}

/** One language pack the wizard offers. */
data class VoicePack(val language: String, val title: String, val subtitle: String) {
    val models: List<OfflineModelEntry> get() = OfflineModelCatalog.packFor(language)
    val totalBytes: Long get() = models.sumOf { it.approxSizeBytes }
}

val VOICE_PACKS = listOf(
    VoicePack("zh", "中文（普通话）", "Paraformer 流式识别 + Kokoro 合成"),
    VoicePack("en", "English", "Moonshine ASR + Matcha-TTS"),
)

/** Download lifecycle for one model (or an aggregate pack). */
sealed interface ModelState {
    data object NotInstalled : ModelState
    data class Downloading(val bytes: Long, val total: Long) : ModelState
    data object Verifying : ModelState
    data object Extracting : ModelState
    data object Ready : ModelState
    data class Failed(val message: String) : ModelState
}
