package com.lingxi.code.voice.offline

// Generated from resources/voice/models.json. Do not edit by hand.

enum class GeneratedModelKind { Stt, Tts }

data class GeneratedTtsVoiceEntry(
    val id: String,
    val displayName: String,
    val language: String,
)

sealed interface GeneratedSherpaRuntimeParams {
    sealed interface Asr : GeneratedSherpaRuntimeParams {
        data class OnlineTransducer(val numThreads: Int, val decoding: String) : Asr
        data class OfflineMoonshine(val numThreads: Int) : Asr
    }

    sealed interface Tts : GeneratedSherpaRuntimeParams {
        data class Vits(val numThreads: Int) : Tts
        data class Kitten(val numThreads: Int) : Tts
    }
}

data class GeneratedOfflineModelEntry(
    val id: String,
    val kind: GeneratedModelKind,
    val displayName: Map<String, String>,
    val languages: Set<String>,
    val streaming: Boolean,
    val sampleRateHz: Int,
    val approxSizeBytes: Long,
    val sha256: String,
    val files: List<String>,
    val requiredDirectories: List<String> = emptyList(),
    val sourceUrl: String,
    val runtimeParams: GeneratedSherpaRuntimeParams,
    val voices: List<GeneratedTtsVoiceEntry> = emptyList(),
    val license: String,
)

data class GeneratedVoicePack(
    val language: String,
    val title: String,
    val subtitle: String,
    val modelIds: List<String>,
) {
    val models: List<GeneratedOfflineModelEntry> get() = modelIds.mapNotNull(GeneratedVoiceModelCatalog::byId)
    val totalBytes: Long get() = models.sumOf(GeneratedOfflineModelEntry::approxSizeBytes)
}

data class GeneratedRuntimeArtifactMetadata(
    val name: String,
    val sizeBytes: Long,
    val url: String,
    val sha256: String,
)

object GeneratedVoiceModelCatalog {
    const val schemaVersion: Int = 1
    const val runtimeVersion: String = "1.13.2"
    val androidRuntimeArtifact: GeneratedRuntimeArtifactMetadata = GeneratedRuntimeArtifactMetadata(
    name = "sherpa-onnx-static-link-onnxruntime-1.13.2.aar",
    sizeBytes = 38208264L,
    url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.2/sherpa-onnx-static-link-onnxruntime-1.13.2.aar",
    sha256 = "9b2a290b8c7f31bd0aba35abb4628e87fe8d0eb71796a98aa12f3acd089ceaed",
)
    val iosRuntimeArtifact: GeneratedRuntimeArtifactMetadata = GeneratedRuntimeArtifactMetadata(
    name = "sherpa-onnx-v1.13.2-ios.tar.bz2",
    sizeBytes = 77611169L,
    url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.2/sherpa-onnx-v1.13.2-ios.tar.bz2",
    sha256 = "2886a04df4f8d5066c6c8b6e712278d65d7b60fc9e45990223df50262861d38b",
)

    val all: List<GeneratedOfflineModelEntry> = listOf(
        GeneratedOfflineModelEntry(
            id = "sherpa.zipformer-zh-14m-mobile",
            kind = GeneratedModelKind.Stt,
            displayName = mapOf("zh" to "Zipformer 中文 14M", "en" to "Zipformer Chinese 14M"),
            languages = setOf("zh"),
            streaming = true,
            sampleRateHz = 16000,
            approxSizeBytes = 54344380L,
            sha256 = "d394cab72b17f788b8b09ffc610f5f070e610fecf022eedca2eae9e38be4f20a",
            files = listOf("encoder-epoch-99-avg-1.int8.onnx", "decoder-epoch-99-avg-1.onnx", "joiner-epoch-99-avg-1.int8.onnx", "tokens.txt"),
            requiredDirectories = emptyList(),
            sourceUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-zh-14M-2023-02-23-mobile.tar.bz2",
            runtimeParams = GeneratedSherpaRuntimeParams.Asr.OnlineTransducer(numThreads = 2, decoding = "greedy_search"),
            voices = emptyList(),
            license = "Apache-2.0",
        ),
        GeneratedOfflineModelEntry(
            id = "sherpa.moonshine-tiny-en",
            kind = GeneratedModelKind.Stt,
            displayName = mapOf("zh" to "Moonshine 英文 Tiny", "en" to "Moonshine Tiny (English)"),
            languages = setOf("en"),
            streaming = false,
            sampleRateHz = 16000,
            approxSizeBytes = 107600538L,
            sha256 = "d5fe6ec4334fef36255b2a4010412cad4c007e33103fec62fb5d17cad88086f2",
            files = listOf("preprocess.onnx", "encode.int8.onnx", "uncached_decode.int8.onnx", "cached_decode.int8.onnx", "tokens.txt"),
            requiredDirectories = emptyList(),
            sourceUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-tiny-en-int8.tar.bz2",
            runtimeParams = GeneratedSherpaRuntimeParams.Asr.OfflineMoonshine(numThreads = 2),
            voices = emptyList(),
            license = "MIT",
        ),
        GeneratedOfflineModelEntry(
            id = "sherpa.melo-zh-en",
            kind = GeneratedModelKind.Tts,
            displayName = mapOf("zh" to "MeloTTS 中英双语", "en" to "MeloTTS Chinese + English"),
            languages = setOf("zh", "en"),
            streaming = true,
            sampleRateHz = 44100,
            approxSizeBytes = 167006755L,
            sha256 = "e58351ed7149f290a54534538badd4077cdbe6fddc964b24d0bee870415d1514",
            files = listOf("model.int8.onnx", "tokens.txt", "lexicon.txt", "date.fst", "phone.fst", "number.fst"),
            requiredDirectories = listOf("dict"),
            sourceUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-melo-tts-zh_en.tar.bz2",
            runtimeParams = GeneratedSherpaRuntimeParams.Tts.Vits(numThreads = 2),
            voices = listOf(
                GeneratedTtsVoiceEntry(id = "melo-zh-en", displayName = "Melo 中英女声", language = "zh")
            ),
            license = "MIT",
        ),
        GeneratedOfflineModelEntry(
            id = "sherpa.kitten-nano-en",
            kind = GeneratedModelKind.Tts,
            displayName = mapOf("zh" to "Kitten Nano 英文", "en" to "Kitten Nano (English)"),
            languages = setOf("en"),
            streaming = true,
            sampleRateHz = 24000,
            approxSizeBytes = 26586708L,
            sha256 = "0345a8a2f4a710cb8f7912c9a731ded8b3e1e69b33a871efa95c2e64651518fe",
            files = listOf("model.fp16.onnx", "voices.bin", "tokens.txt"),
            requiredDirectories = listOf("espeak-ng-data"),
            sourceUrl = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kitten-nano-en-v0_2-fp16.tar.bz2",
            runtimeParams = GeneratedSherpaRuntimeParams.Tts.Kitten(numThreads = 2),
            voices = listOf(
                GeneratedTtsVoiceEntry(id = "expr-voice-2-f", displayName = "Kitten Female", language = "en")
            ),
            license = "Apache-2.0",
        )
    )

    val packs: List<GeneratedVoicePack> = listOf(
        GeneratedVoicePack(
            language = "zh",
            title = "中文（普通话）",
            subtitle = "Zipformer 14M 流式识别 + MeloTTS",
            modelIds = listOf("sherpa.zipformer-zh-14m-mobile", "sherpa.melo-zh-en"),
        ),
        GeneratedVoicePack(
            language = "en",
            title = "English",
            subtitle = "Moonshine Tiny ASR + Kitten TTS",
            modelIds = listOf("sherpa.moonshine-tiny-en", "sherpa.kitten-nano-en"),
        )
    )

    fun byId(id: String): GeneratedOfflineModelEntry? = all.firstOrNull { it.id == id }

    fun packFor(language: String): List<GeneratedOfflineModelEntry> =
        packs.firstOrNull { it.language == language }?.models ?: emptyList()
}
