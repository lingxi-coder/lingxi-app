@preconcurrency import AVFoundation
import AudioToolbox
import CryptoKit
import Foundation
@preconcurrency import Speech
import SherpaOnnx

private let maxLineBytes = 16 * 1024 * 1024
private let maxInlineAudioBase64Bytes = maxLineBytes - (256 * 1024)
private let defaultLanguage = "en-US"
private let silenceThreshold: Float = 0.015
private let silenceDuration: TimeInterval = 1.2

enum HelperError: Error {
    case invalidRequest(String)
    case unavailable(String)
    case busy(String)
    case cancelled(String)
    case permission(String)
    case modelMissing(String)
    case download(String)
    case checksum(String)
    case native(String)

    var code: String {
        switch self {
        case .invalidRequest: return "invalid-request"
        case .unavailable: return "unavailable"
        case .busy: return "busy"
        case .cancelled: return "cancelled"
        case .permission: return "permission"
        case .modelMissing: return "model-missing"
        case .download: return "download"
        case .checksum: return "checksum"
        case .native: return "native-error"
        }
    }

    var message: String {
        switch self {
        case let .invalidRequest(message),
             let .unavailable(message),
             let .busy(message),
             let .cancelled(message),
             let .permission(message),
             let .modelMissing(message),
             let .download(message),
             let .checksum(message),
             let .native(message):
            return message
        }
    }
}

struct HelperOwner: Codable, Equatable, Sendable {
    let kind: String
    let id: String
}

struct HelperTranscript: Encodable, Sendable {
    let text: String
    let language: String?
    let confidence: Double?
}

struct HelperRecording: Encodable, Sendable {
    let audioBase64: String
    let mimeType: String
}

enum HelperModelState: Equatable, Sendable {
    case notInstalled
    case queued
    case downloading(receivedBytes: Int64, totalBytes: Int64)
    case verifying
    case extracting
    case ready
    case failed(String)
}

extension HelperModelState: Encodable {
    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .notInstalled:
            try container.encode("not-installed", forKey: .type)
        case .queued:
            try container.encode("queued", forKey: .type)
        case let .downloading(receivedBytes, totalBytes):
            try container.encode("downloading", forKey: .type)
            try container.encode(receivedBytes, forKey: .receivedBytes)
            try container.encode(totalBytes, forKey: .totalBytes)
        case .verifying:
            try container.encode("verifying", forKey: .type)
        case .extracting:
            try container.encode("extracting", forKey: .type)
        case .ready:
            try container.encode("ready", forKey: .type)
        case let .failed(message):
            try container.encode("failed", forKey: .type)
            try container.encode(message, forKey: .message)
        }
    }

    private enum CodingKeys: String, CodingKey {
        case type
        case receivedBytes
        case totalBytes
        case message
    }
}

struct HelperModelSnapshot: Encodable, Sendable {
    let modelId: String
    let state: HelperModelState
}

struct HelperVoiceOption: Encodable, Sendable {
    let id: String
    let label: String
    let languageTag: String
    let source: String
    let familyId: String
    let isDefault: Bool?
    let networkRequired: Bool?
}

struct HelperRecognitionSnapshot: Encodable, Sendable {
    let requestedMode: String
    let effectiveBackend: String
    let effectiveLanguage: String
    let detail: String
    let fallbackReason: String?
}

struct HelperPlaybackSnapshot: Encodable, Sendable {
    let requestedVoiceSelection: String
    let effectiveVoiceId: String
    let effectiveVoiceLabel: String
}

struct HelperSnapshot: Encodable, Sendable {
    struct HelperState: Encodable, Sendable {
        let state: String
        let message: String?
    }

    struct Permissions: Encodable, Sendable {
        let microphone: String
        let speech: String
    }

    let helper: HelperState
    let permissions: Permissions
    let owner: HelperOwner?
    let activity: String
    let localeTag: String
    let recognizerAvailable: Bool
    let recognition: HelperRecognitionSnapshot?
    let playback: HelperPlaybackSnapshot?
    let voices: [HelperVoiceOption]
    let models: [HelperModelSnapshot]
}

struct HelperResponse: Encodable, Sendable {
    let type: String
    let snapshot: HelperSnapshot
    let transcript: HelperTranscript?
    let recording: HelperRecording?
    let models: [HelperModelSnapshot]?
    let model: HelperModelSnapshot?
    let error: HelperErrorPayload?
}

struct HelperErrorPayload: Encodable, Sendable {
    let code: String
    let message: String
}

struct HelperRecognitionProgress: Encodable, Sendable {
    let owner: HelperOwner
    let text: String
    let isFinal: Bool
}

struct HelperEvent: Encodable, Sendable {
    let type: String
    let snapshot: HelperSnapshot
    let owner: HelperOwner?
    let progress: HelperRecognitionProgress?
    let model: HelperModelSnapshot?
    let state: String?
    let error: HelperErrorPayload?
    let message: String?
}

enum HelperEngineResult: Sendable {
    case ok
    case recordingState(Bool)
    case recording(audioBase64: String, mimeType: String)
    case transcript(text: String, language: String?, confidence: Double?)
    case audio(pcmBase64: String, sampleRateHz: Int32)
    case failed(kind: String, message: String)
}

extension HelperEngineResult: Encodable {
    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .ok:
            try container.encode("ok", forKey: .type)
        case let .recordingState(recording):
            try container.encode("recording_state", forKey: .type)
            try container.encode(recording, forKey: .recording)
        case let .recording(audioBase64, mimeType):
            try container.encode("recording", forKey: .type)
            try container.encode(audioBase64, forKey: .audioBase64)
            try container.encode(mimeType, forKey: .mimeType)
        case let .transcript(text, language, confidence):
            try container.encode("transcript", forKey: .type)
            try container.encode(text, forKey: .text)
            try container.encodeIfPresent(language, forKey: .language)
            try container.encodeIfPresent(confidence, forKey: .confidence)
        case let .audio(pcmBase64, sampleRateHz):
            try container.encode("audio", forKey: .type)
            try container.encode(pcmBase64, forKey: .pcmBase64)
            try container.encode(sampleRateHz, forKey: .sampleRateHz)
        case let .failed(kind, message):
            try container.encode("failed", forKey: .type)
            try container.encode(kind, forKey: .kind)
            try container.encode(message, forKey: .message)
        }
    }

    private enum CodingKeys: String, CodingKey {
        case type
        case recording
        case audioBase64 = "audio_base64"
        case mimeType = "mime_type"
        case text
        case language
        case confidence
        case pcmBase64 = "pcm_base64"
        case sampleRateHz = "sample_rate_hz"
        case kind
        case message
    }
}

struct HelperEngineResponse: Encodable, Sendable {
    let type = "engine_result"
    let snapshot: HelperSnapshot
    let result: HelperEngineResult
}

enum HelperOutputResult: Encodable, Sendable {
    case response(HelperResponse)
    case engine(HelperEngineResponse)

    func encode(to encoder: Encoder) throws {
        switch self {
        case let .response(response):
            try response.encode(to: encoder)
        case let .engine(response):
            try response.encode(to: encoder)
        }
    }
}

struct OutputEnvelope: Encodable, Sendable {
    let id: String?
    let type: String
    let result: HelperOutputResult?
    let event: HelperEvent?
    let error: HelperErrorPayload?
}

actor LineWriter {
    func writeEnvelope(_ envelope: OutputEnvelope) {
        guard let data = try? JSONEncoder().encode(envelope) else { return }
        if data.count > maxLineBytes {
            FileHandle.standardError.write(Data("native-audio-helper: envelope exceeded JSONL size limit\n".utf8))
            if let id = envelope.id,
               let fallback = try? JSONEncoder().encode(OutputEnvelope(
                   id: id,
                   type: "error",
                   result: nil,
                   event: nil,
                   error: .init(code: "native-error", message: "native audio response exceeded the JSONL size limit")
               )) {
                FileHandle.standardOutput.write(fallback)
                FileHandle.standardOutput.write(Data([0x0a]))
            }
            return
        }
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([0x0a]))
    }
}

func languageBase(_ identifier: String) -> String {
    identifier.replacingOccurrences(of: "_", with: "-")
        .split(separator: "-").first.map(String.init)?.lowercased() ?? ""
}

func resolvedLanguage(_ configured: String?) -> String {
    let normalized = configured?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    if normalized.isEmpty || normalized.lowercased() == "auto" {
        let current = Locale.autoupdatingCurrent.identifier.replacingOccurrences(of: "_", with: "-")
        return current.isEmpty ? defaultLanguage : current
    }
    return normalized.replacingOccurrences(of: "_", with: "-")
}

func base64PCM16Wave(samples: [Float], sampleRate: Int) -> String {
    waveData(pcm16: pcm16Data(from: samples), sampleRate: sampleRate).base64EncodedString()
}

func recordingPayload(samples: [Float], sampleRate: Int, format: String) throws -> HelperRecording {
    switch format {
    case "wav":
        return HelperRecording(
            audioBase64: waveData(pcm16: pcm16Data(from: samples), sampleRate: sampleRate).base64EncodedString(),
            mimeType: "audio/wav"
        )
    case "m4a":
        let url = FileManager.default.temporaryDirectory
            .appending(path: "lingxi-audio-\(UUID().uuidString).m4a")
        defer { try? FileManager.default.removeItem(at: url) }
        guard let pcmFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: Double(sampleRate),
            channels: 1,
            interleaved: false
        ), let buffer = AVAudioPCMBuffer(
            pcmFormat: pcmFormat,
            frameCapacity: AVAudioFrameCount(samples.count)
        ), let channel = buffer.floatChannelData?.pointee else {
            throw HelperError.native("failed to allocate the m4a conversion buffer")
        }
        buffer.frameLength = AVAudioFrameCount(samples.count)
        channel.update(from: samples, count: samples.count)
        let settings: [String: Any] = [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: sampleRate,
            AVNumberOfChannelsKey: 1,
        ]
        let file = try AVAudioFile(
            forWriting: url,
            settings: settings,
            commonFormat: .pcmFormatFloat32,
            interleaved: false
        )
        try file.write(from: buffer)
        return HelperRecording(
            audioBase64: try Data(contentsOf: url).base64EncodedString(),
            mimeType: "audio/mp4"
        )
    default:
        throw HelperError.invalidRequest("unsupported recording format \(format)")
    }
}

func pcm16Data(from samples: [Float]) -> Data {
    var pcm = Data(capacity: samples.count * 2)
    for sample in samples {
        let clamped = max(-1, min(1, sample))
        var value = Int16((clamped * 32767).rounded())
        withUnsafeBytes(of: &value) { pcm.append(contentsOf: $0) }
    }
    return pcm
}

func waveData(pcm16: Data, sampleRate: Int) -> Data {
    let headerSize = 44
    let dataSize = pcm16.count
    let totalSize = UInt32(headerSize - 8 + dataSize)
    var wav = Data(capacity: headerSize + dataSize)
    wav.append("RIFF".data(using: .ascii)!)
    var riffSize = totalSize.littleEndian
    withUnsafeBytes(of: &riffSize) { wav.append(contentsOf: $0) }
    wav.append("WAVEfmt ".data(using: .ascii)!)
    var fmtSize = UInt32(16).littleEndian
    withUnsafeBytes(of: &fmtSize) { wav.append(contentsOf: $0) }
    var audioFormat = UInt16(1).littleEndian
    var channels = UInt16(1).littleEndian
    var sampleRateLE = UInt32(sampleRate).littleEndian
    var byteRate = UInt32(sampleRate * 2).littleEndian
    var blockAlign = UInt16(2).littleEndian
    var bitsPerSample = UInt16(16).littleEndian
    withUnsafeBytes(of: &audioFormat) { wav.append(contentsOf: $0) }
    withUnsafeBytes(of: &channels) { wav.append(contentsOf: $0) }
    withUnsafeBytes(of: &sampleRateLE) { wav.append(contentsOf: $0) }
    withUnsafeBytes(of: &byteRate) { wav.append(contentsOf: $0) }
    withUnsafeBytes(of: &blockAlign) { wav.append(contentsOf: $0) }
    withUnsafeBytes(of: &bitsPerSample) { wav.append(contentsOf: $0) }
    wav.append("data".data(using: .ascii)!)
    var dataSizeLE = UInt32(dataSize).littleEndian
    withUnsafeBytes(of: &dataSizeLE) { wav.append(contentsOf: $0) }
    wav.append(pcm16)
    return wav
}

func ensureInlineAudioFits(_ base64: String, label: String) throws {
    if base64.utf8.count > maxInlineAudioBase64Bytes {
        throw HelperError.unavailable("the \(label) audio payload is too large to send over helper IPC")
    }
}

func shell(_ executable: String, _ arguments: [String]) throws -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    let output = Pipe()
    let error = Pipe()
    process.standardOutput = output
    process.standardError = error
    try process.run()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
        let detail = String(data: error.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        throw HelperError.download(detail.isEmpty ? "archive command failed" : detail.trimmingCharacters(in: .whitespacesAndNewlines))
    }
    return String(data: output.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
}

func validateArchivePath(_ path: String) -> Bool {
    guard !path.isEmpty, !path.hasPrefix("/") else { return false }
    return !path.split(separator: "/").contains("..")
}

func currentLocaleTag() -> String {
    let locale = Locale.autoupdatingCurrent.identifier.replacingOccurrences(of: "_", with: "-")
    return locale.isEmpty ? defaultLanguage : locale
}

func recognizerAvailability(for language: String) -> Bool {
    guard let recognizer = SFSpeechRecognizer(locale: Locale(identifier: language)) else { return false }
    return recognizer.isAvailable && recognizer.supportsOnDeviceRecognition
}

func availableVoices(root: URL) -> [HelperVoiceOption] {
    var voices: [HelperVoiceOption] = [
        HelperVoiceOption(
            id: "system:default",
            label: "System Default",
            languageTag: currentLocaleTag(),
            source: "system",
            familyId: "system",
            isDefault: true,
            networkRequired: false
        ),
    ]
    voices.append(contentsOf: AVSpeechSynthesisVoice.speechVoices().map { voice in
        HelperVoiceOption(
            id: "system:\(voice.identifier)",
            label: voice.name,
            languageTag: voice.language.replacingOccurrences(of: "_", with: "-"),
            source: "system",
            familyId: "system",
            isDefault: false,
            networkRequired: false
        )
    })
    for model in GeneratedVoiceModelCatalog.all where model.kind == .tts {
        guard SherpaRuntime.modelDirectory(for: model, root: root) != nil else { continue }
        for voice in model.voices {
            voices.append(HelperVoiceOption(
                id: "sherpa:\(model.id):\(voice.id)",
                label: voice.displayName,
                languageTag: voice.language,
                source: "sherpa",
                familyId: model.id,
                isDefault: false,
                networkRequired: false
            ))
        }
    }
    return voices.sorted { left, right in
        if left.source != right.source { return left.source < right.source }
        if left.languageTag != right.languageTag { return left.languageTag < right.languageTag }
        return left.label.localizedCaseInsensitiveCompare(right.label) == .orderedAscending
    }
}

enum ResolvedPlaybackVoice {
    case system(AVSpeechSynthesisVoice?, HelperPlaybackSnapshot)
    case sherpa(String, HelperPlaybackSnapshot)

    var snapshot: HelperPlaybackSnapshot {
        switch self {
        case let .system(_, snapshot), let .sherpa(_, snapshot):
            return snapshot
        }
    }
}

func resolvePlaybackVoice(_ requestedVoiceID: String?, language: String, root: URL) throws -> ResolvedPlaybackVoice {
    let requested = requestedVoiceID?.isEmpty == false ? requestedVoiceID! : "system:default"
    if requested.hasPrefix("sherpa:") {
        if let (model, voice, _) = SherpaRuntime.voiceEntry(for: requested, languageIdentifier: language, root: root) {
            return .sherpa(
                "sherpa:\(model.id):\(voice.id)",
                HelperPlaybackSnapshot(
                    requestedVoiceSelection: requested,
                    effectiveVoiceId: "sherpa:\(model.id):\(voice.id)",
                    effectiveVoiceLabel: voice.displayName
                )
            )
        }
        let fallback = AVSpeechSynthesisVoice(language: language) ?? AVSpeechSynthesisVoice.speechVoices().first
        return .system(
            fallback,
            HelperPlaybackSnapshot(
                requestedVoiceSelection: requested,
                effectiveVoiceId: fallback.map { "system:\($0.identifier)" } ?? "system:default",
                effectiveVoiceLabel: fallback?.name ?? "System Default"
            )
        )
    }
    let explicitSystemID = requested.hasPrefix("system:") ? String(requested.dropFirst("system:".count)) : nil
    let systemVoices = AVSpeechSynthesisVoice.speechVoices()
    let legacyNameMatch = explicitSystemID.flatMap { name in
        systemVoices.first { $0.name.compare(name, options: .caseInsensitive) == .orderedSame }
    }
    let voice = explicitSystemID == nil || explicitSystemID == "default"
        ? (AVSpeechSynthesisVoice(language: language) ?? systemVoices.first)
        : (AVSpeechSynthesisVoice(identifier: explicitSystemID!) ?? legacyNameMatch ?? AVSpeechSynthesisVoice(language: language) ?? systemVoices.first)
    return .system(
        voice,
        HelperPlaybackSnapshot(
            requestedVoiceSelection: requested,
            effectiveVoiceId: voice.map { "system:\($0.identifier)" } ?? "system:default",
            effectiveVoiceLabel: voice?.name ?? "System Default"
        )
    )
}

final class SherpaRuntime {
    static func recognitionModel(for languageIdentifier: String, root: URL) -> GeneratedOfflineModelEntry? {
        GeneratedVoiceModelCatalog.packFor(languageBase(languageIdentifier)).first {
            $0.kind == .stt && modelDirectory(for: $0, root: root) != nil
        }
    }

    static func voiceEntry(for voiceID: String, languageIdentifier: String, root: URL) -> (GeneratedOfflineModelEntry, GeneratedTtsVoiceEntry, URL)? {
        guard voiceID.hasPrefix("sherpa:") else { return nil }
        let payload = String(voiceID.dropFirst("sherpa:".count))
        guard let separator = payload.lastIndex(of: ":") else { return nil }
        let modelID = String(payload[..<separator])
        let requestedVoiceID = String(payload[payload.index(after: separator)...])
        guard let model = GeneratedVoiceModelCatalog.byID(modelID),
              let directory = modelDirectory(for: model, root: root) else { return nil }
        let target = languageBase(languageIdentifier)
        let voice = model.voices.first { $0.id == requestedVoiceID }
            ?? model.voices.first { $0.language == target }
            ?? model.voices.first
        guard let voice else { return nil }
        return (model, voice, directory)
    }

    static func modelDirectory(for entry: GeneratedOfflineModelEntry, root: URL) -> URL? {
        let base = root.appending(path: entry.id, directoryHint: .isDirectory)
        let fm = FileManager.default
        let candidates = [base] + ((try? fm.contentsOfDirectory(
            at: base,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        )) ?? []).filter { (try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true }
        return candidates.first { candidate in
            entry.files.allSatisfy { fm.fileExists(atPath: candidate.appending(path: $0).path) }
            && entry.requiredDirectories.allSatisfy {
                var isDirectory: ObjCBool = false
                return fm.fileExists(atPath: candidate.appending(path: $0).path, isDirectory: &isDirectory) && isDirectory.boolValue
            }
        }
    }

    static func transcribe(samples: [Float], sampleRate: Int, languageIdentifier: String, root: URL) throws -> HelperTranscript {
        guard let model = recognitionModel(for: languageIdentifier, root: root),
              let directory = modelDirectory(for: model, root: root) else {
            throw HelperError.modelMissing("an installed offline recognition model is required for \(languageBase(languageIdentifier))")
        }
        let text: String
        switch model.runtimeParams {
        case .asrOnlineTransducer:
            let encoder = directory.appending(path: "encoder-epoch-99-avg-1.int8.onnx").path
            let decoder = directory.appending(path: "decoder-epoch-99-avg-1.onnx").path
            let joiner = directory.appending(path: "joiner-epoch-99-avg-1.int8.onnx").path
            let tokens = directory.appending(path: "tokens.txt").path
            let modelConfig = sherpaOnnxOnlineModelConfig(
                tokens: tokens,
                transducer: sherpaOnnxOnlineTransducerModelConfig(
                    encoder: encoder,
                    decoder: decoder,
                    joiner: joiner
                ),
                numThreads: 2,
                provider: "cpu"
            )
            var config = sherpaOnnxOnlineRecognizerConfig(
                featConfig: sherpaOnnxFeatureConfig(sampleRate: sampleRate, featureDim: 80),
                modelConfig: modelConfig,
                enableEndpoint: false,
                decodingMethod: "greedy_search"
            )
            let recognizer = withUnsafePointer(to: &config) { SherpaOnnxRecognizer(config: $0) }
            recognizer.acceptWaveform(samples: samples, sampleRate: sampleRate)
            while recognizer.isReady() { recognizer.decode() }
            recognizer.inputFinished()
            while recognizer.isReady() { recognizer.decode() }
            text = recognizer.getResult().text
        case .asrOfflineMoonshine:
            let tokens = directory.appending(path: "tokens.txt").path
            let modelConfig = sherpaOnnxOfflineModelConfig(
                tokens: tokens,
                numThreads: 2,
                provider: "cpu",
                moonshine: sherpaOnnxOfflineMoonshineModelConfig(
                    preprocessor: directory.appending(path: "preprocess.onnx").path,
                    encoder: directory.appending(path: "encode.int8.onnx").path,
                    uncachedDecoder: directory.appending(path: "uncached_decode.int8.onnx").path,
                    cachedDecoder: directory.appending(path: "cached_decode.int8.onnx").path
                )
            )
            var config = sherpaOnnxOfflineRecognizerConfig(
                featConfig: sherpaOnnxFeatureConfig(sampleRate: sampleRate, featureDim: 80),
                modelConfig: modelConfig,
                decodingMethod: "greedy_search"
            )
            let recognizer = withUnsafePointer(to: &config) { SherpaOnnxOfflineRecognizer(config: $0) }
            text = recognizer.decode(samples: samples, sampleRate: sampleRate).text
        default:
            throw HelperError.modelMissing("the installed model does not support speech recognition")
        }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            throw HelperError.unavailable("no speech was recognized")
        }
        return HelperTranscript(text: trimmed, language: languageIdentifier, confidence: nil)
    }

    static func synthesize(text: String, voiceID: String, languageIdentifier: String, rate: Double, root: URL) throws -> (Data, Int32) {
        guard let (model, voice, directory) = voiceEntry(for: voiceID, languageIdentifier: languageIdentifier, root: root) else {
            throw HelperError.modelMissing("the requested offline voice is not installed")
        }
        let speakerID = Int32(model.voices.firstIndex(where: { $0.id == voice.id }) ?? 0)
        switch model.runtimeParams {
        case .ttsKitten:
            var config = sherpaOnnxOfflineTtsConfig(
                model: sherpaOnnxOfflineTtsModelConfig(
                    debug: 0,
                    kitten: sherpaOnnxOfflineTtsKittenModelConfig(
                        model: directory.appending(path: "model.fp16.onnx").path,
                        voices: directory.appending(path: "voices.bin").path,
                        tokens: directory.appending(path: "tokens.txt").path,
                        dataDir: directory.appending(path: "espeak-ng-data").path
                    )
                ),
                maxNumSentences: 2,
                silenceScale: 0.2
            )
            let tts = withUnsafePointer(to: &config) { SherpaOnnxOfflineTtsWrapper(config: $0) }
            let generated = tts.generate(text: text, sid: Int(speakerID), speed: Float(max(0.5, min(2, rate))))
            guard generated.n > 0, generated.sampleRate > 0 else {
                throw HelperError.native("the offline synthesizer failed to generate speech")
            }
            return (pcm16Data(from: generated.samples), generated.sampleRate)
        case .ttsVits:
            var config = sherpaOnnxOfflineTtsConfig(
                model: sherpaOnnxOfflineTtsModelConfig(
                    vits: sherpaOnnxOfflineTtsVitsModelConfig(
                        model: directory.appending(path: "model.int8.onnx").path,
                        lexicon: directory.appending(path: "lexicon.txt").path,
                        tokens: directory.appending(path: "tokens.txt").path,
                        dataDir: directory.appending(path: "dict").path
                    )
                ),
                ruleFsts: [
                    directory.appending(path: "phone.fst").path,
                    directory.appending(path: "date.fst").path,
                    directory.appending(path: "number.fst").path,
                ].joined(separator: ","),
                maxNumSentences: 2,
                silenceScale: 0.2
            )
            let tts = withUnsafePointer(to: &config) { SherpaOnnxOfflineTtsWrapper(config: $0) }
            let generated = tts.generate(text: text, sid: Int(speakerID), speed: Float(max(0.5, min(2, rate))))
            guard generated.n > 0, generated.sampleRate > 0 else {
                throw HelperError.native("the offline synthesizer failed to generate speech")
            }
            return (pcm16Data(from: generated.samples), generated.sampleRate)
        default:
            throw HelperError.modelMissing("the requested offline model does not support speech synthesis")
        }
    }
}

@MainActor
final class ListeningSession {
    enum Route {
        case system
        case sherpa
        case captureOnly
    }

    private let owner: HelperOwner
    private let languageIdentifier: String
    private let route: Route
    private let outputSampleRate: Int
    private let wantsRecording: Bool
    private let onPartial: @Sendable (HelperTranscript) async -> Void
    private let onSilence: @Sendable () async -> Void

    private let engine = AVAudioEngine()
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var transcriptText = ""
    private var confidence: Double?
    private var recognitionFailure: String?
    private var samples: [Float] = []
    private var lastSpeechAt = Date()
    private var hasHeardSpeech = false
    private var monitorTask: Task<Void, Never>?

    init(
        owner: HelperOwner,
        languageIdentifier: String,
        route: Route,
        outputSampleRate: Int,
        wantsRecording: Bool,
        onPartial: @escaping @Sendable (HelperTranscript) async -> Void,
        onSilence: @escaping @Sendable () async -> Void
    ) {
        self.owner = owner
        self.languageIdentifier = languageIdentifier
        self.route = route
        self.outputSampleRate = outputSampleRate
        self.wantsRecording = wantsRecording
        self.onPartial = onPartial
        self.onSilence = onSilence
    }

    func start() throws {
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0 else {
            throw HelperError.unavailable("the microphone input is unavailable")
        }

        if route == .system {
            let recognizer = SFSpeechRecognizer(locale: Locale(identifier: languageIdentifier))
            guard let recognizer, recognizer.isAvailable, recognizer.supportsOnDeviceRecognition else {
                throw HelperError.unavailable("on-device Apple Speech is unavailable for \(languageIdentifier)")
            }
            let request = SFSpeechAudioBufferRecognitionRequest()
            request.shouldReportPartialResults = true
            request.requiresOnDeviceRecognition = true
            self.request = request
            task = recognizer.recognitionTask(with: request) { [weak self] result, error in
                guard let self else { return }
                Task { @MainActor in
                    if let result {
                        self.transcriptText = result.bestTranscription.formattedString
                        let segments = result.bestTranscription.segments
                        self.confidence = segments.isEmpty
                            ? nil
                            : Double(segments.map(\.confidence).reduce(0, +) / Float(segments.count))
                        self.lastSpeechAt = Date()
                        self.hasHeardSpeech = !self.transcriptText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        await self.onPartial(HelperTranscript(
                            text: self.transcriptText,
                            language: self.languageIdentifier,
                            confidence: self.confidence
                        ))
                    } else if let error {
                        self.recognitionFailure = error.localizedDescription
                    }
                }
            }
        }

        input.removeTap(onBus: 0)
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
            guard let self else { return }
            Task { @MainActor in
                self.capture(buffer: buffer)
            }
        }
        engine.prepare()
        try engine.start()

        monitorTask = Task { [weak self] in
            while let self {
                try? await Task.sleep(nanoseconds: 150_000_000)
                await MainActor.run {
                    guard self.engine.isRunning else { return }
                    let elapsed = Date().timeIntervalSince(self.lastSpeechAt)
                    if self.hasHeardSpeech, elapsed >= silenceDuration {
                        Task { await self.onSilence() }
                    }
                }
            }
        }
    }

    private func capture(buffer: AVAudioPCMBuffer) {
        let frameCount = Int(buffer.frameLength)
        guard frameCount > 0 else { return }
        if route == .system {
            request?.append(buffer)
        }
        guard let channelData = buffer.floatChannelData?.pointee else { return }
        let source = UnsafeBufferPointer(start: channelData, count: frameCount)
        let sourceRate = Int(buffer.format.sampleRate.rounded())
        let converted = sourceRate == outputSampleRate
            ? Array(source)
            : resample(Array(source), from: sourceRate, to: outputSampleRate)
        samples.append(contentsOf: converted)
        let rms = sqrt(converted.reduce(0) { $0 + ($1 * $1) } / Float(max(1, converted.count)))
        if rms >= silenceThreshold {
            lastSpeechAt = Date()
            hasHeardSpeech = true
        }
    }

    private func resample(_ input: [Float], from sourceRate: Int, to targetRate: Int) -> [Float] {
        if input.isEmpty || sourceRate == targetRate { return input }
        let ratio = Double(targetRate) / Double(sourceRate)
        let outputCount = max(1, Int((Double(input.count) * ratio).rounded()))
        return (0..<outputCount).map { index in
            let sourcePosition = Double(index) / ratio
            let lowerIndex = Int(sourcePosition.rounded(.down))
            let upperIndex = min(input.count - 1, lowerIndex + 1)
            let fraction = Float(sourcePosition - Double(lowerIndex))
            return input[lowerIndex] + ((input[upperIndex] - input[lowerIndex]) * fraction)
        }
    }

    func stop() -> ([Float], HelperTranscript?, Int, String?) {
        monitorTask?.cancel()
        monitorTask = nil
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        request?.endAudio()
        task?.cancel()
        let trimmed = transcriptText.trimmingCharacters(in: .whitespacesAndNewlines)
        let transcript = trimmed.isEmpty || recognitionFailure != nil
            ? nil
            : HelperTranscript(text: trimmed, language: languageIdentifier, confidence: confidence)
        return (samples, transcript, outputSampleRate, recognitionFailure)
    }
}

@MainActor
final class PlaybackSession: NSObject {
    private enum Mode {
        case system(AVSpeechSynthesizer)
        case audioPlayer(AVAudioPlayer)
    }

    private var mode: Mode?
    private var continuation: CheckedContinuation<Void, Error>?

    func playSystem(text: String, voice: AVSpeechSynthesisVoice?, rate: Float) async throws {
        try stopIfNeeded()
        let synthesizer = AVSpeechSynthesizer()
        synthesizer.delegate = self
        let utterance = AVSpeechUtterance(string: text)
        utterance.rate = rate
        utterance.voice = voice
        mode = .system(synthesizer)
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            self.continuation = continuation
            synthesizer.speak(utterance)
        }
    }

    func playPCM16(_ pcm16: Data, sampleRate: Int32) async throws {
        try stopIfNeeded()
        let player = try AVAudioPlayer(data: waveData(pcm16: pcm16, sampleRate: Int(sampleRate)))
        player.delegate = self
        player.prepareToPlay()
        guard player.play() else {
            throw HelperError.native("failed to start audio playback")
        }
        mode = .audioPlayer(player)
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            self.continuation = continuation
        }
    }

    func stop() {
        switch mode {
        case let .system(synthesizer):
            synthesizer.stopSpeaking(at: .immediate)
        case let .audioPlayer(player):
            player.stop()
            finish(with: .failure(HelperError.cancelled("speech playback was cancelled")))
        case nil:
            break
        }
    }

    private func stopIfNeeded() throws {
        if continuation != nil {
            stop()
            throw HelperError.busy("another playback session is already active")
        }
    }

    private func finish(with result: Result<Void, Error>) {
        mode = nil
        guard let continuation else { return }
        self.continuation = nil
        continuation.resume(with: result)
    }

}

extension PlaybackSession: AVSpeechSynthesizerDelegate {
    nonisolated func speechSynthesizer(_: AVSpeechSynthesizer, didFinish _: AVSpeechUtterance) {
        Task { @MainActor [weak self] in
            self?.finish(with: .success(()))
        }
    }

    nonisolated func speechSynthesizer(_: AVSpeechSynthesizer, didCancel _: AVSpeechUtterance) {
        Task { @MainActor [weak self] in
            self?.finish(with: .failure(HelperError.cancelled("speech playback was cancelled")))
        }
    }
}

extension PlaybackSession: AVAudioPlayerDelegate {
    nonisolated func audioPlayerDidFinishPlaying(_: AVAudioPlayer, successfully flag: Bool) {
        Task { @MainActor [weak self] in
            self?.finish(with: flag ? .success(()) : .failure(HelperError.native("audio playback did not finish successfully")))
        }
    }

    nonisolated func audioPlayerDecodeErrorDidOccur(_: AVAudioPlayer, error: Error?) {
        Task { @MainActor [weak self] in
            self?.finish(with: .failure(error ?? HelperError.native("audio playback failed")))
        }
    }
}

actor ModelStore {
    private struct PartialDownloadMetadata: Codable {
        let etag: String?
        let lastModified: String?
    }

    private let root: URL
    private let writer: LineWriter
    private let snapshotProvider: @Sendable () async -> HelperSnapshot
    private var states: [String: HelperModelSnapshot] = [:]
    private var tasks: [String: Task<Void, Never>] = [:]

    init(root: URL, writer: LineWriter, snapshotProvider: @escaping @Sendable () async -> HelperSnapshot) {
        self.root = root
        self.writer = writer
        self.snapshotProvider = snapshotProvider
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        self.states = Dictionary(uniqueKeysWithValues: GeneratedVoiceModelCatalog.all.map { model in
            let ready = SherpaRuntime.modelDirectory(for: model, root: root) != nil
            return (
                model.id,
                HelperModelSnapshot(
                    modelId: model.id,
                    state: ready ? .ready : .notInstalled
                )
            )
        })
    }

    func snapshots() -> [HelperModelSnapshot] {
        GeneratedVoiceModelCatalog.all.compactMap { states[$0.id] }.sorted(by: { $0.modelId < $1.modelId })
    }

    func reconcile() {
        var next: [String: HelperModelSnapshot] = [:]
        for model in GeneratedVoiceModelCatalog.all {
            let ready = SherpaRuntime.modelDirectory(for: model, root: root) != nil
            next[model.id] = HelperModelSnapshot(
                modelId: model.id,
                state: ready ? .ready : .notInstalled
            )
        }
        states = next
    }

    func install(modelID: String) async throws {
        guard let model = GeneratedVoiceModelCatalog.byID(modelID) else {
            throw HelperError.invalidRequest("unknown model id \(modelID)")
        }
        if states[modelID]?.state == .ready { return }
        if tasks[modelID] != nil { return }
        states[modelID] = HelperModelSnapshot(
            modelId: model.id,
            state: .queued
        )
        await publishModel(modelID)
        let task = Task {
            do {
                let archive = self.root.appending(path: ".download-\(model.id).part")
                let staging = self.root.appending(path: ".stage-\(model.id)-\(UUID().uuidString)", directoryHint: .isDirectory)
                try FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
                defer { try? FileManager.default.removeItem(at: staging) }
                try await self.download(model: model, archive: archive)
                try await self.updateState(model.id, state: .verifying)
                let digest = try await ModelStore.sha256(of: archive)
                if digest != model.sha256 {
                    throw HelperError.checksum("the downloaded model failed checksum verification")
                }
                try await self.updateState(model.id, state: .extracting)
                let listed = try shell("/usr/bin/tar", ["-tf", archive.path])
                    .split(separator: "\n")
                    .map(String.init)
                    .filter { !$0.isEmpty }
                guard !listed.isEmpty, listed.allSatisfy(validateArchivePath) else {
                    throw HelperError.download("the downloaded archive contains an unsafe path")
                }
                let verboseList = try shell("/usr/bin/tar", ["-tvf", archive.path])
                    .split(separator: "\n")
                    .map(String.init)
                    .filter { !$0.isEmpty }
                guard verboseList.allSatisfy({ line in
                    guard let first = line.first else { return false }
                    return first != "l" && first != "h"
                }) else {
                    throw HelperError.download("the downloaded archive contains unsupported link entries")
                }
                _ = try shell("/usr/bin/tar", ["-xf", archive.path, "-C", staging.path])
                try Self.ensureNoSymlinks(in: staging)
                guard let extracted = SherpaRuntime.modelDirectory(for: model, root: staging) else {
                    throw HelperError.download("the downloaded archive did not contain the expected model files")
                }
                let final = self.root.appending(path: model.id, directoryHint: .isDirectory)
                let activation = self.root.appending(path: ".activate-\(model.id)-\(UUID().uuidString)", directoryHint: .isDirectory)
                try? FileManager.default.removeItem(at: activation)
                try FileManager.default.moveItem(at: extracted, to: activation)
                if FileManager.default.fileExists(atPath: final.path) {
                    try? FileManager.default.removeItem(at: final)
                }
                try FileManager.default.moveItem(at: activation, to: final)
                try? FileManager.default.removeItem(at: archive)
                try? FileManager.default.removeItem(at: self.metadataURL(for: model.id))
                self.reconcile()
                await self.publishModel(model.id)
                await self.writer.writeEnvelope(OutputEnvelope(
                    id: nil,
                    type: "event",
                    result: nil,
                    event: HelperEvent(
                        type: "snapshot_changed",
                        snapshot: await self.snapshotProvider(),
                        owner: nil,
                        progress: nil,
                        model: nil,
                        state: nil,
                        error: nil,
                        message: nil
                    ),
                    error: nil
                ))
            } catch is CancellationError {
                self.reconcile()
                await self.publishModel(model.id)
            } catch let error as HelperError {
                await self.setFailed(model.id, code: error.code, message: error.message)
            } catch {
                await self.setFailed(model.id, code: "download", message: error.localizedDescription)
            }
            self.clearTask(model.id)
        }
        tasks[modelID] = task
    }

    func cancel(modelID: String) {
        tasks[modelID]?.cancel()
        tasks[modelID] = nil
        reconcile()
    }

    func remove(modelID: String) throws {
        tasks[modelID]?.cancel()
        tasks[modelID] = nil
        try? FileManager.default.removeItem(at: root.appending(path: modelID, directoryHint: .isDirectory))
        try? FileManager.default.removeItem(at: root.appending(path: ".download-\(modelID).part"))
        try? FileManager.default.removeItem(at: metadataURL(for: modelID))
        reconcile()
    }

    private func clearTask(_ modelID: String) {
        tasks[modelID] = nil
    }

    private func setFailed(_ modelID: String, code _: String, message: String) async {
        if let current = states[modelID] {
            states[modelID] = HelperModelSnapshot(
                modelId: current.modelId,
                state: .failed(message)
            )
            await publishModel(modelID)
        }
    }

    private func updateState(_ modelID: String, state: HelperModelState) async throws {
        guard let current = states[modelID] else { throw HelperError.invalidRequest("unknown model id \(modelID)") }
        states[modelID] = HelperModelSnapshot(
            modelId: current.modelId,
            state: state
        )
        await publishModel(modelID)
    }

    private func publishModel(_ modelID: String) async {
        guard let model = states[modelID] else { return }
        await writer.writeEnvelope(OutputEnvelope(
            id: nil,
            type: "event",
            result: nil,
            event: HelperEvent(
                type: "model_state",
                snapshot: await snapshotProvider(),
                owner: nil,
                progress: nil,
                model: model,
                state: nil,
                error: nil,
                message: nil
            ),
            error: nil
        ))
    }

    private func download(model: GeneratedOfflineModelEntry, archive: URL) async throws {
        guard let url = URL(string: model.sourceURL) else { throw HelperError.download("the model URL is invalid") }
        let existingBytes = (try? FileManager.default.attributesOfItem(atPath: archive.path)[.size] as? NSNumber)?.int64Value ?? 0
        let existingMetadata = loadPartialMetadata(for: model.id)
        var request = URLRequest(url: url)
        if existingBytes > 0 {
            request.setValue("bytes=\(existingBytes)-", forHTTPHeaderField: "Range")
            if let ifRange = existingMetadata?.etag ?? existingMetadata?.lastModified {
                request.setValue(ifRange, forHTTPHeaderField: "If-Range")
            }
        }
        let (bytes, response) = try await URLSession.shared.bytes(for: request)
        guard let response = response as? HTTPURLResponse else {
            throw HelperError.download("the model download failed")
        }
        let restartDownload = existingBytes > 0 && response.statusCode == 200
        if restartDownload {
            try? FileManager.default.removeItem(at: archive)
            try? FileManager.default.removeItem(at: metadataURL(for: model.id))
            return try await download(model: model, archive: archive)
        }
        guard response.statusCode == 200 || response.statusCode == 206 else {
            throw HelperError.download("the model download failed")
        }
        savePartialMetadata(
            PartialDownloadMetadata(
                etag: response.value(forHTTPHeaderField: "ETag"),
                lastModified: response.value(forHTTPHeaderField: "Last-Modified")
            ),
            for: model.id
        )
        if !FileManager.default.fileExists(atPath: archive.path) {
            FileManager.default.createFile(atPath: archive.path, contents: nil)
        }
        let handle = try FileHandle(forWritingTo: archive)
        defer { try? handle.close() }
        if existingBytes > 0 {
            try handle.seekToEnd()
        } else {
            try handle.truncate(atOffset: 0)
        }
        let expectedBytes = max(
            model.approxSizeBytes,
            existingBytes + Int64(response.expectedContentLength > 0 ? response.expectedContentLength : 0)
        )
        try await updateState(model.id, state: .downloading(receivedBytes: existingBytes, totalBytes: expectedBytes))
        var buffer = Data()
        var writtenBytes = existingBytes
        var lastPublishedBytes = existingBytes
        for try await byte in bytes {
            try Task.checkCancellation()
            buffer.append(byte)
            if buffer.count >= 64 * 1024 {
                handle.write(buffer)
                writtenBytes += Int64(buffer.count)
                buffer.removeAll(keepingCapacity: true)
                if writtenBytes - lastPublishedBytes >= 256 * 1024 {
                    lastPublishedBytes = writtenBytes
                    try await updateState(model.id, state: .downloading(receivedBytes: writtenBytes, totalBytes: expectedBytes))
                }
            }
        }
        if !buffer.isEmpty {
            handle.write(buffer)
            writtenBytes += Int64(buffer.count)
        }
        try await updateState(model.id, state: .downloading(receivedBytes: writtenBytes, totalBytes: expectedBytes))
    }

    private func loadPartialMetadata(for modelID: String) -> PartialDownloadMetadata? {
        let url = metadataURL(for: modelID)
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(PartialDownloadMetadata.self, from: data)
    }

    private func savePartialMetadata(_ metadata: PartialDownloadMetadata, for modelID: String) {
        guard let data = try? JSONEncoder().encode(metadata) else { return }
        try? data.write(to: metadataURL(for: modelID), options: .atomic)
    }

    private func metadataURL(for modelID: String) -> URL {
        root.appending(path: ".download-\(modelID).json")
    }

    private static func ensureNoSymlinks(in root: URL) throws {
        let keys: Set<URLResourceKey> = [.isSymbolicLinkKey]
        guard let enumerator = FileManager.default.enumerator(
            at: root,
            includingPropertiesForKeys: Array(keys),
            options: [.skipsHiddenFiles]
        ) else {
            return
        }
        for case let fileURL as URL in enumerator {
            let values = try fileURL.resourceValues(forKeys: keys)
            if values.isSymbolicLink == true {
                throw HelperError.download("the extracted model contains an unsupported symbolic link")
            }
        }
    }

    private static func sha256(of file: URL) async throws -> String {
        try await Task.detached(priority: .utility) {
            let handle = try FileHandle(forReadingFrom: file)
            defer { try? handle.close() }
            var hash = SHA256()
            while let data = try handle.read(upToCount: 1024 * 1024), !data.isEmpty {
                try Task.checkCancellation()
                hash.update(data: data)
            }
            return hash.finalize().map { String(format: "%02x", $0) }.joined()
        }.value
    }
}

actor HelperStateStore {
    private let storageRoot: URL
    private let writer: LineWriter
    private lazy var models = ModelStore(root: storageRoot, writer: writer) { [weak self] in
        await self?.snapshot(message: nil) ?? HelperSnapshot(
            helper: .init(state: "running", message: nil),
            permissions: .init(microphone: "unavailable", speech: "unavailable"),
            owner: nil,
            activity: "idle",
            localeTag: currentLocaleTag(),
            recognizerAvailable: false,
            recognition: nil,
            playback: nil,
            voices: [],
            models: []
        )
    }

    private var owner: HelperOwner?
    private var activity = "idle"
    private var listeningOwner: HelperOwner?
    private var activeSession: ListeningSession?
    private var activeSpeech: PlaybackSession?
    private var speechTask: Task<Void, Never>?
    private var lastRecordingByOwner: [String: (samples: [Float], sampleRate: Int, recording: HelperRecording, transcript: HelperTranscript?)] = [:]
    private var engineRecordingFormatByOwner: [String: String] = [:]
    private var helperMessage: String?
    private var recognitionSnapshot: HelperRecognitionSnapshot?
    private var playbackSnapshot: HelperPlaybackSnapshot?
    private var activeLanguage = currentLocaleTag()

    init(storageRoot: URL, writer: LineWriter) {
        self.storageRoot = storageRoot
        self.writer = writer
    }

    func snapshot(message: String?) async -> HelperSnapshot {
        let speechStatus = SFSpeechRecognizer.authorizationStatus()
        return HelperSnapshot(
            helper: .init(state: "running", message: message ?? helperMessage),
            permissions: .init(
                microphone: Self.microphonePermissionState(),
                speech: Self.speechPermissionState(speechStatus)
            ),
            owner: owner,
            activity: activity,
            localeTag: currentLocaleTag(),
            recognizerAvailable: recognizerAvailability(for: activeLanguage),
            recognition: recognitionSnapshot,
            playback: playbackSnapshot,
            voices: availableVoices(root: storageRoot),
            models: await models.snapshots()
        )
    }

    func publishSnapshotChanged() async {
        await writer.writeEnvelope(OutputEnvelope(
            id: nil,
            type: "event",
            result: nil,
            event: HelperEvent(type: "snapshot_changed", snapshot: await snapshot(message: nil), owner: nil, progress: nil, model: nil, state: nil, error: nil, message: nil),
            error: nil
        ))
    }

    private func publishOwnerChanged() async {
        await writer.writeEnvelope(OutputEnvelope(
            id: nil,
            type: "event",
            result: nil,
            event: HelperEvent(type: "owner_changed", snapshot: await snapshot(message: nil), owner: owner, progress: nil, model: nil, state: nil, error: nil, message: nil),
            error: nil
        ))
    }

    private func publishRecognitionProgress(owner: HelperOwner, text: String, isFinal: Bool) async {
        await writer.writeEnvelope(OutputEnvelope(
            id: nil,
            type: "event",
            result: nil,
            event: HelperEvent(
                type: "recognition_state",
                snapshot: await snapshot(message: nil),
                owner: nil,
                progress: .init(owner: owner, text: text, isFinal: isFinal),
                model: nil,
                state: nil,
                error: nil,
                message: nil
            ),
            error: nil
        ))
    }

    private func publishSpeechState(owner: HelperOwner, state: String) async {
        await writer.writeEnvelope(OutputEnvelope(
            id: nil,
            type: "event",
            result: nil,
            event: HelperEvent(type: "speech_state", snapshot: await snapshot(message: nil), owner: owner, progress: nil, model: nil, state: state, error: nil, message: nil),
            error: nil
        ))
    }

    private func publishError(_ error: HelperError, owner: HelperOwner?) async {
        await writer.writeEnvelope(OutputEnvelope(
            id: nil,
            type: "event",
            result: nil,
            event: HelperEvent(
                type: "error",
                snapshot: await snapshot(message: error.message),
                owner: owner,
                progress: nil,
                model: nil,
                state: nil,
                error: .init(code: error.code, message: error.message),
                message: nil
            ),
            error: nil
        ))
    }

    func handleCommand(id: String, input: [String: Any]) async {
        do {
            let commandType = input["type"] as? String ?? ""
            let result = try await dispatchCommand(input, commandType: commandType)
            await writer.writeEnvelope(OutputEnvelope(id: id, type: "response", result: .response(result), event: nil, error: nil))
        } catch let error as HelperError {
            await writer.writeEnvelope(OutputEnvelope(
                id: id,
                type: "response",
                result: .response(HelperResponse(type: "error", snapshot: await snapshot(message: error.message), transcript: nil, recording: nil, models: nil, model: nil, error: .init(code: error.code, message: error.message))),
                event: nil,
                error: nil
            ))
            await publishError(error, owner: parseOwner(input["owner"]))
        } catch {
            let helperError = HelperError.native(error.localizedDescription)
            await writer.writeEnvelope(OutputEnvelope(
                id: id,
                type: "response",
                result: .response(HelperResponse(type: "error", snapshot: await snapshot(message: helperError.message), transcript: nil, recording: nil, models: nil, model: nil, error: .init(code: helperError.code, message: helperError.message))),
                event: nil,
                error: nil
            ))
            await publishError(helperError, owner: parseOwner(input["owner"]))
        }
    }

    func handleEngineRequest(id: String, owner: HelperOwner, op: [String: Any]) async {
        do {
            let result = try await executeEngine(owner: owner, op: op)
            await writer.writeEnvelope(OutputEnvelope(
                id: id,
                type: "response",
                result: .engine(HelperEngineResponse(snapshot: await snapshot(message: nil), result: result)),
                event: nil,
                error: nil
            ))
        } catch let error as HelperError {
            await writer.writeEnvelope(OutputEnvelope(
                id: id,
                type: "response",
                result: .engine(HelperEngineResponse(snapshot: await snapshot(message: error.message), result: .failed(kind: Self.errorKind(for: error), message: error.message))),
                event: nil,
                error: nil
            ))
            await publishError(error, owner: owner)
        } catch {
            let helperError = HelperError.native(error.localizedDescription)
            await writer.writeEnvelope(OutputEnvelope(
                id: id,
                type: "response",
                result: .engine(HelperEngineResponse(snapshot: await snapshot(message: helperError.message), result: .failed(kind: "other", message: helperError.message))),
                event: nil,
                error: nil
            ))
            await publishError(helperError, owner: owner)
        }
    }

    private func dispatchCommand(_ input: [String: Any], commandType: String) async throws -> HelperResponse {
        switch commandType {
        case "get_snapshot":
            return HelperResponse(type: "snapshot", snapshot: await snapshot(message: nil), transcript: nil, recording: nil, models: nil, model: nil, error: nil)
        case "request_authorization":
            let permissions = input["permissions"] as? [String] ?? []
            try await requestPermissions(permissions)
            return HelperResponse(type: "authorization", snapshot: await snapshot(message: nil), transcript: nil, recording: nil, models: nil, model: nil, error: nil)
        case "list_models":
            return HelperResponse(type: "models", snapshot: await snapshot(message: nil), transcript: nil, recording: nil, models: await models.snapshots(), model: nil, error: nil)
        case "install_model":
            guard let modelID = input["modelId"] as? String else { throw HelperError.invalidRequest("missing modelId") }
            try await models.install(modelID: modelID)
            guard let model = await models.snapshots().first(where: { $0.modelId == modelID }) else {
                throw HelperError.invalidRequest("unknown model id \(modelID)")
            }
            return HelperResponse(type: "model_operation", snapshot: await snapshot(message: nil), transcript: nil, recording: nil, models: nil, model: model, error: nil)
        case "cancel_model":
            guard let modelID = input["modelId"] as? String else { throw HelperError.invalidRequest("missing modelId") }
            await models.cancel(modelID: modelID)
            guard let model = await models.snapshots().first(where: { $0.modelId == modelID }) else {
                throw HelperError.invalidRequest("unknown model id \(modelID)")
            }
            return HelperResponse(type: "model_operation", snapshot: await snapshot(message: nil), transcript: nil, recording: nil, models: nil, model: model, error: nil)
        case "remove_model":
            guard let modelID = input["modelId"] as? String else { throw HelperError.invalidRequest("missing modelId") }
            try await models.remove(modelID: modelID)
            return HelperResponse(type: "model_operation", snapshot: await snapshot(message: nil), transcript: nil, recording: nil, models: nil, model: HelperModelSnapshot(modelId: modelID, state: .notInstalled), error: nil)
        case "start_listening":
            guard let owner = parseOwner(input["owner"]),
                  let recognitionMode = input["recognitionMode"] as? String else {
                throw HelperError.invalidRequest("missing listening owner or recognitionMode")
            }
            try await startListening(
                owner: owner,
                recognitionMode: recognitionMode,
                language: resolvedLanguage(input["language"] as? String),
                sampleRate: input["sampleRateHz"] as? Int ?? 16_000,
                wantsRecording: owner.kind == "dictation" || owner.kind == "flow"
            )
            return HelperResponse(type: "listening_started", snapshot: await snapshot(message: nil), transcript: nil, recording: nil, models: nil, model: nil, error: nil)
        case "finish_listening":
            guard let owner = parseOwner(input["owner"]) else { throw HelperError.invalidRequest("missing listening owner") }
            let result = try await finishListening(owner: owner, emitFinalEvent: true, includeRecordingPayload: false)
            return HelperResponse(type: "listening_finished", snapshot: await snapshot(message: nil), transcript: result.transcript, recording: result.recording, models: nil, model: nil, error: nil)
        case "cancel":
            guard let owner = parseOwner(input["owner"]) else { throw HelperError.invalidRequest("missing owner") }
            try await cancel(owner: owner)
            return HelperResponse(type: "cancelled", snapshot: await snapshot(message: nil), transcript: nil, recording: nil, models: nil, model: nil, error: nil)
        case "speak":
            guard let owner = parseOwner(input["owner"]),
                  let text = input["text"] as? String else { throw HelperError.invalidRequest("missing speech owner or text") }
            try await startSpeaking(owner: owner, text: text, voiceID: input["voiceId"] as? String, rate: input["rate"] as? Double ?? 1)
            return HelperResponse(type: "speaking_started", snapshot: await snapshot(message: nil), transcript: nil, recording: nil, models: nil, model: nil, error: nil)
        case "stop_speaking":
            guard let owner = parseOwner(input["owner"]) else { throw HelperError.invalidRequest("missing owner") }
            try await stopSpeaking(owner: owner)
            return HelperResponse(type: "speaking_stopped", snapshot: await snapshot(message: nil), transcript: nil, recording: nil, models: nil, model: nil, error: nil)
        default:
            throw HelperError.invalidRequest("unsupported command type \(commandType)")
        }
    }

    private func executeEngine(owner: HelperOwner, op: [String: Any]) async throws -> HelperEngineResult {
        switch op["type"] as? String ?? "" {
        case "is_recording":
            return .recordingState(listeningOwner == owner)
        case "start_recording":
            let format = op["format"] as? String ?? "wav"
            if format != "wav" && format != "m4a" {
                throw HelperError.invalidRequest("unsupported recording format \(format)")
            }
            engineRecordingFormatByOwner[owner.id] = format
            do {
                try await startListening(owner: owner, recognitionMode: "automatic", language: activeLanguage, sampleRate: op["sample_rate_hz"] as? Int ?? 16_000, wantsRecording: true)
            } catch {
                engineRecordingFormatByOwner.removeValue(forKey: owner.id)
                throw error
            }
            return .ok
        case "stop_recording":
            let result = try await finishListening(owner: owner, emitFinalEvent: false, includeRecordingPayload: true)
            guard let recording = result.recording else {
                throw HelperError.unavailable("the recorded clip could not be encoded")
            }
            lastRecordingByOwner[owner.id] = (result.samples, result.sampleRate, recording, result.transcript)
            return .recording(audioBase64: recording.audioBase64, mimeType: recording.mimeType)
        case "transcribe":
            guard let cached = lastRecordingByOwner[owner.id] else {
                throw HelperError.unavailable("there is no captured recording to transcribe")
            }
            if let transcript = cached.transcript,
               !transcript.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                return .transcript(text: transcript.text, language: transcript.language, confidence: transcript.confidence)
            }
            let language = resolvedLanguage(op["language"] as? String)
            let transcript = try SherpaRuntime.transcribe(samples: cached.samples, sampleRate: cached.sampleRate, languageIdentifier: language, root: storageRoot)
            return .transcript(text: transcript.text, language: transcript.language, confidence: transcript.confidence)
        case "synthesize":
            guard let text = op["text"] as? String else { throw HelperError.invalidRequest("missing synthesis text") }
            let resolved = try resolvePlaybackVoice(op["voice"] as? String, language: activeLanguage, root: storageRoot)
            switch resolved {
            case let .system(systemVoice, _):
                let session = await MainActor.run { PlaybackSession() }
                try await session.playSystem(text: text, voice: systemVoice, rate: AVSpeechUtteranceDefaultSpeechRate)
                // Engine synthesis permits the established "played in place"
                // success pair when a system API cannot export raw PCM.
                return .audio(pcmBase64: "", sampleRateHz: 0)
            case let .sherpa(voiceID, _):
                let rendered = try SherpaRuntime.synthesize(text: text, voiceID: voiceID, languageIdentifier: activeLanguage, rate: 1, root: storageRoot)
                let base64 = rendered.0.base64EncodedString()
                try ensureInlineAudioFits(base64, label: "synthesized")
                return .audio(pcmBase64: base64, sampleRateHz: rendered.1)
            }
        default:
            throw HelperError.invalidRequest("unsupported audio op \(op["type"] as? String ?? "")")
        }
    }

    private func requestPermissions(_ permissions: [String]) async throws {
        let requested = Set(permissions)
        guard requested.isSubset(of: ["microphone", "speech"]) else {
            throw HelperError.invalidRequest("unsupported audio permission")
        }
        if requested.contains("speech"), SFSpeechRecognizer.authorizationStatus() == .notDetermined {
            _ = await withCheckedContinuation { continuation in
                SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
            }
        }
        if requested.contains("microphone"), AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
            _ = await withCheckedContinuation { continuation in
                AVCaptureDevice.requestAccess(for: .audio) { continuation.resume(returning: $0) }
            }
        }
        await publishSnapshotChanged()
    }

    private func startListening(
        owner nextOwner: HelperOwner,
        recognitionMode: String,
        language: String,
        sampleRate: Int,
        wantsRecording: Bool
    ) async throws {
        if let owner, owner != nextOwner {
            throw HelperError.busy("audio is already owned by \(owner.kind)")
        }
        guard (8_000...48_000).contains(sampleRate) else {
            throw HelperError.invalidRequest("sample rate must be between 8000 and 48000 Hz")
        }
        if AVCaptureDevice.authorizationStatus(for: .audio) != .authorized {
            try await requestPermissions(["microphone"])
            if AVCaptureDevice.authorizationStatus(for: .audio) != .authorized {
                throw HelperError.permission("microphone access is required")
            }
        }
        if recognitionMode == "automatic",
           nextOwner.kind != "engine",
           SFSpeechRecognizer.authorizationStatus() == .notDetermined {
            try await requestPermissions(["speech"])
        }
        activeLanguage = language
        let systemAvailable = recognitionMode == "automatic"
            && SFSpeechRecognizer.authorizationStatus() == .authorized
            && recognizerAvailability(for: language)
        let route: ListeningSession.Route
        if systemAvailable {
            route = .system
            recognitionSnapshot = .init(
                requestedMode: recognitionMode,
                effectiveBackend: "apple",
                effectiveLanguage: language,
                detail: "Using Apple Speech on this Mac.",
                fallbackReason: nil
            )
        } else if SherpaRuntime.recognitionModel(for: language, root: storageRoot) != nil {
            route = .sherpa
            recognitionSnapshot = .init(
                requestedMode: recognitionMode,
                effectiveBackend: "sherpa",
                effectiveLanguage: language,
                detail: "Using installed Sherpa offline speech recognition.",
                fallbackReason: recognitionMode == "automatic" ? "Apple Speech was unavailable, so the helper fell back to Sherpa." : nil
            )
        } else if nextOwner.kind == "engine" {
            route = .captureOnly
            recognitionSnapshot = .init(
                requestedMode: recognitionMode,
                effectiveBackend: "unavailable",
                effectiveLanguage: language,
                detail: "Recording is available; transcription requires Apple Speech or a matching offline model.",
                fallbackReason: recognitionMode == "automatic" ? "Apple Speech was unavailable and no installed Sherpa model matched the language." : nil
            )
        } else {
            recognitionSnapshot = .init(
                requestedMode: recognitionMode,
                effectiveBackend: "unavailable",
                effectiveLanguage: language,
                detail: "No matching speech recognition backend is available.",
                fallbackReason: recognitionMode == "automatic" ? "Apple Speech was unavailable and no installed Sherpa model matched the language." : nil
            )
            throw recognitionMode == "localOnly"
                ? HelperError.modelMissing("an offline recognition model is required for \(languageBase(language))")
                : HelperError.unavailable("no system recognizer or offline model is available for \(languageBase(language))")
        }
        let session = await MainActor.run {
            ListeningSession(
                owner: nextOwner,
                languageIdentifier: language,
                route: route,
                outputSampleRate: sampleRate,
                wantsRecording: wantsRecording,
                onPartial: { [weak self] transcript in
                    await self?.publishRecognitionProgress(owner: nextOwner, text: transcript.text, isFinal: false)
                },
                onSilence: { [weak self] in
                    guard nextOwner.kind == "flow" else { return }
                    _ = try? await self?.finishListening(owner: nextOwner, emitFinalEvent: true, includeRecordingPayload: false)
                }
            )
        }
        try await MainActor.run { try session.start() }
        self.owner = nextOwner
        listeningOwner = nextOwner
        activity = "listening"
        activeSession = session
        await publishOwnerChanged()
        await publishSnapshotChanged()
    }

    private func finishListening(
        owner expectedOwner: HelperOwner,
        emitFinalEvent: Bool,
        includeRecordingPayload: Bool
    ) async throws -> (transcript: HelperTranscript?, recording: HelperRecording?, samples: [Float], sampleRate: Int) {
        guard listeningOwner == expectedOwner, let activeSession else {
            throw HelperError.cancelled("the requested listening owner is not active")
        }
        activity = "recognizing"
        listeningOwner = nil
        owner = nil
        let result = await MainActor.run { activeSession.stop() }
        self.activeSession = nil
        let samples = result.0
        let systemTranscript = result.1
        let sampleRate = result.2
        let recognitionFailure = result.3
        let recordingFormat = includeRecordingPayload
            ? engineRecordingFormatByOwner.removeValue(forKey: expectedOwner.id) ?? "wav"
            : "wav"
        do {
            let transcript: HelperTranscript?
            if let systemTranscript {
                transcript = systemTranscript
            } else if emitFinalEvent,
                      !samples.isEmpty,
                      recognitionSnapshot?.effectiveBackend == "sherpa" {
                transcript = try SherpaRuntime.transcribe(
                    samples: samples,
                    sampleRate: sampleRate,
                    languageIdentifier: activeLanguage,
                    root: storageRoot
                )
            } else if emitFinalEvent,
                      let recognitionFailure,
                      !samples.isEmpty,
                      SherpaRuntime.recognitionModel(for: activeLanguage, root: storageRoot) != nil {
                transcript = try SherpaRuntime.transcribe(
                    samples: samples,
                    sampleRate: sampleRate,
                    languageIdentifier: activeLanguage,
                    root: storageRoot
                )
                recognitionSnapshot = .init(
                    requestedMode: "automatic",
                    effectiveBackend: "sherpa",
                    effectiveLanguage: activeLanguage,
                    detail: "Apple Speech failed during recognition; using installed Sherpa offline speech recognition.",
                    fallbackReason: recognitionFailure
                )
            } else if emitFinalEvent, let recognitionFailure {
                throw HelperError.unavailable("Apple Speech failed: \(recognitionFailure); no matching offline model is installed")
            } else {
                transcript = nil
            }

            let recording: HelperRecording?
            if includeRecordingPayload && !samples.isEmpty {
                let payload = try recordingPayload(samples: samples, sampleRate: sampleRate, format: recordingFormat)
                try ensureInlineAudioFits(payload.audioBase64, label: "recording")
                recording = payload
            } else {
                recording = nil
            }
            activity = "idle"
            await publishOwnerChanged()
            if let transcript, emitFinalEvent {
                await publishRecognitionProgress(owner: expectedOwner, text: transcript.text, isFinal: true)
            }
            await publishSnapshotChanged()
            return (transcript, recording, samples, sampleRate)
        } catch {
            activity = "idle"
            await publishOwnerChanged()
            await publishSnapshotChanged()
            throw error
        }
    }

    private func cancel(owner expectedOwner: HelperOwner) async throws {
        if listeningOwner == expectedOwner, let activeSession {
            _ = await MainActor.run { activeSession.stop() }
            self.activeSession = nil
            listeningOwner = nil
            owner = nil
            engineRecordingFormatByOwner.removeValue(forKey: expectedOwner.id)
            activity = "idle"
            await publishOwnerChanged()
            await publishSnapshotChanged()
            return
        }
        if owner == expectedOwner {
            speechTask?.cancel()
            speechTask = nil
            if let activeSpeech {
                await MainActor.run { activeSpeech.stop() }
            }
            self.activeSpeech = nil
            self.owner = nil
            activity = "idle"
            await publishOwnerChanged()
            await publishSnapshotChanged()
            return
        }
        throw HelperError.cancelled("the requested owner is not active")
    }

    private func startSpeaking(owner nextOwner: HelperOwner, text: String, voiceID: String?, rate: Double) async throws {
        if let owner, owner != nextOwner {
            throw HelperError.busy("audio is already owned by \(owner.kind)")
        }
        let voice = try resolvePlaybackVoice(voiceID, language: activeLanguage, root: storageRoot)
        playbackSnapshot = voice.snapshot
        owner = nextOwner
        activity = "speaking"
        let session = await MainActor.run { PlaybackSession() }
        activeSpeech = session
        await publishOwnerChanged()
        await publishSnapshotChanged()
        await publishSpeechState(owner: nextOwner, state: "starting")
        speechTask?.cancel()
        speechTask = Task { [weak self] in
            guard let self else { return }
            do {
                await self.publishSpeechState(owner: nextOwner, state: "speaking")
                switch voice {
                case let .system(systemVoice, _):
                    try await session.playSystem(text: text, voice: systemVoice, rate: Float(max(0.5, min(2, rate))) * 0.28)
                case let .sherpa(renderVoiceID, _):
                    let rendered = try await self.renderSpeech(text: text, voiceID: renderVoiceID, rate: rate)
                    try await session.playPCM16(rendered.pcm16, sampleRate: rendered.sampleRate)
                }
                await self.finishSpeech(owner: nextOwner, interrupted: false)
            } catch {
                let helperError = (error as? HelperError) ?? HelperError.native(error.localizedDescription)
                if helperError.code == "cancelled" {
                    await self.finishSpeech(owner: nextOwner, interrupted: true)
                } else {
                    await self.failSpeech(owner: nextOwner, error: helperError)
                }
            }
        }
    }

    private func finishSpeech(owner expectedOwner: HelperOwner, interrupted: Bool) async {
        speechTask = nil
        activeSpeech = nil
        if owner == expectedOwner {
            owner = nil
            activity = "idle"
        }
        await publishSpeechState(owner: expectedOwner, state: interrupted ? "interrupted" : "finished")
        await publishOwnerChanged()
        await publishSnapshotChanged()
    }

    private func failSpeech(owner expectedOwner: HelperOwner, error: HelperError) async {
        helperMessage = error.message
        await publishError(error, owner: expectedOwner)
        await finishSpeech(owner: expectedOwner, interrupted: true)
    }

    private func stopSpeaking(owner expectedOwner: HelperOwner) async throws {
        guard owner == expectedOwner else {
            throw HelperError.cancelled("the requested owner is not active")
        }
        speechTask?.cancel()
        speechTask = nil
        if let activeSpeech {
            await MainActor.run { activeSpeech.stop() }
        }
        self.activeSpeech = nil
        self.owner = nil
        activity = "idle"
        await publishSpeechState(owner: expectedOwner, state: "interrupted")
        await publishOwnerChanged()
        await publishSnapshotChanged()
    }

    private func renderSpeech(text: String, voiceID: String?, rate: Double) async throws -> (pcm16: Data, sampleRate: Int32) {
        let resolved = try resolvePlaybackVoice(voiceID, language: activeLanguage, root: storageRoot)
        switch resolved {
        case .sherpa(let effectiveVoiceID, _):
            let rendered = try SherpaRuntime.synthesize(text: text, voiceID: effectiveVoiceID, languageIdentifier: activeLanguage, rate: rate, root: storageRoot)
            return (rendered.0, rendered.1)
        case .system:
            throw HelperError.unavailable("system voice synthesis is available for playback but not yet exposed as raw PCM")
        }
    }

    private func parseOwner(_ value: Any?) -> HelperOwner? {
        guard let input = value as? [String: Any],
              let kind = input["kind"] as? String,
              let id = input["id"] as? String else {
            return nil
        }
        return HelperOwner(kind: kind, id: id)
    }

    private static func microphonePermissionState() -> String {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: return "granted"
        case .denied, .restricted: return "denied"
        case .notDetermined: return "prompt"
        @unknown default: return "unavailable"
        }
    }

    private static func speechPermissionState(_ status: SFSpeechRecognizerAuthorizationStatus) -> String {
        switch status {
        case .authorized: return "authorized"
        case .denied: return "denied"
        case .restricted: return "restricted"
        case .notDetermined: return "not_determined"
        @unknown default: return "unavailable"
        }
    }

    private static func errorKind(for error: HelperError) -> String {
        switch error {
        case .permission:
            return "permission_denied"
        case .busy:
            return "busy"
        case .cancelled:
            return "retriable"
        case .modelMissing, .unavailable:
            return "unavailable"
        case .download, .checksum, .invalidRequest, .native:
            return "other"
        }
    }
}

func readInputLoop(state: HelperStateStore) async {
    do {
        for try await line in FileHandle.standardInput.bytes.lines {
            guard let data = line.data(using: .utf8), !data.isEmpty else { continue }
            let parsed = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            let id = parsed?["id"] as? String ?? UUID().uuidString
            let kind = parsed?["kind"] as? String ?? "command"
            if kind == "engine_request" {
                let owner = (parsed?["owner"] as? [String: Any]).flatMap { HelperOwner(kind: $0["kind"] as? String ?? "", id: $0["id"] as? String ?? "") }
                let op = parsed?["op"] as? [String: Any] ?? [:]
                if let owner {
                    await state.handleEngineRequest(id: id, owner: owner, op: op)
                }
                continue
            }
            let command = parsed?["command"] as? [String: Any] ?? [:]
            await state.handleCommand(id: id, input: command)
        }
    } catch {
        return
    }
}

@main
struct LingXiAudioHelperApp {
    static func main() async {
        let rootPath = ProcessInfo.processInfo.environment["LINGXI_AUDIO_MODELS_ROOT"]
            ?? NSHomeDirectory().appending("/Library/Application Support/LingXi/voice-models")
        let storageRoot = URL(fileURLWithPath: rootPath, isDirectory: true)
        let writer = LineWriter()
        let state = HelperStateStore(storageRoot: storageRoot, writer: writer)
        await writer.writeEnvelope(OutputEnvelope(
            id: nil,
            type: "event",
            result: nil,
            event: HelperEvent(type: "helper_state", snapshot: await state.snapshot(message: nil), owner: nil, progress: nil, model: nil, state: "running", error: nil, message: nil),
            error: nil
        ))
        await readInputLoop(state: state)
    }
}
