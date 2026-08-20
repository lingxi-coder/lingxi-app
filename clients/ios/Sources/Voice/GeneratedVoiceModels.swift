import Foundation

// Generated from clients/voice/models.json. Do not edit by hand.

enum GeneratedVoiceModelKind: String, Sendable {
    case stt
    case tts
}

struct GeneratedTtsVoiceEntry: Equatable, Sendable {
    let id: String
    let displayName: String
    let language: String
}

enum GeneratedSherpaRuntimeParams: Equatable, Sendable {
    case asrOnlineTransducer(numThreads: Int, decoding: String)
    case asrOfflineMoonshine(numThreads: Int)
    case ttsVits(numThreads: Int)
    case ttsKitten(numThreads: Int)
}

struct GeneratedOfflineModelEntry: Equatable, Sendable {
    let id: String
    let kind: GeneratedVoiceModelKind
    let displayName: [String: String]
    let languages: [String]
    let streaming: Bool
    let sampleRateHz: Int
    let approxSizeBytes: Int64
    let sha256: String
    let files: [String]
    let requiredDirectories: [String]
    let sourceURL: String
    let runtimeParams: GeneratedSherpaRuntimeParams
    let voices: [GeneratedTtsVoiceEntry]
    let license: String
}

struct GeneratedVoicePack: Equatable, Sendable {
    let language: String
    let title: String
    let subtitle: String
    let modelIDs: [String]

    var models: [GeneratedOfflineModelEntry] {
        modelIDs.compactMap { GeneratedVoiceModelCatalog.byID($0) }
    }

    var totalBytes: Int64 {
        models.reduce(0) { $0 + $1.approxSizeBytes }
    }
}

struct GeneratedRuntimeArtifactMetadata: Equatable, Sendable {
    let name: String
    let sizeBytes: Int64
    let url: String
    let sha256: String
}

enum GeneratedVoiceModelCatalog {
    static let schemaVersion = 1
    static let runtimeVersion = "1.13.2"
    static let androidRuntimeArtifact = GeneratedRuntimeArtifactMetadata(
        name: "sherpa-onnx-static-link-onnxruntime-1.13.2.aar",
        sizeBytes: 38208264,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.2/sherpa-onnx-static-link-onnxruntime-1.13.2.aar",
        sha256: "9b2a290b8c7f31bd0aba35abb4628e87fe8d0eb71796a98aa12f3acd089ceaed"
    )
    static let iosRuntimeArtifact = GeneratedRuntimeArtifactMetadata(
        name: "sherpa-onnx-v1.13.2-ios.tar.bz2",
        sizeBytes: 77611169,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.2/sherpa-onnx-v1.13.2-ios.tar.bz2",
        sha256: "2886a04df4f8d5066c6c8b6e712278d65d7b60fc9e45990223df50262861d38b"
    )

    static let all: [GeneratedOfflineModelEntry] = [
        GeneratedOfflineModelEntry(
                id: "sherpa.zipformer-zh-14m-mobile",
                kind: .stt,
                displayName: ["zh": "Zipformer 中文 14M", "en": "Zipformer Chinese 14M"],
                languages: ["zh"],
                streaming: true,
                sampleRateHz: 16000,
                approxSizeBytes: 54344380,
                sha256: "d394cab72b17f788b8b09ffc610f5f070e610fecf022eedca2eae9e38be4f20a",
                files: ["encoder-epoch-99-avg-1.int8.onnx", "decoder-epoch-99-avg-1.onnx", "joiner-epoch-99-avg-1.int8.onnx", "tokens.txt"],
                requiredDirectories: [],
                sourceURL: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-zh-14M-2023-02-23-mobile.tar.bz2",
                runtimeParams: .asrOnlineTransducer(numThreads: 2, decoding: "greedy_search"),
                voices: [],
                license: "Apache-2.0"
            ),
        GeneratedOfflineModelEntry(
                id: "sherpa.moonshine-tiny-en",
                kind: .stt,
                displayName: ["zh": "Moonshine 英文 Tiny", "en": "Moonshine Tiny (English)"],
                languages: ["en"],
                streaming: false,
                sampleRateHz: 16000,
                approxSizeBytes: 107600538,
                sha256: "d5fe6ec4334fef36255b2a4010412cad4c007e33103fec62fb5d17cad88086f2",
                files: ["preprocess.onnx", "encode.int8.onnx", "uncached_decode.int8.onnx", "cached_decode.int8.onnx", "tokens.txt"],
                requiredDirectories: [],
                sourceURL: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-tiny-en-int8.tar.bz2",
                runtimeParams: .asrOfflineMoonshine(numThreads: 2),
                voices: [],
                license: "MIT"
            ),
        GeneratedOfflineModelEntry(
                id: "sherpa.melo-zh-en",
                kind: .tts,
                displayName: ["zh": "MeloTTS 中英双语", "en": "MeloTTS Chinese + English"],
                languages: ["zh", "en"],
                streaming: true,
                sampleRateHz: 44100,
                approxSizeBytes: 167006755,
                sha256: "e58351ed7149f290a54534538badd4077cdbe6fddc964b24d0bee870415d1514",
                files: ["model.int8.onnx", "tokens.txt", "lexicon.txt", "date.fst", "phone.fst", "number.fst"],
                requiredDirectories: ["dict"],
                sourceURL: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-melo-tts-zh_en.tar.bz2",
                runtimeParams: .ttsVits(numThreads: 2),
                voices: [
                    GeneratedTtsVoiceEntry(id: "melo-zh-en", displayName: "Melo 中英女声", language: "zh")
                ],
                license: "MIT"
            ),
        GeneratedOfflineModelEntry(
                id: "sherpa.kitten-nano-en",
                kind: .tts,
                displayName: ["zh": "Kitten Nano 英文", "en": "Kitten Nano (English)"],
                languages: ["en"],
                streaming: true,
                sampleRateHz: 24000,
                approxSizeBytes: 26586708,
                sha256: "0345a8a2f4a710cb8f7912c9a731ded8b3e1e69b33a871efa95c2e64651518fe",
                files: ["model.fp16.onnx", "voices.bin", "tokens.txt"],
                requiredDirectories: ["espeak-ng-data"],
                sourceURL: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kitten-nano-en-v0_2-fp16.tar.bz2",
                runtimeParams: .ttsKitten(numThreads: 2),
                voices: [
                    GeneratedTtsVoiceEntry(id: "expr-voice-2-f", displayName: "Kitten Female", language: "en")
                ],
                license: "Apache-2.0"
            )
    ]

    static let packs: [GeneratedVoicePack] = [
        GeneratedVoicePack(
                language: "zh",
                title: "中文（普通话）",
                subtitle: "Zipformer 14M 流式识别 + MeloTTS",
                modelIDs: ["sherpa.zipformer-zh-14m-mobile", "sherpa.melo-zh-en"]
            ),
        GeneratedVoicePack(
                language: "en",
                title: "English",
                subtitle: "Moonshine Tiny ASR + Kitten TTS",
                modelIDs: ["sherpa.moonshine-tiny-en", "sherpa.kitten-nano-en"]
            )
    ]

    static func byID(_ id: String) -> GeneratedOfflineModelEntry? {
        all.first { $0.id == id }
    }

    static func packFor(_ language: String) -> [GeneratedOfflineModelEntry] {
        packs.first { $0.language == language }?.models ?? []
    }
}
