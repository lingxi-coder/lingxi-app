@preconcurrency import AVFoundation
@preconcurrency import AppKit
import AudioToolbox
import CryptoKit
import Foundation
@preconcurrency import Speech
import SherpaOnnx

private let maxLineBytes = 16 * 1024 * 1024
private let maxInlineAudioBase64Bytes = maxLineBytes - (64 * 1024)
private let defaultLanguage = "en-US"
private let silenceThreshold: Float = 0.015
private let silenceDuration: TimeInterval = 1.2
private let noSpeechTimeout: TimeInterval = 15
private let speechFinalResultTimeoutNanoseconds: UInt64 = 1_000_000_000
private let recordingInitialCaptureTimeoutNanoseconds: UInt64 = 500_000_000

enum HelperError: Error, Sendable {
    case invalidRequest(String)
    case unavailable(String)
    case busy(String)
    case cancelled(String)
    case permission(String)
    case timeout(String)
    case noSpeech(String)
    case notRecording(String)
    case unsupported(String)
    case modelMissing(String)
    case voiceMissing(String)
    case synthesisFailed(String)
    case mediaTooLarge(String)
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
        case .timeout: return "timeout"
        case .noSpeech: return "no-speech"
        case .notRecording: return "not-recording"
        case .unsupported: return "unsupported"
        case .modelMissing: return "model-missing"
        case .voiceMissing: return "voice-missing"
        case .synthesisFailed: return "synthesis-failed"
        case .mediaTooLarge: return "media-too-large"
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
             let .timeout(message),
             let .noSpeech(message),
             let .notRecording(message),
             let .unsupported(message),
             let .modelMissing(message),
             let .voiceMissing(message),
             let .synthesisFailed(message),
             let .mediaTooLarge(message),
             let .download(message),
             let .checksum(message),
             let .native(message):
            return message
        }
    }
}

func permissionRequestFromArguments(_ arguments: [String]) throws -> Set<String>? {
    guard let flagIndex = arguments.firstIndex(of: "--request-permissions") else { return nil }
    guard arguments.indices.contains(flagIndex + 1) else {
        throw HelperError.invalidRequest("missing audio permission list")
    }
    let requested = Set(arguments[flagIndex + 1]
        .split(separator: ",")
        .map(String.init)
        .filter { !$0.isEmpty })
    guard !requested.isEmpty, requested.isSubset(of: ["microphone", "speech"]) else {
        throw HelperError.invalidRequest("unsupported audio permission")
    }
    return requested
}

@MainActor
func requestSystemPermissions(_ requested: Set<String>) async throws {
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
}

@MainActor
func requestForegroundSystemPermissions(_ requested: Set<String>) async throws {
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    app.activate(ignoringOtherApps: true)
    try await requestSystemPermissions(requested)
}

struct HelperOwner: Codable, Equatable, Sendable {
    let kind: String
    let id: String
}

struct HelperAudioOperationIdentity: Codable, Equatable, Hashable, Sendable {
    let id: String
    let generation: UInt64
    let serviceEpoch: UInt64

    enum CodingKeys: String, CodingKey {
        case id, generation
        case serviceEpoch = "service_epoch"
    }
}

final class AudioCancellationFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var failure: HelperError?
    private var handlers: [UUID: @Sendable () -> Void] = [:]

    var isCancelled: Bool {
        lock.lock(); defer { lock.unlock() }
        return failure != nil
    }

    func cancel(with error: HelperError = .cancelled("the audio operation was cancelled")) {
        lock.lock()
        guard failure == nil else { lock.unlock(); return }
        failure = error
        let callbacks = Array(handlers.values)
        handlers.removeAll()
        lock.unlock()
        callbacks.forEach { $0() }
    }

    func check() throws {
        lock.lock(); let terminal = failure; lock.unlock()
        if let terminal { throw terminal }
        if Task.isCancelled { throw HelperError.cancelled("the audio operation was cancelled") }
    }

    func terminalError() -> HelperError? {
        lock.lock(); defer { lock.unlock() }
        return failure
    }

    @discardableResult
    func installCancellationHandler(_ handler: @escaping @Sendable () -> Void) -> UUID {
        lock.lock()
        let token = UUID()
        let alreadyCancelled = failure != nil
        if !alreadyCancelled { handlers[token] = handler }
        lock.unlock()
        if alreadyCancelled { handler() }
        return token
    }

    func removeCancellationHandler(_ token: UUID) {
        lock.lock(); defer { lock.unlock() }
        handlers.removeValue(forKey: token)
    }
}

struct HelperAudioOwner: Codable, Equatable, Sendable {
    let type: String
    let sessionID: String?
    let appID: String?
    let runtimeGeneration: UInt64?
    let instanceID: String?

    enum CodingKeys: String, CodingKey {
        case type
        case sessionID = "session_id"
        case appID = "app_id"
        case runtimeGeneration = "runtime_generation"
        case instanceID = "instance_id"
    }

    var helperOwner: HelperOwner {
        switch type {
        case "session": return HelperOwner(kind: type, id: sessionID ?? "")
        case "local_app": return HelperOwner(kind: type, id: "\(appID ?? ""):\(runtimeGeneration ?? 0)")
        default: return HelperOwner(kind: type, id: instanceID ?? "")
        }
    }
}

func parseHelperAudioIdentity(_ value: [String: Any]) -> HelperAudioOperationIdentity? {
    guard let id = value["id"] as? String,
          let generation = (value["generation"] as? NSNumber)?.uint64Value,
          let serviceEpoch = (value["service_epoch"] as? NSNumber)?.uint64Value else { return nil }
    return HelperAudioOperationIdentity(id: id, generation: generation, serviceEpoch: serviceEpoch)
}

func helperAudioIdentityKey(_ identity: HelperAudioOperationIdentity) -> String {
    "\(identity.serviceEpoch):\(identity.generation):\(identity.id)"
}

func helperAudioOwnerKey(_ owner: HelperAudioOwner) -> String {
    switch owner.type {
    case "session": return "session:\(owner.sessionID ?? "")"
    case "local_app": return "local_app:\(owner.appID ?? ""):\(owner.runtimeGeneration ?? 0)"
    default: return "\(owner.type):\(owner.instanceID ?? "")"
    }
}

func helperAudioOwnerKey(_ owner: HelperOwner) -> String {
    "\(owner.kind):\(owner.id)"
}

func parseHelperAudioOwner(_ value: [String: Any]) -> HelperAudioOwner? {
    guard let type = value["type"] as? String else { return nil }
    switch type {
    case "session":
        guard let sessionID = value["session_id"] as? String else { return nil }
        return HelperAudioOwner(type: type, sessionID: sessionID, appID: nil, runtimeGeneration: nil, instanceID: nil)
    case "local_app":
        guard let appID = value["app_id"] as? String,
              let generation = (value["runtime_generation"] as? NSNumber)?.uint64Value else { return nil }
        return HelperAudioOwner(type: type, sessionID: nil, appID: appID, runtimeGeneration: generation, instanceID: nil)
    case "ui", "system":
        guard let instanceID = value["instance_id"] as? String else { return nil }
        return HelperAudioOwner(type: type, sessionID: nil, appID: nil, runtimeGeneration: nil, instanceID: instanceID)
    default:
        return nil
    }
}

struct HelperAudioCurrentOperation: Encodable, Sendable {
    let identity: HelperAudioOperationIdentity
    let owner: HelperAudioOwner
}

struct HelperAudioOperationTrace: Encodable, Sendable {
    let identity: HelperAudioOperationIdentity
    let owner: HelperAudioOwner
    let operation: String
    let configurationRevision: UInt64
    let requestedSource: String?
    let requestedModelId: String?
    let requestedVoiceId: String?
    let effectiveSource: String?
    let effectiveModelId: String?
    let effectiveVoiceId: String?
    let fallbackReason: String?
}

struct HelperAudioReadiness: Encodable, Sendable {
    let operation: String
    let state: String
}

struct HelperAudioCapabilities: Encodable, Sendable {
    let serviceEpoch: UInt64
    let supportRevision: UInt64
    let supportedOperations: [String]
    let readiness: [HelperAudioReadiness]
    let maxPayloadBytes: UInt64

    enum CodingKeys: String, CodingKey {
        case serviceEpoch = "service_epoch"
        case supportRevision = "support_revision"
        case supportedOperations = "supported_operations"
        case readiness
        case maxPayloadBytes = "max_payload_bytes"
    }
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
    let capabilities: HelperAudioCapabilities?
    let configurationRevision: UInt64?
    let currentOperation: HelperAudioCurrentOperation?
    var audioOperations: [HelperAudioOperationTrace] = []
    var activeOperationCount: Int = 0
    var pendingOperationCount: Int = 0
    var activeRecordingCount: Int = 0
    var activePlaybackCount: Int = 0
    var activeModelReferenceCount: Int = 0
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
    let snapshot: HelperSnapshot?
    let owner: HelperOwner?
    let progress: HelperRecognitionProgress?
    let model: HelperModelSnapshot?
    let state: String?
    let error: HelperErrorPayload?
    let message: String?
    let level: Double?

    init(
        type: String,
        snapshot: HelperSnapshot?,
        owner: HelperOwner?,
        progress: HelperRecognitionProgress?,
        model: HelperModelSnapshot?,
        state: String?,
        error: HelperErrorPayload?,
        message: String?,
        level: Double? = nil
    ) {
        self.type = type
        self.snapshot = snapshot
        self.owner = owner
        self.progress = progress
        self.model = model
        self.state = state
        self.error = error
        self.message = message
        self.level = level
    }
}

enum HelperEngineResult: Sendable {
    case recordingStarted(String)
    case recording(audioBase64: String, mimeType: String)
    case transcript(text: String, language: String?, confidence: Double?)
    case synthesized(pcmBase64: String, sampleRateHz: UInt32)
    case playbackCompleted(durationMs: UInt64)
    case status(recording: Bool, playing: Bool)
    case ownerEnded
    case failed(kind: String, message: String)
}

extension HelperEngineResult: Encodable {
    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .recordingStarted(handle):
            try container.encode("recording_started", forKey: .type)
            try container.encode(handle, forKey: .handle)
        case let .recording(audioBase64, mimeType):
            try container.encode("recording", forKey: .type)
            try container.encode(audioBase64, forKey: .audioBase64)
            try container.encode(mimeType, forKey: .mimeType)
        case let .transcript(text, language, confidence):
            try container.encode("transcript", forKey: .type)
            try container.encode(text, forKey: .text)
            try container.encodeIfPresent(language, forKey: .language)
            try container.encodeIfPresent(confidence, forKey: .confidence)
        case let .synthesized(pcmBase64, sampleRateHz):
            try container.encode("synthesized", forKey: .type)
            try container.encode(pcmBase64, forKey: .pcmBase64)
            try container.encode(sampleRateHz, forKey: .sampleRateHz)
        case let .playbackCompleted(durationMs):
            try container.encode("playback_completed", forKey: .type)
            try container.encode(durationMs, forKey: .durationMs)
        case let .status(recording, playing):
            try container.encode("status", forKey: .type)
            try container.encode(HelperAudioStatus(recording: recording, playing: playing), forKey: .status)
        case .ownerEnded:
            try container.encode("owner_ended", forKey: .type)
        case let .failed(kind, message):
            try container.encode("failed", forKey: .type)
            try container.encode(HelperAudioError(kind: kind, message: message), forKey: .error)
        }
    }

    private enum CodingKeys: String, CodingKey {
        case type
        case handle
        case audioBase64 = "audio_base64"
        case mimeType = "mime_type"
        case text
        case language
        case confidence
        case pcmBase64 = "pcm_base64"
        case sampleRateHz = "sample_rate_hz"
        case durationMs = "duration_ms"
        case status, error
    }
}

struct HelperAudioStatus: Encodable, Sendable {
    let recording: Bool
    let playing: Bool
}

struct HelperAudioError: Encodable, Sendable {
    let kind: String
    let message: String
}

struct HelperEngineResponse: Encodable, Sendable {
    let type = "engine_result"
    let snapshot: HelperSnapshot
    let result: HelperEngineResult
}

private final class AudioListenWaiter: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<HelperTranscript, Error>?
    private var terminal: Result<HelperTranscript, HelperError>?
    private var hasCompleted = false

    func value() async throws -> HelperTranscript {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                lock.lock()
                if let terminal {
                    lock.unlock()
                    switch terminal {
                    case let .success(transcript): continuation.resume(returning: transcript)
                    case let .failure(error): continuation.resume(throwing: error)
                    }
                } else {
                    self.continuation = continuation
                    lock.unlock()
                }
            }
        } onCancel: {
            self.complete(.failure(.cancelled("the live listening operation was cancelled")))
        }
    }

    func complete(_ result: Result<HelperTranscript, HelperError>) {
        lock.lock()
        guard !hasCompleted else { lock.unlock(); return }
        hasCompleted = true
        terminal = result
        let continuation = self.continuation
        self.continuation = nil
        lock.unlock()
        guard let continuation else { return }
        switch result {
        case let .success(transcript): continuation.resume(returning: transcript)
        case let .failure(error): continuation.resume(throwing: error)
        }
    }
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
    private let emit: @Sendable (Data) -> Void

    init(emit: @escaping @Sendable (Data) -> Void = { data in
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([0x0a]))
    }) {
        self.emit = emit
    }

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
                emit(fallback)
            }
            return
        }
        emit(data)
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

func isDefaultOrAutoVoiceOverride(_ value: Any?) -> Bool {
    guard let raw = value as? String else { return false }
    let normalized = raw.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    return normalized == "default" || normalized == "auto"
}

func speechPreferenceForSingleUtterance(
    _ preference: AudioSpeechPreference,
    voiceOverride: Any?
) -> AudioSpeechPreference {
    guard isDefaultOrAutoVoiceOverride(voiceOverride) else { return preference }
    return AudioSpeechPreference(
        source: preference.source,
        offlineModelId: preference.offlineModelId,
        voice: nil
    )
}

func base64PCM16Wave(samples: [Float], sampleRate: Int) -> String {
    waveData(pcm16: pcm16Data(from: samples), sampleRate: sampleRate).base64EncodedString()
}

func finalizeStoppedRecording<Value>(
    ownsCurrentLease: Bool,
    releaseLease: () -> Void,
    finalize: () throws -> Value
) rethrows -> Value {
    defer { if ownsCurrentLease { releaseLease() } }
    return try finalize()
}

func validateRecordingStart(recordingFailure: HelperError?, leaseIsActive: Bool) throws {
    if let recordingFailure { throw recordingFailure }
    guard leaseIsActive else {
        throw HelperError.cancelled("the recording capture stopped before it was ready")
    }
}

func validateRecordingStartAvailability(
    existingHandle: String?,
    hasRecordingOrigin: Bool,
    physicalAudioBusy: Bool
) throws {
    guard existingHandle == nil, !hasRecordingOrigin, !physicalAudioBusy else {
        throw HelperError.busy("the device audio resource is already in use")
    }
}

func admitRecordingStart(
    existingHandle: inout String?,
    existingOwner: inout HelperAudioOwner?,
    existingOrigin: inout HelperAudioOperationIdentity?,
    handle: String,
    owner: HelperAudioOwner,
    origin: HelperAudioOperationIdentity,
    physicalAudioBusy: Bool
) throws {
    try validateRecordingStartAvailability(
        existingHandle: existingHandle,
        hasRecordingOrigin: existingOrigin != nil,
        physicalAudioBusy: physicalAudioBusy
    )
    existingHandle = handle
    existingOwner = owner
    existingOrigin = origin
}

func validateRecordingHandle(
    requestedHandle: String,
    activeHandle: String?,
    activeOwner: HelperAudioOwner?,
    requestedOwner: HelperAudioOwner
) throws {
    guard requestedHandle == activeHandle,
          activeOwner.map(helperAudioOwnerKey) == helperAudioOwnerKey(requestedOwner) else {
        throw HelperError.notRecording("the recording handle is not active for this owner")
    }
}

func recordingOriginToRollback(
    for identity: HelperAudioOperationIdentity,
    operation: String?,
    owner operationOwner: HelperAudioOwner?,
    recordingOrigin: HelperAudioOperationIdentity?,
    recordingOwner: HelperAudioOwner?
) -> HelperAudioOperationIdentity? {
    guard let recordingOrigin else { return nil }
    if recordingOrigin == identity { return recordingOrigin }
    guard operation == "stop_recording",
          let operationOwner,
          let recordingOwner,
          helperAudioOwnerKey(operationOwner.helperOwner) == helperAudioOwnerKey(recordingOwner.helperOwner) else {
        return nil
    }
    return recordingOrigin
}

func recordingPayload(samples: [Float], sampleRate: Int, format: String, maximumPayloadBytes: Int) throws -> HelperRecording {
    guard maximumPayloadBytes > 0, samples.count <= maximumPayloadBytes / 2 else {
        throw HelperError.mediaTooLarge("captured audio exceeds the configured payload limit")
    }
    switch format {
    case "wav":
        guard samples.count <= max(0, (maximumPayloadBytes - 44) / 2) else {
            throw HelperError.mediaTooLarge("recording WAV exceeds the configured payload limit")
        }
        let data = waveData(pcm16: pcm16Data(from: samples), sampleRate: sampleRate)
        guard data.count <= maximumPayloadBytes else {
            throw HelperError.mediaTooLarge("recording WAV exceeds the configured payload limit")
        }
        return HelperRecording(
            audioBase64: data.base64EncodedString(),
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
        let metadata = try FileManager.default.attributesOfItem(atPath: url.path)
        guard let fileSize = (metadata[.size] as? NSNumber)?.intValue, fileSize <= maximumPayloadBytes else {
            throw HelperError.mediaTooLarge("recording audio exceeds the configured payload limit")
        }
        let audioData = try Data(contentsOf: url)
        guard audioData.count <= maximumPayloadBytes else {
            throw HelperError.mediaTooLarge("recording audio exceeds the configured payload limit")
        }
        return HelperRecording(
            audioBase64: audioData.base64EncodedString(),
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

final class PCM16SampleCollector: @unchecked Sendable {
    private let lock = NSLock()
    private let maximumBytes: Int
    private let cancellation: AudioCancellationFlag?
    private var storage = Data()
    private var failure: HelperError?
    private var cancelled = false

    init(maximumBytes: Int, cancellation: AudioCancellationFlag? = nil) {
        self.maximumBytes = max(0, maximumBytes)
        self.cancellation = cancellation
        storage.reserveCapacity(min(self.maximumBytes, 64 * 1024))
    }

    func append(samples: UnsafePointer<Float>?, count: Int) -> Int32 {
        guard let samples, count > 0 else { return 1 }
        lock.lock()
        defer { lock.unlock() }
        if cancelled || cancellation?.isCancelled == true {
            failure = .cancelled("audio synthesis was cancelled")
            return 0
        }
        guard count <= (maximumBytes - storage.count) / 2 else {
            failure = .mediaTooLarge("synthesized PCM exceeds the configured payload limit")
            return 0
        }
        let start = storage.count
        storage.count += count * 2
        storage.withUnsafeMutableBytes { raw in
            let output = raw.bindMemory(to: Int16.self)
            for index in 0..<count {
                let sample = max(-1, min(1, samples[index]))
                output[(start / 2) + index] = Int16((sample * 32767).rounded()).littleEndian
            }
        }
        return 1
    }

    func cancel() {
        lock.lock(); defer { lock.unlock() }
        cancelled = true
    }

    func result() -> (Data, HelperError?) {
        lock.lock(); defer { lock.unlock() }
        return (storage, failure)
    }
}

private let sherpaTtsProgressCallback: TtsProgressCallbackWithArg = { samples, count, _, rawContext in
    guard let rawContext, count >= 0 else { return 0 }
    let collector = Unmanaged<PCM16SampleCollector>.fromOpaque(rawContext).takeUnretainedValue()
    return collector.append(samples: samples, count: Int(count))
}

final class SystemPCMCollector: @unchecked Sendable {
    private let lock = NSLock()
    private let maximumBytes: Int
    private let cancellation: AudioCancellationFlag?
    private var storage = Data()
    private var sampleRate: Int32?
    private var failure: HelperError?
    private var continuation: CheckedContinuation<(Data, Int32), Error>?
    private var completed = false

    init(maximumBytes: Int, cancellation: AudioCancellationFlag? = nil) {
        self.maximumBytes = max(0, maximumBytes)
        self.cancellation = cancellation
        storage.reserveCapacity(min(self.maximumBytes, 64 * 1024))
    }

    func install(_ continuation: CheckedContinuation<(Data, Int32), Error>) {
        lock.lock()
        if completed {
            let result = (storage, sampleRate ?? 0)
            let failure = self.failure
            lock.unlock()
            if let failure { continuation.resume(throwing: failure) }
            else if !result.0.isEmpty, result.1 > 0 { continuation.resume(returning: result) }
            else { continuation.resume(throwing: HelperError.synthesisFailed("system voice produced no audio")) }
            return
        }
        self.continuation = continuation
        lock.unlock()
    }

    func value() async throws -> (Data, Int32) {
        try await withCheckedThrowingContinuation { continuation in install(continuation) }
    }

    func append(_ buffer: AVAudioBuffer) {
        if cancellation?.isCancelled == true {
            finish(cancellation?.terminalError() ?? HelperError.cancelled("system audio synthesis was cancelled"))
            return
        }
        guard let pcm = buffer as? AVAudioPCMBuffer else {
            finish(HelperError.synthesisFailed("system voice returned an unsupported PCM buffer"))
            return
        }
        if pcm.frameLength == 0 {
            finish(nil)
            return
        }
        let format = pcm.format
        let frames = Int(pcm.frameLength)
        guard format.commonFormat == .pcmFormatFloat32,
              let channels = pcm.floatChannelData,
              format.channelCount > 0,
              format.sampleRate.isFinite,
              format.sampleRate > 0 else {
            finish(HelperError.synthesisFailed("system voice returned a non-PCM32 buffer"))
            return
        }
        lock.lock()
        defer { lock.unlock() }
        if let failure { _ = failure; return }
        let currentRate = Int32(format.sampleRate.rounded())
        guard (8_000...768_000).contains(currentRate) else {
            failure = .synthesisFailed("system voice returned an unsupported sample rate")
            completeLocked()
            return
        }
        if cancellation?.isCancelled == true {
            failure = cancellation?.terminalError() ?? .cancelled("system audio synthesis was cancelled")
            completeLocked()
            return
        }
        if let sampleRate, sampleRate != currentRate {
            failure = .synthesisFailed("system voice changed sample rate while rendering")
            completeLocked()
            return
        }
        sampleRate = currentRate
        guard frames <= (maximumBytes - storage.count) / 2 else {
            failure = .mediaTooLarge("system synthesized PCM exceeds the configured payload limit")
            completeLocked()
            return
        }
        let start = storage.count
        storage.count += frames * 2
        storage.withUnsafeMutableBytes { raw in
            let output = raw.bindMemory(to: Int16.self)
            for frame in 0..<frames {
                var mixed: Float = 0
                for channel in 0..<Int(format.channelCount) { mixed += channels[channel][frame] }
                let sample = max(-1, min(1, mixed / Float(format.channelCount)))
                output[(start / 2) + frame] = Int16((sample * 32767).rounded()).littleEndian
            }
        }
    }

    func cancel(with error: HelperError = .cancelled("system audio synthesis was cancelled")) { finish(error) }

    private func finish(_ error: Error?) {
        lock.lock(); defer { lock.unlock() }
        if let error { failure = (error as? HelperError) ?? HelperError.synthesisFailed(error.localizedDescription) }
        completeLocked()
    }

    private func completeLocked() {
        guard !completed else { return }
        completed = true
        let continuation = self.continuation
        self.continuation = nil
        let result = (storage, sampleRate ?? 0)
        let failure = self.failure
        lock.unlock()
        if let failure { continuation?.resume(throwing: failure) }
        else if !result.0.isEmpty, result.1 > 0 { continuation?.resume(returning: result) }
        else { continuation?.resume(throwing: HelperError.synthesisFailed("system voice produced no audio")) }
        lock.lock()
    }
}

private final class SpeechSynthesizerCancellationHandle: @unchecked Sendable {
    let synthesizer: AVSpeechSynthesizer
    init(_ synthesizer: AVSpeechSynthesizer) { self.synthesizer = synthesizer }
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
        throw HelperError.mediaTooLarge("the \(label) audio payload is too large to send over helper IPC")
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
    // Drain both streams while the child is running. Waiting first can deadlock
    // when tar (or another child) fills either pipe before it exits.
    try? output.fileHandleForWriting.close()
    try? error.fileHandleForWriting.close()
    let readGroup = DispatchGroup()
    let collected = ShellOutputCollector()
    readGroup.enter()
    DispatchQueue.global(qos: .utility).async {
        collected.setStandardOutput(output.fileHandleForReading.readDataToEndOfFile())
        readGroup.leave()
    }
    readGroup.enter()
    DispatchQueue.global(qos: .utility).async {
        collected.setStandardError(error.fileHandleForReading.readDataToEndOfFile())
        readGroup.leave()
    }
    process.waitUntilExit()
    readGroup.wait()
    let (standardOutput, standardError) = collected.data
    guard process.terminationStatus == 0 else {
        let detail = String(data: standardError, encoding: .utf8) ?? ""
        throw HelperError.download(detail.isEmpty ? "archive command failed" : detail.trimmingCharacters(in: .whitespacesAndNewlines))
    }
    return String(data: standardOutput, encoding: .utf8) ?? ""
}

private final class ShellOutputCollector: @unchecked Sendable {
    private let lock = NSLock()
    private var output = Data()
    private var error = Data()

    func setStandardOutput(_ data: Data) {
        lock.lock(); defer { lock.unlock() }
        output = data
    }

    func setStandardError(_ data: Data) {
        lock.lock(); defer { lock.unlock() }
        error = data
    }

    var data: (Data, Data) {
        lock.lock(); defer { lock.unlock() }
        return (output, error)
    }
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

struct SystemVoiceIdentity: Equatable, Sendable {
    let identifier: String
    let name: String
}

func resolvingLegacySystemVoice(
    _ selection: AudioVoiceSelection?,
    against voices: [SystemVoiceIdentity]
) -> AudioVoiceSelection? {
    guard let selection, selection.source == .system,
          selection.id != "default",
          !voices.contains(where: { $0.identifier == selection.id }) else { return selection }
    let matches = voices.filter { $0.name.compare(selection.id, options: .caseInsensitive) == .orderedSame }
    guard matches.count == 1, let match = matches.first else { return selection }
    return AudioVoiceSelection(source: .system, id: match.identifier)
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
        throw HelperError.voiceMissing("the selected offline voice is unavailable")
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
    private static func supports(language: String, in available: [String]) -> Bool {
        let requested = language.lowercased()
        return available.contains { supported in
            let candidate = supported.lowercased()
            return requested == candidate || requested.hasPrefix(candidate + "-")
        }
    }

    static func recognitionModel(for languageIdentifier: String, modelID: String? = nil, root: URL) -> GeneratedOfflineModelEntry? {
        if let modelID {
            guard let entry = GeneratedVoiceModelCatalog.byID(modelID), entry.kind == .stt,
                  modelDirectory(for: entry, root: root) != nil,
                  supports(language: languageIdentifier, in: entry.languages) else { return nil }
            return entry
        }
        return GeneratedVoiceModelCatalog.packFor(languageBase(languageIdentifier)).first {
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
            ?? (requestedVoiceID.isEmpty ? model.voices.first { $0.language == target } ?? model.voices.first : nil)
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

    static func transcribe(samples: [Float], sampleRate: Int, languageIdentifier: String, modelID: String? = nil, root: URL) throws -> HelperTranscript {
        guard let model = recognitionModel(for: languageIdentifier, modelID: modelID, root: root),
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
            throw HelperError.noSpeech("no speech was recognized")
        }
        return HelperTranscript(text: trimmed, language: languageIdentifier, confidence: nil)
    }

    static func synthesize(
        text: String,
        voiceID: String,
        languageIdentifier: String,
        rate: Double,
        maximumPayloadBytes: Int,
        cancellation: AudioCancellationFlag,
        root: URL
    ) throws -> (Data, Int32) {
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
            let collector = PCM16SampleCollector(maximumBytes: maximumPayloadBytes, cancellation: cancellation)
            let context = Unmanaged.passUnretained(collector).toOpaque()
            let generated = tts.generateWithConfig(
                text: text,
                config: SherpaOnnxGenerationConfigSwift(speed: Float(max(0.5, min(2, rate))), sid: Int(speakerID)),
                callback: sherpaTtsProgressCallback,
                arg: context
            )
            try cancellation.check()
            let (pcm, failure) = collector.result()
            if let failure { throw failure }
            guard generated.n > 0, generated.sampleRate > 0, !pcm.isEmpty else {
                throw HelperError.synthesisFailed("the offline synthesizer failed to generate speech")
            }
            return (pcm, Int32(generated.sampleRate))
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
            let collector = PCM16SampleCollector(maximumBytes: maximumPayloadBytes, cancellation: cancellation)
            let context = Unmanaged.passUnretained(collector).toOpaque()
            let generated = tts.generateWithConfig(
                text: text,
                config: SherpaOnnxGenerationConfigSwift(speed: Float(max(0.5, min(2, rate))), sid: Int(speakerID)),
                callback: sherpaTtsProgressCallback,
                arg: context
            )
            try cancellation.check()
            let (pcm, failure) = collector.result()
            if let failure { throw failure }
            guard generated.n > 0, generated.sampleRate > 0, !pcm.isEmpty else {
                throw HelperError.synthesisFailed("the offline synthesizer failed to generate speech")
            }
            return (pcm, Int32(generated.sampleRate))
        default:
            throw HelperError.modelMissing("the requested offline model does not support speech synthesis")
        }
    }
}

@MainActor
func runListeningMonitor(
    intervalNanoseconds: UInt64 = 150_000_000,
    sleep: @escaping @MainActor (UInt64) async throws -> Void = { nanoseconds in
        try await Task.sleep(nanoseconds: nanoseconds)
    },
    tick: @escaping @MainActor () async -> Bool
) async {
    while !Task.isCancelled {
        do {
            try await sleep(intervalNanoseconds)
        } catch {
            return
        }
        guard !Task.isCancelled, await tick() else { return }
    }
}

enum ListeningMonitorOutcome: Equatable, Sendable {
    case continueListening
    case finalizeSpeech
    case noSpeech
}

@MainActor
final class SpeechFinalResultGate {
    private var didReceiveFinalResult = false
    private var waiters: [CheckedContinuation<Bool, Never>] = []
    private var timeoutTask: Task<Void, Never>?

    func signalFinalResult() {
        didReceiveFinalResult = true
        completeWait(receivedFinalResult: true)
    }

    func waitForFinalResult(timeoutNanoseconds: UInt64) async -> Bool {
        guard !didReceiveFinalResult else { return true }
        return await withCheckedContinuation { continuation in
            guard !didReceiveFinalResult else {
                continuation.resume(returning: true)
                return
            }
            waiters.append(continuation)
            if timeoutTask == nil {
                timeoutTask = Task { [weak self] in
                    do { try await Task.sleep(nanoseconds: timeoutNanoseconds) }
                    catch { return }
                    self?.completeWait(receivedFinalResult: false)
                }
            }
        }
    }

    private func completeWait(receivedFinalResult: Bool) {
        guard !waiters.isEmpty else { return }
        let waiters = self.waiters
        self.waiters.removeAll()
        timeoutTask?.cancel()
        timeoutTask = nil
        waiters.forEach { $0.resume(returning: receivedFinalResult) }
    }
}

@MainActor
final class CaptureAdmissionGate {
    private var didProcessFirstBuffer = false
    private var firstBufferFailure: HelperError?
    private var waiter: CheckedContinuation<HelperError?, Never>?
    private var timeoutTask: Task<Void, Never>?

    func reportFirstBuffer(failure: HelperError?) {
        guard !didProcessFirstBuffer else { return }
        didProcessFirstBuffer = true
        firstBufferFailure = failure
        completeWait(with: failure)
    }

    func waitForFirstBuffer(timeoutNanoseconds: UInt64) async -> HelperError? {
        if didProcessFirstBuffer { return firstBufferFailure }
        return await withCheckedContinuation { continuation in
            guard !didProcessFirstBuffer else {
                continuation.resume(returning: firstBufferFailure)
                return
            }
            waiter = continuation
            timeoutTask = Task { [weak self] in
                do { try await Task.sleep(nanoseconds: timeoutNanoseconds) }
                catch { return }
                self?.completeWait(with: nil)
            }
        }
    }

    private func completeWait(with failure: HelperError?) {
        guard let waiter else { return }
        self.waiter = nil
        timeoutTask?.cancel()
        timeoutTask = nil
        waiter.resume(returning: failure)
    }
}

func listeningMonitorOutcome(
    hasHeardSpeech: Bool,
    elapsedSinceStart: TimeInterval,
    elapsedSinceSpeech: TimeInterval,
    silenceTimeout: TimeInterval = silenceDuration,
    noSpeechTimeout: TimeInterval = noSpeechTimeout
) -> ListeningMonitorOutcome {
    if !hasHeardSpeech, elapsedSinceStart >= noSpeechTimeout { return .noSpeech }
    if hasHeardSpeech, elapsedSinceSpeech >= silenceTimeout { return .finalizeSpeech }
    return .continueListening
}

@MainActor
final class ListeningSession {
    typealias StopResult = ([Float], HelperTranscript?, Int, String?, HelperError?)

    enum Route {
        case system
        case sherpa
        case captureOnly
    }

    private let owner: HelperOwner
    private let languageIdentifier: String
    private let route: Route
    private let outputSampleRate: Int
    private let maximumSampleCount: Int
    private let cancellation: AudioCancellationFlag
    private let onPartial: @Sendable (HelperTranscript) async -> Void
    private let onLevel: @Sendable (Double) async -> Void
    private let onSilence: @Sendable () async -> Void
    private let onNoSpeech: @Sendable () async -> Void
    private let onLimit: @Sendable () async -> Void
    private let captureAdmissionGate = CaptureAdmissionGate()

    private let engine = AVAudioEngine()
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private let finalResultGate = SpeechFinalResultGate()
    private var transcriptText = ""
    private var confidence: Double?
    private var recognitionFailure: String?
    private var captureFailure: HelperError?
    private var samples: [Float] = []
    private let startedAt = Date()
    private var lastSpeechAt = Date()
    private var hasHeardSpeech = false
    private var lastLevelPublishedAt = Date.distantPast
    private var monitorTask: Task<Void, Never>?
    private var stopTask: Task<StopResult, Never>?
    private var stoppedResult: StopResult?

    init(
        owner: HelperOwner,
        languageIdentifier: String,
        route: Route,
        outputSampleRate: Int,
        maximumSampleCount: Int,
        cancellation: AudioCancellationFlag,
        onPartial: @escaping @Sendable (HelperTranscript) async -> Void,
        onLevel: @escaping @Sendable (Double) async -> Void,
        onSilence: @escaping @Sendable () async -> Void,
        onNoSpeech: @escaping @Sendable () async -> Void = {},
        onLimit: @escaping @Sendable () async -> Void
    ) {
        self.owner = owner
        self.languageIdentifier = languageIdentifier
        self.route = route
        self.outputSampleRate = outputSampleRate
        self.maximumSampleCount = maximumSampleCount
        self.cancellation = cancellation
        self.onPartial = onPartial
        self.onLevel = onLevel
        self.onSilence = onSilence
        self.onNoSpeech = onNoSpeech
        self.onLimit = onLimit
    }

    func start() throws {
        try cancellation.check()
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
                        if result.isFinal { self.finalResultGate.signalFinalResult() }
                        await self.onPartial(HelperTranscript(
                            text: self.transcriptText,
                            language: self.languageIdentifier,
                            confidence: self.confidence
                        ))
                    } else if let error {
                        self.recognitionFailure = error.localizedDescription
                        self.finalResultGate.signalFinalResult()
                    }
                }
            }
        }

        try cancellation.check()
        input.removeTap(onBus: 0)
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
            guard let self else { return }
            Task { @MainActor in
                self.capture(buffer: buffer)
            }
        }
        engine.prepare()
        try cancellation.check()
        try engine.start()
        try cancellation.check()

        monitorTask = Task { [weak self] in
            await runListeningMonitor { [weak self] in
                guard let self else { return false }
                return await self.monitoringTick()
            }
        }
    }

    func waitForInitialCaptureBuffer(timeoutNanoseconds: UInt64) async -> HelperError? {
        await captureAdmissionGate.waitForFirstBuffer(timeoutNanoseconds: timeoutNanoseconds)
    }

    private func monitoringTick() async -> Bool {
        guard engine.isRunning else { return false }
        let now = Date()
        switch listeningMonitorOutcome(
            hasHeardSpeech: hasHeardSpeech,
            elapsedSinceStart: now.timeIntervalSince(startedAt),
            elapsedSinceSpeech: now.timeIntervalSince(lastSpeechAt)
        ) {
        case .continueListening:
            return !Task.isCancelled
        case .finalizeSpeech:
            await onSilence()
        case .noSpeech:
            await onNoSpeech()
        }
        return false
    }

    private func capture(buffer: AVAudioPCMBuffer) {
        guard captureFailure == nil, !cancellation.isCancelled else { return }
        let frameCount = Int(buffer.frameLength)
        guard frameCount > 0 else { return }
        if route == .system {
            request?.append(buffer)
        }
        guard let channelData = buffer.floatChannelData?.pointee else { return }
        let sourceRate = Int(buffer.format.sampleRate.rounded())
        let remaining = maximumSampleCount - samples.count
        let outputCount = sourceRate == outputSampleRate
            ? frameCount
            : max(1, Int((Double(frameCount) * Double(outputSampleRate) / Double(sourceRate)).rounded()))
        guard outputCount <= remaining else {
            captureFailure = .mediaTooLarge("captured audio exceeds the configured payload limit")
            captureAdmissionGate.reportFirstBuffer(failure: captureFailure)
            Task { await onLimit() }
            return
        }
        let source = UnsafeBufferPointer(start: channelData, count: frameCount)
        let converted = sourceRate == outputSampleRate
            ? Array(source)
            : resample(Array(source), from: sourceRate, to: outputSampleRate, outputCount: outputCount)
        samples.append(contentsOf: converted)
        captureAdmissionGate.reportFirstBuffer(failure: nil)
        let rms = sqrt(converted.reduce(0) { $0 + ($1 * $1) } / Float(max(1, converted.count)))
        let now = Date()
        if rms.isFinite, now.timeIntervalSince(lastLevelPublishedAt) >= 0.05 {
            lastLevelPublishedAt = now
            let level = Double(min(1, max(0, rms)))
            Task { await onLevel(level) }
        }
        if rms >= silenceThreshold {
            lastSpeechAt = Date()
            hasHeardSpeech = true
        }
    }

    private func resample(_ input: [Float], from sourceRate: Int, to targetRate: Int, outputCount: Int) -> [Float] {
        if input.isEmpty || sourceRate == targetRate { return input }
        let ratio = Double(targetRate) / Double(sourceRate)
        return (0..<outputCount).map { index in
            let sourcePosition = Double(index) / ratio
            let lowerIndex = Int(sourcePosition.rounded(.down))
            let upperIndex = min(input.count - 1, lowerIndex + 1)
            let fraction = Float(sourcePosition - Double(lowerIndex))
            return input[lowerIndex] + ((input[upperIndex] - input[lowerIndex]) * fraction)
        }
    }

    func stop() async -> StopResult {
        if let stoppedResult { return stoppedResult }
        if let stopTask { return await stopTask.value }
        let task = Task { @MainActor in await self.performStop() }
        stopTask = task
        let result = await task.value
        stoppedResult = result
        stopTask = nil
        return result
    }

    private func performStop() async -> StopResult {
        monitorTask?.cancel()
        monitorTask = nil
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        request?.endAudio()
        if route == .system, request != nil, task != nil {
            _ = await finalResultGate.waitForFinalResult(timeoutNanoseconds: speechFinalResultTimeoutNanoseconds)
        }
        task?.cancel()
        let trimmed = transcriptText.trimmingCharacters(in: .whitespacesAndNewlines)
        let transcript = trimmed.isEmpty || recognitionFailure != nil
            ? nil
            : HelperTranscript(text: trimmed, language: languageIdentifier, confidence: confidence)
        return (samples, transcript, outputSampleRate, recognitionFailure, captureFailure)
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
    private var didEmitSpeech = false
    private var operationCancellation: AudioCancellationFlag?

    var hasStartedOutput: Bool { didEmitSpeech }

    func playSystem(text: String, voice: AVSpeechSynthesisVoice?, rate: Float, cancellation: AudioCancellationFlag) async throws {
        try cancellation.check()
        try stopIfNeeded()
        didEmitSpeech = false
        operationCancellation = cancellation
        let synthesizer = AVSpeechSynthesizer()
        synthesizer.delegate = self
        let utterance = AVSpeechUtterance(string: text)
        utterance.rate = rate
        utterance.voice = voice
        mode = .system(synthesizer)
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            do { try cancellation.check() } catch {
                continuation.resume(throwing: error)
                return
            }
            self.continuation = continuation
            synthesizer.speak(utterance)
        }
    }

    func playPCM16(_ pcm16: Data, sampleRate: Int32, cancellation: AudioCancellationFlag) async throws {
        try cancellation.check()
        try stopIfNeeded()
        operationCancellation = cancellation
        let player = try AVAudioPlayer(data: waveData(pcm16: pcm16, sampleRate: Int(sampleRate)))
        player.delegate = self
        player.prepareToPlay()
        try cancellation.check()
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
    nonisolated func speechSynthesizer(
        _: AVSpeechSynthesizer,
        willSpeakRangeOfSpeechString _: NSRange,
        utterance _: AVSpeechUtterance
    ) {
        Task { @MainActor [weak self] in self?.didEmitSpeech = true }
    }

    nonisolated func speechSynthesizer(_: AVSpeechSynthesizer, didFinish _: AVSpeechUtterance) {
        Task { @MainActor [weak self] in
            self?.finish(with: .success(()))
        }
    }

    nonisolated func speechSynthesizer(_: AVSpeechSynthesizer, didCancel _: AVSpeechUtterance) {
        Task { @MainActor [weak self] in
            guard let self else { return }
            if self.operationCancellation?.isCancelled == true {
                self.finish(with: .failure(HelperError.cancelled("speech playback was cancelled")))
            } else if !self.didEmitSpeech {
                self.finish(with: .failure(HelperError.unavailable("system speech could not start before emitting audio")))
            } else {
                self.finish(with: .failure(HelperError.cancelled("speech playback was interrupted")))
            }
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
    private struct ActiveInstall {
        let token: UUID
        let task: Task<Void, Never>
    }

    private struct PartialDownloadMetadata: Codable {
        let etag: String?
        let lastModified: String?
    }

    private let root: URL
    private let writer: LineWriter
    private let snapshotProvider: @Sendable () async -> HelperSnapshot
    private let downloadOverride: (@Sendable (GeneratedOfflineModelEntry, URL) async throws -> Void)?
    private var states: [String: HelperModelSnapshot] = [:]
    private var tasks: [String: ActiveInstall] = [:]
    private var quiescingInstalls: [String: Int] = [:]
    private var referenceCounts: [String: Int] = [:]

    init(
        root: URL,
        writer: LineWriter,
        downloadOverride: (@Sendable (GeneratedOfflineModelEntry, URL) async throws -> Void)? = nil,
        snapshotProvider: @escaping @Sendable () async -> HelperSnapshot
    ) {
        self.root = root
        self.writer = writer
        self.downloadOverride = downloadOverride
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

    func retain(modelID: String) throws -> GeneratedOfflineModelEntry {
        guard let model = GeneratedVoiceModelCatalog.byID(modelID) else {
            throw HelperError.modelMissing("the requested offline model is unknown")
        }
        guard SherpaRuntime.modelDirectory(for: model, root: root) != nil else {
            throw HelperError.modelMissing("the requested offline model is not installed")
        }
        referenceCounts[modelID, default: 0] += 1
        return model
    }

    func release(modelID: String) {
        let count = referenceCounts[modelID, default: 0]
        if count <= 1 { referenceCounts.removeValue(forKey: modelID) }
        else { referenceCounts[modelID] = count - 1 }
    }

    func activeReferenceCount() -> Int {
        referenceCounts.values.reduce(0, +)
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
        if tasks[modelID] != nil || quiescingInstalls[modelID, default: 0] > 0 { return }
        states[modelID] = HelperModelSnapshot(
            modelId: model.id,
            state: .queued
        )
        let token = UUID()
        let task = Task { await self.performInstall(model: model, token: token) }
        tasks[modelID] = ActiveInstall(token: token, task: task)
        await publishModel(modelID)
    }

    func cancel(modelID: String) async {
        guard states[modelID] != nil else { return }
        quiescingInstalls[modelID, default: 0] += 1
        defer { finishQuiescing(modelID) }
        if let install = tasks[modelID] {
            install.task.cancel()
            await install.task.value
            clearTask(modelID, token: install.token, allowDuringQuiescing: true)
        }
        reconcile()
        await publishModel(modelID)
    }

    func remove(modelID: String) async throws {
        guard states[modelID] != nil else { throw HelperError.invalidRequest("unknown model id \(modelID)") }
        if referenceCounts[modelID, default: 0] > 0 {
            throw HelperError.busy("the selected offline model is currently in use")
        }
        quiescingInstalls[modelID, default: 0] += 1
        defer { finishQuiescing(modelID) }
        if let install = tasks[modelID] {
            install.task.cancel()
            await install.task.value
            clearTask(modelID, token: install.token, allowDuringQuiescing: true)
        }
        guard referenceCounts[modelID, default: 0] == 0 else {
            throw HelperError.busy("the selected offline model is currently in use")
        }
        try? FileManager.default.removeItem(at: root.appending(path: modelID, directoryHint: .isDirectory))
        try? FileManager.default.removeItem(at: root.appending(path: ".download-\(modelID).part"))
        try? FileManager.default.removeItem(at: metadataURL(for: modelID))
        reconcile()
        await publishModel(modelID)
    }

    private func performInstall(model: GeneratedOfflineModelEntry, token: UUID) async {
        let modelID = model.id
        do {
            let archive = root.appending(path: ".download-\(modelID).part")
            let staging = root.appending(path: ".stage-\(modelID)-\(UUID().uuidString)", directoryHint: .isDirectory)
            try FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: staging) }
            try checkInstallActive(modelID, token: token)
            try await download(model: model, archive: archive, token: token)
            try checkInstallActive(modelID, token: token)
            try await updateState(modelID, state: .verifying, token: token)
            try checkInstallActive(modelID, token: token)
            let digest = try await ModelStore.sha256(of: archive)
            try checkInstallActive(modelID, token: token)
            if digest != model.sha256 {
                throw HelperError.checksum("the downloaded model failed checksum verification")
            }
            try await updateState(modelID, state: .extracting, token: token)
            try checkInstallActive(modelID, token: token)
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
            try checkInstallActive(modelID, token: token)
            let final = root.appending(path: modelID, directoryHint: .isDirectory)
            let activation = root.appending(path: ".activate-\(modelID)-\(UUID().uuidString)", directoryHint: .isDirectory)
            try? FileManager.default.removeItem(at: activation)
            try FileManager.default.moveItem(at: extracted, to: activation)
            if FileManager.default.fileExists(atPath: final.path) {
                try? FileManager.default.removeItem(at: final)
            }
            try checkInstallActive(modelID, token: token)
            try FileManager.default.moveItem(at: activation, to: final)
            try? FileManager.default.removeItem(at: archive)
            try? FileManager.default.removeItem(at: metadataURL(for: modelID))
            reconcile()
            await publishModel(modelID)
            try checkInstallActive(modelID, token: token)
            await writer.writeEnvelope(OutputEnvelope(
                id: nil,
                type: "event",
                result: nil,
                event: HelperEvent(
                    type: "snapshot_changed",
                    snapshot: await snapshotProvider(),
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
            // The cancel/remove caller owns cleanup and publishes the final state after this task quiesces.
        } catch let error as HelperError {
            if installIsCurrent(modelID, token: token), !Task.isCancelled {
                await setFailed(modelID, code: error.code, message: error.message, token: token)
            }
        } catch {
            if installIsCurrent(modelID, token: token), !Task.isCancelled {
                await setFailed(modelID, code: "download", message: error.localizedDescription, token: token)
            }
        }
        clearTask(modelID, token: token)
    }

    private func installIsCurrent(_ modelID: String, token: UUID) -> Bool {
        tasks[modelID]?.token == token && quiescingInstalls[modelID, default: 0] == 0
    }

    private func checkInstallActive(_ modelID: String, token: UUID) throws {
        try Task.checkCancellation()
        guard installIsCurrent(modelID, token: token) else { throw CancellationError() }
    }

    private func clearTask(_ modelID: String, token: UUID, allowDuringQuiescing: Bool = false) {
        guard tasks[modelID]?.token == token,
              allowDuringQuiescing || quiescingInstalls[modelID, default: 0] == 0 else { return }
        tasks[modelID] = nil
    }

    private func finishQuiescing(_ modelID: String) {
        let count = quiescingInstalls[modelID, default: 1]
        if count <= 1 { quiescingInstalls.removeValue(forKey: modelID) }
        else { quiescingInstalls[modelID] = count - 1 }
    }

    private func setFailed(_ modelID: String, code _: String, message: String, token: UUID) async {
        if installIsCurrent(modelID, token: token), let current = states[modelID] {
            states[modelID] = HelperModelSnapshot(
                modelId: current.modelId,
                state: .failed(message)
            )
            await publishModel(modelID)
        }
    }

    private func updateState(_ modelID: String, state: HelperModelState, token: UUID) async throws {
        try checkInstallActive(modelID, token: token)
        guard let current = states[modelID] else { throw HelperError.invalidRequest("unknown model id \(modelID)") }
        states[modelID] = HelperModelSnapshot(
            modelId: current.modelId,
            state: state
        )
        await publishModel(modelID)
        try checkInstallActive(modelID, token: token)
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

    private func download(model: GeneratedOfflineModelEntry, archive: URL, token: UUID) async throws {
        if let downloadOverride {
            try await downloadOverride(model, archive)
            return
        }
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
            return try await download(model: model, archive: archive, token: token)
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
        try await updateState(model.id, state: .downloading(receivedBytes: existingBytes, totalBytes: expectedBytes), token: token)
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
                    try await updateState(model.id, state: .downloading(receivedBytes: writtenBytes, totalBytes: expectedBytes), token: token)
                }
            }
        }
        if !buffer.isEmpty {
            handle.write(buffer)
            writtenBytes += Int64(buffer.count)
        }
        try await updateState(model.id, state: .downloading(receivedBytes: writtenBytes, totalBytes: expectedBytes), token: token)
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

typealias HelperAudioOperationExecutor = @Sendable (_ operation: String, _ cancellation: AudioCancellationFlag) async throws -> HelperEngineResult?

actor HelperStateStore {
    private let storageRoot: URL
    private let writer: LineWriter
    private let operationExecutor: HelperAudioOperationExecutor?
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
            models: [],
            capabilities: nil,
            configurationRevision: nil,
            currentOperation: nil
        )
    }

    private var owner: HelperOwner?
    private var activity = "idle"
    private var reservedPhysicalOwner: HelperOwner?
    private var reservedPhysicalIdentity: HelperAudioOperationIdentity?
    private var listeningIdentity: HelperAudioOperationIdentity?
    private var speechIdentity: HelperAudioOperationIdentity?
    private var listeningOwner: HelperOwner?
    private var activeSession: ListeningSession?
    private var activeSpeech: PlaybackSession?
    private var speechTask: Task<Void, Never>?
    private var helperMessage: String?
    private var isShuttingDown = false
    private var recognitionSnapshot: HelperRecognitionSnapshot?
    private var playbackSnapshot: HelperPlaybackSnapshot?
    private var activeLanguage = currentLocaleTag()
    private let serviceEpoch = UInt64.random(in: 1...9_000_000_000_000_000)
    private let supportRevision: UInt64 = 1
    private struct ActiveAudioOperation {
        let identity: HelperAudioOperationIdentity
        let owner: HelperAudioOwner
        let operation: String
        let cancellation: AudioCancellationFlag
        let configurationRevision: UInt64
    }

    private struct ActiveListenContext {
        let identity: HelperAudioOperationIdentity
        let owner: HelperAudioOwner
        let language: String
        let route: AudioRouteResolution
        let cancellation: AudioCancellationFlag
    }

    private struct PendingListenFinish {
        let owner: HelperOwner
        let expiresAt: Date
    }

    private var activeRequestIdentity: HelperAudioOperationIdentity?
    private var activeRequestOwner: HelperAudioOwner?
    private var activeOperations: [String: ActiveAudioOperation] = [:]
    private var activeListenContexts: [String: ActiveListenContext] = [:]
    private var listenFinalizers: [String: Task<HelperTranscript, Error>] = [:]
    private var pendingListenFinishes: [String: PendingListenFinish] = [:]
    private var pendingListenFinishOrder: [String] = []
    private var finishingOperations = Set<String>()
    private var operationCompletionWaiters: [String: [UUID: CheckedContinuation<Bool, Never>]] = [:]
    private var endingOwners: [String: Int] = [:]
    private var activeOfflineSyntheses: [String: HelperAudioOwner] = [:]
    private var offlineRenderModels = Set<String>()
    private var cancelledOperationKeys = Set<String>()
    private var cancelledOperationHistory: [String] = []
    private var seenAudioIdentities = Set<String>()
    private var audioIdentityHistory: [String] = []
    private var audioConfigurationRevision: UInt64 = 0
    private var operationTraces: [HelperAudioOperationTrace] = []
    private var listenWaiters: [String: AudioListenWaiter] = [:]
    private var recordingHandle: String?
    private var recordingOwner: HelperAudioOwner?
    private var recordingOriginIdentity: HelperAudioOperationIdentity?
    private var recordingFormat = "wav"
    private var recordingMaximumPayloadBytes = maxInlineAudioBase64Bytes * 3 / 4
    private var recordingConfigurationRevision: UInt64 = 0
    private var speechRenderOwner: HelperAudioOwner?
    private var speechRenderIdentity: HelperAudioOperationIdentity?
    private var systemRenderIdentity: HelperAudioOperationIdentity?
    private var recordingFailure: HelperError?
    private var audioConfiguration = AudioConfigurationNormalizer.defaults

    init(storageRoot: URL, writer: LineWriter, operationExecutor: HelperAudioOperationExecutor? = nil) {
        self.storageRoot = storageRoot
        self.writer = writer
        self.operationExecutor = operationExecutor
    }

    func snapshot(
        message: String?,
        revision: UInt64? = nil
    ) async -> HelperSnapshot {
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
            models: await models.snapshots(),
            capabilities: await audioCapabilities(speechStatus: speechStatus),
            configurationRevision: revision ?? audioConfigurationRevision,
            currentOperation: activeRequestIdentity.flatMap { identity in
                activeRequestOwner.map { HelperAudioCurrentOperation(identity: identity, owner: $0) }
            },
            audioOperations: operationTraces,
            activeOperationCount: activeOperations.count,
            pendingOperationCount: activeOperations.count,
            activeRecordingCount: owner == nil || activity != "listening" ? 0 : 1,
            activePlaybackCount: activeSpeech == nil ? 0 : 1,
            activeModelReferenceCount: await models.activeReferenceCount()
        )
    }

    private func audioCapabilities(speechStatus: SFSpeechRecognizerAuthorizationStatus) async -> HelperAudioCapabilities {
        let microphone = Self.microphonePermissionState()
        let configuration = audioConfiguration
        let language = resolvedLanguage(nil, configuration: configuration)
        let recognitionRoute = await resolveRecognitionRoute(
            configuration: configuration,
            language: language,
            speechStatus: speechStatus
        )
        let speechRoute = await resolveSpeechRoute(
            configuration: configuration,
            language: language,
            voiceOverride: nil
        )
        func readinessState(for route: AudioRouteResolution) -> String {
            switch route.status {
            case .ready: return "ready"
            case .permissionRequired: return "needs_permission"
            case .invalidRequest: return "unavailable"
            case .unavailable:
                return route.reason.localizedCaseInsensitiveContains("model") ? "missing_model" : "unavailable"
            }
        }
        let physicalBusy = owner != nil || reservedPhysicalIdentity != nil
        let recordState = physicalBusy ? "busy" : microphone == "granted" ? "ready" : microphone == "prompt" ? "needs_permission" : "unavailable"
        let recognitionState = readinessState(for: recognitionRoute)
        let listenState = physicalBusy ? "busy" : microphone == "prompt" ? "needs_permission" : microphone == "granted" ? recognitionState : "unavailable"
        let speechState = readinessState(for: speechRoute)
        let systemSpeechBusy = speechRoute.effective?.source == .system && (physicalBusy || systemRenderIdentity != nil)
        let synthState = systemSpeechBusy ? "busy" : speechState
        let speakState = physicalBusy || systemRenderIdentity != nil ? "busy" : speechState
        let rawMaximumPayload = maxInlineAudioBase64Bytes * 3 / 4
        return HelperAudioCapabilities(
            serviceEpoch: serviceEpoch,
            supportRevision: supportRevision,
            supportedOperations: ["record", "listen", "synthesize", "speak"],
            readiness: [
                HelperAudioReadiness(operation: "record", state: recordState),
                HelperAudioReadiness(operation: "listen", state: listenState),
                HelperAudioReadiness(operation: "synthesize", state: synthState),
                HelperAudioReadiness(operation: "speak", state: speakState),
            ],
            maxPayloadBytes: UInt64(rawMaximumPayload & ~1)
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

    private func publishInputLevel(owner: HelperOwner, level: Double) async {
        await writer.writeEnvelope(OutputEnvelope(
            id: nil,
            type: "event",
            result: nil,
            event: HelperEvent(
                type: "input_level",
                snapshot: nil,
                owner: owner,
                progress: nil,
                model: nil,
                state: nil,
                error: nil,
                message: nil,
                level: level
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

    func rejectOverloaded(_ envelope: [String: Any]) async {
        let id = envelope["id"] as? String ?? UUID().uuidString
        let request = envelope["request"] as? [String: Any] ?? [:]
        guard envelope["kind"] as? String == "engine_request",
              parseHelperAudioIdentity(request["identity"] as? [String: Any] ?? [:]) != nil,
              parseHelperAudioOwner(request["owner"] as? [String: Any] ?? [:]) != nil else {
            await handleCommand(id: id, input: envelope["command"] as? [String: Any] ?? [:])
            return
        }
        await writeEngineResult(
            id: id,
            revision: (envelope["configurationRevision"] as? NSNumber)?.uint64Value ?? 0,
            result: .failed(kind: "busy", message: "the native audio helper is at its in-flight operation limit"),
            message: nil
        )
    }

    func cancelOperation(_ rawIdentity: [String: Any]) async {
        guard let identity = parseHelperAudioIdentity(rawIdentity) else { return }
        let key = helperAudioIdentityKey(identity)
        let activeOperation = activeOperations[key]
        rememberCancelledIdentity(key)
        removePendingListenFinish(key)
        activeOperation?.cancellation.cancel()
        if let waiter = listenWaiters[key] {
            waiter.complete(.failure(.cancelled("the live listening operation was cancelled")))
            listenWaiters.removeValue(forKey: key)
        }
        if let recordingToRollback = recordingOriginToRollback(
            for: identity,
            operation: activeOperation?.operation,
            owner: activeOperation?.owner,
            recordingOrigin: recordingOriginIdentity,
            recordingOwner: recordingOwner
        ) {
            await rollbackRecording(identity: recordingToRollback)
        }
        if listeningIdentity == identity, let activeSession {
            _ = await activeSession.stop()
            if listeningIdentity == identity, self.activeSession === activeSession {
                releaseCaptureLease(identity: identity, owner: listeningOwner)
            }
        }
        if speechIdentity == identity {
            speechTask?.cancel()
            if let activeSpeech { await MainActor.run { activeSpeech.stop() } }
            self.activeSpeech = nil
            self.speechIdentity = nil
            if self.owner == activeOperations[key]?.owner.helperOwner {
                self.owner = nil
                self.activity = "idle"
            }
        }
        if activeRequestIdentity == identity {
            self.activeRequestIdentity = nil
            self.activeRequestOwner = nil
        }
        await publishOwnerChanged()
        await publishSnapshotChanged()
    }

    func pendingListenFinishCount() -> Int {
        pendingListenFinishes.count
    }

    private func requestListenFinish(
        identity: HelperAudioOperationIdentity,
        owner requestOwner: HelperOwner
    ) async throws -> HelperTranscript? {
        guard !isShuttingDown else { throw HelperError.cancelled("the audio helper is shutting down") }
        guard identity.serviceEpoch == serviceEpoch else {
            throw HelperError.cancelled("the live listening operation belongs to a different helper session")
        }
        let key = helperAudioIdentityKey(identity)
        if cancelledOperationKeys.contains(key) || (seenAudioIdentities.contains(key) && activeOperations[key] == nil) {
            return nil
        }
        if let active = activeOperations[key] {
            guard active.operation == "listen",
                  helperAudioOwnerKey(active.owner.helperOwner) == helperAudioOwnerKey(requestOwner) else {
                throw HelperError.invalidRequest("the finish request does not target the active listen owner")
            }
        }
        if let context = activeListenContexts[key] {
            guard helperAudioOwnerKey(context.owner.helperOwner) == helperAudioOwnerKey(requestOwner) else {
                throw HelperError.invalidRequest("the live listening owner does not match the finish request")
            }
            return try await finishListen(context: context)
        }
        queuePendingListenFinish(key, owner: requestOwner)
        return nil
    }

    private func queuePendingListenFinish(_ key: String, owner: HelperOwner) {
        let now = Date()
        for expiredKey in pendingListenFinishes.compactMap({ $0.value.expiresAt <= now ? $0.key : nil }) {
            removePendingListenFinish(expiredKey)
        }
        removePendingListenFinish(key)
        pendingListenFinishes[key] = PendingListenFinish(owner: owner, expiresAt: now.addingTimeInterval(300))
        pendingListenFinishOrder.append(key)
        while pendingListenFinishOrder.count > 64 {
            removePendingListenFinish(pendingListenFinishOrder[0])
        }
    }

    private func consumePendingListenFinish(_ context: ActiveListenContext) -> Bool {
        let key = helperAudioIdentityKey(context.identity)
        guard let pending = pendingListenFinishes[key] else { return false }
        removePendingListenFinish(key)
        guard pending.expiresAt > Date(),
              helperAudioOwnerKey(pending.owner) == helperAudioOwnerKey(context.owner.helperOwner) else { return false }
        return true
    }

    private func removePendingListenFinish(_ key: String) {
        pendingListenFinishes.removeValue(forKey: key)
        pendingListenFinishOrder.removeAll { $0 == key }
    }

    private func removePendingListenFinishes(ownerKey: String) {
        for key in pendingListenFinishes.compactMap({
            helperAudioOwnerKey($0.value.owner) == ownerKey ? $0.key : nil
        }) {
            removePendingListenFinish(key)
        }
    }

    func handleEngineRequest(
        id: String,
        identity: HelperAudioOperationIdentity,
        owner requestOwner: HelperAudioOwner,
        operation: [String: Any],
        configuration: AudioConfigurationV3,
        configurationRevision: UInt64,
        maxPayloadBytes: UInt64,
        timeoutBudgetMs: UInt64? = nil
    ) async {
        let identityKey = helperAudioIdentityKey(identity)
        guard !isShuttingDown,
              identity.serviceEpoch == serviceEpoch,
              !seenAudioIdentities.contains(identityKey),
              !cancelledOperationKeys.contains(identityKey) else {
            await writeEngineResult(id: id, revision: configurationRevision,
                                    result: .failed(kind: "cancelled", message: "the audio operation identity is stale"), message: nil)
            return
        }
        rememberAudioIdentity(identityKey)
        let operationName = operation["type"] as? String ?? ""
        if endingOwners[helperAudioOwnerKey(requestOwner), default: 0] > 0, operationName != "end_owner" {
            await writeEngineResult(
                id: id,
                revision: configurationRevision,
                result: .failed(kind: "busy", message: "the audio owner is being torn down"),
                message: nil
            )
            return
        }
        if timeoutBudgetMs == 0 {
            audioConfiguration = configuration
            appendOperationTrace(
                identity: identity,
                owner: requestOwner,
                operation: operation["type"] as? String ?? "",
                configuration: configuration,
                revision: configurationRevision
            )
            let timeout = HelperError.timeout("the audio operation deadline has already expired")
            await writeEngineResult(
                id: id,
                revision: configurationRevision,
                result: .failed(kind: Self.errorKind(for: timeout), message: timeout.message),
                message: timeout.message
            )
            await publishError(timeout, owner: requestOwner.helperOwner)
            return
        }
        let cancellation = AudioCancellationFlag()
        activeOperations[identityKey] = ActiveAudioOperation(
            identity: identity,
            owner: requestOwner,
            operation: operationName,
            cancellation: cancellation,
            configurationRevision: configurationRevision
        )
        activeRequestIdentity = identity
        activeRequestOwner = requestOwner
        audioConfigurationRevision = configurationRevision
        let timeoutTask = timeoutBudgetMs.map { budget in
            Task { [weak self] in
                let bounded = min(budget, operationName == "speak" ? 1_200_000 : 300_000)
                do { try await Task.sleep(nanoseconds: bounded * 1_000_000) }
                catch { return }
                await self?.timeoutOperation(identity: identity)
            }
        }
        defer { timeoutTask?.cancel() }
        let result: HelperEngineResult
        var failureToPublish: HelperError?
        do {
            result = try await withTaskCancellationHandler {
                try ensureCurrent(identity)
                let operationResult = try await executeAudio(
                    identity: identity,
                    owner: requestOwner,
                    operation: operation,
                    configuration: configuration,
                    configurationRevision: configurationRevision,
                    maxPayloadBytes: maxPayloadBytes,
                    cancellation: cancellation
                )
                try ensureCurrent(identity)
                return operationResult
            } onCancel: {
                cancellation.cancel()
            }
        } catch let error as HelperError {
            if recordingOriginIdentity == identity { await rollbackRecording(identity: identity) }
            result = .failed(kind: Self.errorKind(for: error), message: error.message)
            failureToPublish = error
        } catch {
            if recordingOriginIdentity == identity { await rollbackRecording(identity: identity) }
            let failure = cancellation.terminalError()
                ?? (Task.isCancelled ? .cancelled("the audio operation was cancelled") : .native(error.localizedDescription))
            result = .failed(kind: Self.errorKind(for: failure), message: failure.message)
            failureToPublish = failure
        }
        finishingOperations.insert(identityKey)
        activeOperations.removeValue(forKey: identityKey)
        if activeRequestIdentity == identity {
            activeRequestIdentity = recordingOriginIdentity
            activeRequestOwner = recordingOriginIdentity == nil ? nil : recordingOwner
        }
        await writeEngineResult(id: id, revision: configurationRevision, result: result, message: failureToPublish?.message)
        if let failureToPublish { await publishError(failureToPublish, owner: requestOwner.helperOwner) }
        finishingOperations.remove(identityKey)
        let waiters = operationCompletionWaiters.removeValue(forKey: identityKey) ?? [:]
        waiters.values.forEach { $0.resume(returning: true) }
    }

    func shutdown() async {
        guard !isShuttingDown else { return }
        isShuttingDown = true
        pendingListenFinishes.removeAll()
        pendingListenFinishOrder.removeAll()
        let active = activeOperations
        for (key, operation) in active {
            rememberCancelledIdentity(key)
            operation.cancellation.cancel()
            listenWaiters.removeValue(forKey: key)?.complete(.failure(.cancelled("the audio helper is shutting down")))
        }
        if let recordingOriginIdentity {
            await rollbackRecording(identity: recordingOriginIdentity, publishEvents: false)
        } else if let listeningIdentity {
            await stopCaptureIfCurrent(identity: listeningIdentity, publishEvents: false)
        }
        if let speechIdentity {
            if let activeSpeech { await MainActor.run { activeSpeech.stop() } }
            clearPhysicalLease(identity: speechIdentity)
        }
        let cleanupKeys = Set(active.keys).union(finishingOperations)
        let settled = await waitForOperations(Array(cleanupKeys), timeoutMs: 2_000)
        activeListenContexts.removeAll()
        listenFinalizers.removeAll()
        if !settled {
            FileHandle.standardError.write(Data("native-audio-helper: shutdown timed out waiting for audio callbacks\n".utf8))
        }
        if owner != nil || reservedPhysicalIdentity != nil {
            owner = nil
            activity = "idle"
            reservedPhysicalOwner = nil
            reservedPhysicalIdentity = nil
            listeningOwner = nil
            listeningIdentity = nil
            activeSession = nil
            activeSpeech = nil
            speechIdentity = nil
        }
    }

    private func writeEngineResult(
        id: String,
        revision: UInt64,
        result: HelperEngineResult,
        message: String?
    ) async {
        await writer.writeEnvelope(OutputEnvelope(
            id: id,
            type: "response",
            result: .engine(HelperEngineResponse(
                snapshot: await snapshot(message: message, revision: revision),
                result: result
            )),
            event: nil,
            error: nil
        ))
    }

    private func requestPermissions(_ permissions: [String]) async throws {
        let requested = Set(permissions)
        guard !requested.isEmpty, requested.isSubset(of: ["microphone", "speech"]) else {
            throw HelperError.invalidRequest("unsupported audio permission request")
        }
        try await requestSystemPermissions(requested)
        await publishSnapshotChanged()
    }

    private func ensureCurrent(_ identity: HelperAudioOperationIdentity) throws {
        let key = helperAudioIdentityKey(identity)
        guard let active = activeOperations[key], active.identity == identity else {
            throw HelperError.cancelled("the audio operation was cancelled")
        }
        if let terminal = active.cancellation.terminalError() { throw terminal }
        guard !Task.isCancelled, !cancelledOperationKeys.contains(key) else {
            throw HelperError.cancelled("the audio operation was cancelled")
        }
    }

    private func rememberAudioIdentity(_ identityKey: String) {
        seenAudioIdentities.insert(identityKey)
        audioIdentityHistory.append(identityKey)
        if audioIdentityHistory.count > 4_096 {
            let oldest = audioIdentityHistory.removeFirst()
            seenAudioIdentities.remove(oldest)
        }
    }

    private func rememberCancelledIdentity(_ identityKey: String) {
        guard cancelledOperationKeys.insert(identityKey).inserted else { return }
        cancelledOperationHistory.append(identityKey)
        if cancelledOperationHistory.count > 4_096 {
            let oldest = cancelledOperationHistory.removeFirst()
            cancelledOperationKeys.remove(oldest)
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
        case "cancel_operation":
            await cancelOperation(input["identity"] as? [String: Any] ?? [:])
            return HelperResponse(type: "cancelled", snapshot: await snapshot(message: nil), transcript: nil, recording: nil, models: nil, model: nil, error: nil)
        case "finish_listening":
            guard let identity = parseHelperAudioIdentity(input["identity"] as? [String: Any] ?? [:]),
                  let owner = parseOwner(input["owner"]) else {
                throw HelperError.invalidRequest("finish_listening requires an operation identity and owner")
            }
            let transcript = try await requestListenFinish(identity: identity, owner: owner)
            return HelperResponse(
                type: "listening_finished",
                snapshot: await snapshot(message: nil),
                transcript: transcript,
                recording: nil,
                models: nil,
                model: nil,
                error: nil
            )
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
        default:
            throw HelperError.invalidRequest("unsupported command type \(commandType)")
        }
    }

    private func executeAudio(
        identity: HelperAudioOperationIdentity,
        owner requestOwner: HelperAudioOwner,
        operation: [String: Any],
        configuration: AudioConfigurationV3,
        configurationRevision: UInt64,
        maxPayloadBytes: UInt64,
        cancellation: AudioCancellationFlag
    ) async throws -> HelperEngineResult {
        let type = operation["type"] as? String ?? ""
        try ensureCurrent(identity)
        audioConfiguration = configuration
        appendOperationTrace(
            identity: identity,
            owner: requestOwner,
            operation: type,
            configuration: configuration,
            revision: type == "stop_recording" ? recordingConfigurationRevision : configurationRevision
        )
        if let operationExecutor, let result = try await operationExecutor(type, cancellation) {
            try ensureCurrent(identity)
            return result
        }
        switch type {
        case "start_recording":
            return try await startRecording(
                identity: identity,
                owner: requestOwner,
                operation: operation,
                configurationRevision: configurationRevision,
                maxPayloadBytes: maxPayloadBytes,
                cancellation: cancellation
            )
        case "stop_recording":
            return try await stopRecording(identity: identity, owner: requestOwner, operation: operation, cancellation: cancellation)
        case "listen":
            return try await listen(
                identity: identity,
                owner: requestOwner,
                operation: operation,
                configuration: configuration,
                configurationRevision: configurationRevision,
                maxPayloadBytes: maxPayloadBytes,
                cancellation: cancellation
            )
        case "synthesize":
            return try await synthesize(
                identity: identity,
                owner: requestOwner,
                operation: operation,
                configuration: configuration,
                configurationRevision: configurationRevision,
                maxPayloadBytes: maxPayloadBytes,
                cancellation: cancellation
            )
        case "speak":
            return try await speak(
                identity: identity,
                owner: requestOwner,
                operation: operation,
                configuration: configuration,
                configurationRevision: configurationRevision,
                maxPayloadBytes: maxPayloadBytes,
                cancellation: cancellation
            )
        case "status":
            if let handle = operation["handle"] as? String,
               recordingHandle != handle || recordingOwner.map(helperAudioOwnerKey) != helperAudioOwnerKey(requestOwner) {
                throw HelperError.notRecording("the recording handle is not active for this owner")
            }
            return .status(recording: recordingHandle != nil && recordingOwner.map(helperAudioOwnerKey) == helperAudioOwnerKey(requestOwner),
                           playing: activeSpeech != nil && speechRenderOwner.map(helperAudioOwnerKey) == helperAudioOwnerKey(requestOwner))
        case "end_owner":
            try await endAudioOwner(identity: identity, owner: requestOwner)
            return .ownerEnded
        default:
            throw HelperError.invalidRequest("unsupported audio operation \(type)")
        }
    }

    private func payloadLimit(_ requested: UInt64) throws -> Int {
        guard requested > 0 else { throw HelperError.invalidRequest("audio payload limit must be positive") }
        return Int(min(requested, UInt64(maxInlineAudioBase64Bytes * 3 / 4)))
    }

    private func offlineModelAvailability() async -> [AudioOfflineModelAvailability] {
        let installed = Set((await models.snapshots()).compactMap { model -> String? in
            if case .ready = model.state { return model.modelId }
            return nil
        })
        return GeneratedVoiceModelCatalog.all.map { model in
            AudioOfflineModelAvailability(
                id: model.id,
                kind: model.kind == .stt ? .recognition : .speech,
                languages: model.languages,
                installed: installed.contains(model.id),
                voiceIds: model.kind == .tts ? model.voices.map(\.id) : nil
            )
        }
    }

    private func resolvedLanguage(_ override: Any?, configuration: AudioConfigurationV3) -> String {
        let configured = (override as? String) ?? configuration.language
        return resolveAudioLanguage(configured: configured, deviceLocale: currentLocaleTag())
    }

    private func voiceOverride(_ value: Any?) -> AudioVoiceSelection? {
        guard let raw = value as? String else { return nil }
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return nil }
        if isDefaultOrAutoVoiceOverride(text) { return nil }
        if text.hasPrefix("system:") {
            let id = String(text.dropFirst("system:".count))
            return AudioVoiceSelection(source: .system, id: id.isEmpty ? "default" : id)
        }
        if text.hasPrefix("sherpa:") {
            let payload = String(text.dropFirst("sherpa:".count))
            guard let separator = payload.lastIndex(of: ":") else {
                return AudioVoiceSelection(source: .offline, id: payload, modelId: nil)
            }
            return AudioVoiceSelection(
                source: .offline,
                id: String(payload[payload.index(after: separator)...]),
                modelId: String(payload[..<separator])
            )
        }
        return AudioVoiceSelection(source: .system, id: text)
    }

    private func configurationForSingleUtterance(
        _ configuration: AudioConfigurationV3,
        operation: [String: Any]
    ) -> AudioConfigurationV3 {
        var adjusted = configuration
        adjusted.speech = speechPreferenceForSingleUtterance(
            configuration.speech,
            voiceOverride: operation["voice"]
        )
        return adjusted
    }

    private func recognitionSystemReadiness(
        language: String,
        status: SFSpeechRecognizerAuthorizationStatus? = nil
    ) -> AudioReadiness {
        switch status ?? SFSpeechRecognizer.authorizationStatus() {
        case .notDetermined: return .permissionRequired
        case .denied, .restricted: return .denied
        case .authorized:
            return recognizerAvailability(for: language) ? .available : .unavailable
        @unknown default: return .unavailable
        }
    }

    private func speechSystemReadiness() -> AudioReadiness {
        AVSpeechSynthesisVoice.speechVoices().isEmpty ? .unavailable : .available
    }

    private func resolveRecognitionRoute(
        configuration: AudioConfigurationV3,
        language: String,
        speechStatus: SFSpeechRecognizerAuthorizationStatus? = nil
    ) async -> AudioRouteResolution {
        let models = await offlineModelAvailability()
        return resolveAudioRoute(AudioRouteRequest(
            kind: .recognition,
            preference: configuration.recognition,
            language: language,
            systemStatus: recognitionSystemReadiness(language: language, status: speechStatus),
            offlineModels: models
        ))
    }

    private func resolveSpeechRoute(
        configuration: AudioConfigurationV3,
        language: String,
        voiceOverride: AudioVoiceSelection?
    ) async -> AudioRouteResolution {
        let models = await offlineModelAvailability()
        let systemVoices = AVSpeechSynthesisVoice.speechVoices()
        let voiceIdentities = systemVoices.map { SystemVoiceIdentity(identifier: $0.identifier, name: $0.name) }
        let preference = AudioSpeechPreference(
            source: configuration.speech.source,
            offlineModelId: configuration.speech.offlineModelId,
            voice: resolvingLegacySystemVoice(configuration.speech.voice, against: voiceIdentities)
        )
        return resolveAudioRoute(AudioRouteRequest(
            kind: .speech,
            preference: preference,
            language: language,
            systemStatus: speechSystemReadiness(),
            offlineModels: models,
            systemVoiceIds: systemVoices.map(\.identifier),
            voiceOverride: resolvingLegacySystemVoice(voiceOverride, against: voiceIdentities)
        ))
    }

    private func requireRoute(_ resolution: AudioRouteResolution) throws -> AudioRouteResolution.Effective {
        if resolution.status == .permissionRequired {
            throw HelperError.permission("the selected system audio service needs permission")
        }
        if resolution.status == .invalidRequest {
            throw HelperError.invalidRequest("the saved audio route is invalid: \(resolution.reason)")
        }
        guard let effective = resolution.effective, resolution.status == .ready else {
            if resolution.requested.source == .system && resolution.reason == "systemDenied" {
                throw HelperError.permission("system speech recognition permission is denied")
            }
            if resolution.requested.source == .offline {
                throw HelperError.modelMissing("the selected offline audio model or voice is unavailable: \(resolution.reason)")
            }
            throw HelperError.unavailable("the requested audio route is unavailable: \(resolution.reason)")
        }
        return effective
    }

    private func appendOperationTrace(
        identity: HelperAudioOperationIdentity,
        owner: HelperAudioOwner,
        operation: String,
        configuration: AudioConfigurationV3,
        revision: UInt64
    ) {
        let preference: AudioRoutePreferenceProviding?
        switch operation {
        case "listen": preference = configuration.recognition
        case "synthesize", "speak": preference = configuration.speech
        default: preference = nil
        }
        let trace = HelperAudioOperationTrace(
            identity: identity,
            owner: owner,
            operation: operation,
            configurationRevision: revision,
            requestedSource: preference?.source.rawValue,
            requestedModelId: preference?.offlineModelId,
            requestedVoiceId: preference?.routeVoice.map(voiceIdentifier),
            effectiveSource: nil,
            effectiveModelId: nil,
            effectiveVoiceId: nil,
            fallbackReason: nil
        )
        operationTraces.append(trace)
        if operationTraces.count > 64 { operationTraces.removeFirst(operationTraces.count - 64) }
    }

    private func updateOperationTrace(identity: HelperAudioOperationIdentity, route: AudioRouteResolution) {
        guard let index = operationTraces.lastIndex(where: { $0.identity == identity }) else { return }
        let old = operationTraces[index]
        operationTraces[index] = HelperAudioOperationTrace(
            identity: old.identity,
            owner: old.owner,
            operation: old.operation,
            configurationRevision: old.configurationRevision,
            requestedSource: route.requested.source.rawValue,
            requestedModelId: route.requested.offlineModelId,
            requestedVoiceId: route.requested.voice.map(voiceIdentifier),
            effectiveSource: route.effective?.source.rawValue,
            effectiveModelId: route.effective?.modelId,
            effectiveVoiceId: route.effective?.voiceId,
            fallbackReason: route.fallbackReason
        )
    }

    private func voiceIdentifier(_ voice: AudioVoiceSelection) -> String {
        if voice.source == .offline { return "sherpa:\(voice.modelId ?? ""):\(voice.id)" }
        return "system:\(voice.id)"
    }

    private func reservePhysical(identity: HelperAudioOperationIdentity, owner nextOwner: HelperOwner, activity nextActivity: String) throws {
        guard owner == nil, reservedPhysicalIdentity == nil, systemRenderIdentity == nil else {
            throw HelperError.busy("the device audio resource is already in use")
        }
        reservedPhysicalIdentity = identity
        reservedPhysicalOwner = nextOwner
        activity = nextActivity
    }

    private func releasePhysical(identity: HelperAudioOperationIdentity) {
        guard reservedPhysicalIdentity == identity || listeningIdentity == identity || speechIdentity == identity else { return }
        if listeningIdentity == identity {
            listeningIdentity = nil
            listeningOwner = nil
            activeSession = nil
        }
        if speechIdentity == identity {
            speechIdentity = nil
            activeSpeech = nil
        }
        if reservedPhysicalIdentity == identity {
            reservedPhysicalIdentity = nil
            reservedPhysicalOwner = nil
        }
        if owner == reservedPhysicalOwner || owner == listeningOwner || owner == speechRenderOwner?.helperOwner {
            owner = nil
            activity = "idle"
        } else if owner == nil {
            activity = "idle"
        }
        if speechRenderIdentity == identity {
            speechRenderIdentity = nil
            speechRenderOwner = nil
        }
    }

    private func startCapture(
        identity: HelperAudioOperationIdentity,
        owner requestOwner: HelperAudioOwner,
        language: String,
        sampleRate: Int,
        maximumPayloadBytes: Int,
        route: ListeningSession.Route,
        cancellation: AudioCancellationFlag,
        onSilence: @escaping @Sendable () async -> Void,
        onNoSpeech: @escaping @Sendable () async -> Void,
        onLimit: @escaping @Sendable () async -> Void
    ) async throws {
        try ensureCurrent(identity)
        guard (8_000...768_000).contains(sampleRate) else {
            throw HelperError.invalidRequest("sample rate must be between 8000 and 768000 Hz")
        }
        try reservePhysical(identity: identity, owner: requestOwner.helperOwner, activity: "listening")
        guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
            reservedPhysicalIdentity = nil
            reservedPhysicalOwner = nil
            activity = "idle"
            throw HelperError.permission("microphone access is required")
        }
        if route == .system, SFSpeechRecognizer.authorizationStatus() != .authorized {
            reservedPhysicalIdentity = nil
            reservedPhysicalOwner = nil
            activity = "idle"
            throw HelperError.permission("system Speech permission is required")
        }
        activeLanguage = language
        let maximumSamples = max(0, maximumPayloadBytes / 2)
        let session = await MainActor.run {
            ListeningSession(
                owner: requestOwner.helperOwner,
                languageIdentifier: language,
                route: route,
                outputSampleRate: sampleRate,
                maximumSampleCount: maximumSamples,
                cancellation: cancellation,
                onPartial: { [weak self] transcript in
                    await self?.publishRecognitionProgress(owner: requestOwner.helperOwner, text: transcript.text, isFinal: false)
                },
                onLevel: { [weak self] level in
                    await self?.publishInputLevel(owner: requestOwner.helperOwner, level: level)
                },
                onSilence: onSilence,
                onNoSpeech: onNoSpeech,
                onLimit: onLimit
            )
        }
        // Publish the provisional capture lease before starting the engine: AVAudioEngine
        // may deliver an oversized first buffer before `start()` returns to this actor.
        owner = requestOwner.helperOwner
        activity = "listening"
        listeningOwner = requestOwner.helperOwner
        listeningIdentity = identity
        activeSession = session
        reservedPhysicalIdentity = identity
        reservedPhysicalOwner = requestOwner.helperOwner
        do {
            try ensureCurrent(identity)
            try await MainActor.run { try session.start() }
            try ensureCurrent(identity)
            guard listeningIdentity == identity, activeSession === session,
                  reservedPhysicalIdentity == identity else {
                throw HelperError.cancelled("the microphone capture ended before it became active")
            }
        } catch {
            _ = await session.stop()
            if listeningIdentity == identity, activeSession === session {
                releaseCaptureLease(identity: identity, owner: requestOwner.helperOwner)
            } else if reservedPhysicalIdentity == identity {
                reservedPhysicalIdentity = nil
                reservedPhysicalOwner = nil
            }
            throw error
        }
        await publishOwnerChanged()
        await publishSnapshotChanged()
    }

    private func startRecording(
        identity: HelperAudioOperationIdentity,
        owner requestOwner: HelperAudioOwner,
        operation: [String: Any],
        configurationRevision: UInt64,
        maxPayloadBytes: UInt64,
        cancellation: AudioCancellationFlag
    ) async throws -> HelperEngineResult {
        let format = operation["format"] as? String ?? "wav"
        guard format == "wav" || format == "m4a" else {
            throw HelperError.invalidRequest("unsupported recording format \(format)")
        }
        let sampleRate = (operation["sample_rate_hz"] as? NSNumber)?.intValue ?? 16_000
        let payloadBytes = try payloadLimit(maxPayloadBytes)
        let sampleLimit = format == "wav" ? max(0, (payloadBytes - 44) / 2) : payloadBytes / 2
        guard sampleLimit > 0 else { throw HelperError.mediaTooLarge("the audio payload limit is too small to record") }
        try validateRecordingStartAvailability(
            existingHandle: recordingHandle,
            hasRecordingOrigin: recordingOriginIdentity != nil,
            physicalAudioBusy: owner != nil
                || reservedPhysicalIdentity != nil
                || listeningIdentity != nil
                || speechIdentity != nil
                || systemRenderIdentity != nil
        )
        let handle = UUID().uuidString.lowercased()
        try admitRecordingStart(
            existingHandle: &recordingHandle,
            existingOwner: &recordingOwner,
            existingOrigin: &recordingOriginIdentity,
            handle: handle,
            owner: requestOwner,
            origin: identity,
            physicalAudioBusy: owner != nil
                || reservedPhysicalIdentity != nil
                || listeningIdentity != nil
                || speechIdentity != nil
                || systemRenderIdentity != nil
        )
        recordingFormat = format
        recordingMaximumPayloadBytes = payloadBytes
        recordingConfigurationRevision = configurationRevision
        recordingFailure = nil
        do {
            try await startCapture(
                identity: identity,
                owner: requestOwner,
                language: currentLocaleTag(),
                sampleRate: sampleRate,
                maximumPayloadBytes: sampleLimit * 2,
                route: .captureOnly,
                cancellation: cancellation,
                onSilence: {},
                onNoSpeech: {},
                onLimit: { [weak self] in await self?.recordingLimitReached(identity: identity) }
            )
            try ensureCurrent(identity)
            if let session = activeSession, listeningIdentity == identity,
               let captureFailure = await session.waitForInitialCaptureBuffer(
                   timeoutNanoseconds: recordingInitialCaptureTimeoutNanoseconds
               ) {
                throw captureFailure
            }
            try validateRecordingStart(
                recordingFailure: recordingFailure,
                leaseIsActive: listeningIdentity == identity && activeSession != nil
            )
        } catch {
            let failure = recordingFailure ?? (error as? HelperError)
            await rollbackRecording(identity: identity)
            throw failure ?? error
        }
        activeRequestIdentity = identity
        activeRequestOwner = requestOwner
        updateOperationTrace(identity: identity, route: captureRouteResolution())
        await publishSnapshotChanged()
        return .recordingStarted(handle)
    }

    private func captureRouteResolution() -> AudioRouteResolution {
        let capture = AudioSource(rawValue: "capture")
        return AudioRouteResolution(
            requested: .init(source: capture, offlineModelId: nil, voice: nil),
            effective: .init(source: capture, modelId: nil, voiceId: nil),
            status: .ready,
            reason: "captureOnly",
            fallbackReason: nil
        )
    }

    private func stopRecording(
        identity: HelperAudioOperationIdentity,
        owner requestOwner: HelperAudioOwner,
        operation: [String: Any],
        cancellation: AudioCancellationFlag
    ) async throws -> HelperEngineResult {
        guard let handle = operation["handle"] as? String else {
            throw HelperError.notRecording("the recording handle is not active for this owner")
        }
        try validateRecordingHandle(
            requestedHandle: handle,
            activeHandle: recordingHandle,
            activeOwner: recordingOwner,
            requestedOwner: requestOwner
        )
        try ensureCurrent(identity)
        if let recordingFailure {
            await rollbackRecording(identity: recordingOriginIdentity)
            throw recordingFailure
        }
        guard let origin = recordingOriginIdentity, let activeSession, listeningIdentity == origin else {
            throw HelperError.notRecording("the recording is no longer active")
        }
        let captured = await activeSession.stop()
        let stillOwnsRecording = recordingOriginIdentity == origin && self.activeSession === activeSession
        let payload = try finalizeStoppedRecording(
            ownsCurrentLease: stillOwnsRecording,
            releaseLease: { self.clearRecordingLease(identity: origin) }
        ) {
            try ensureCurrent(identity)
            guard stillOwnsRecording else {
                throw HelperError.cancelled("the recording was ended by a newer audio operation")
            }
            guard captured.4 == nil else { throw captured.4! }
            let payload = try recordingPayload(
                samples: captured.0,
                sampleRate: captured.2,
                format: recordingFormat,
                maximumPayloadBytes: recordingMaximumPayloadBytes
            )
            try ensureInlineAudioFits(payload.audioBase64, label: "recording")
            return payload
        }
        appendOperationTrace(
            identity: identity,
            owner: requestOwner,
            operation: "stop_recording",
            configuration: audioConfiguration,
            revision: recordingConfigurationRevision
        )
        await publishOwnerChanged()
        await publishSnapshotChanged()
        return .recording(audioBase64: payload.audioBase64, mimeType: payload.mimeType)
    }

    private func listen(
        identity: HelperAudioOperationIdentity,
        owner requestOwner: HelperAudioOwner,
        operation: [String: Any],
        configuration: AudioConfigurationV3,
        configurationRevision: UInt64,
        maxPayloadBytes: UInt64,
        cancellation: AudioCancellationFlag
    ) async throws -> HelperEngineResult {
        let language = resolvedLanguage(operation["language"], configuration: configuration)
        let route = await resolveRecognitionRoute(configuration: configuration, language: language)
        updateOperationTrace(identity: identity, route: route)
        let effective = try requireRoute(route)
        let speechRoute: ListeningSession.Route = effective.source == .system ? .system : .captureOnly
        let payloadBytes = try payloadLimit(maxPayloadBytes)
        let selectedModel = effective.modelId
        if let selectedModel { _ = try await models.retain(modelID: selectedModel) }
        let key = helperAudioIdentityKey(identity)
        let waiter = AudioListenWaiter()
        listenWaiters[key] = waiter
        let context = ActiveListenContext(
            identity: identity,
            owner: requestOwner,
            language: language,
            route: route,
            cancellation: cancellation
        )
        recognitionSnapshot = HelperRecognitionSnapshot(
            requestedMode: route.requested.source == .offline ? "localOnly" : "automatic",
            effectiveBackend: effective.source == .system ? "apple" : "sherpa",
            effectiveLanguage: language,
            detail: effective.source == .system ? "Using Apple Speech for this live listen." : "Using the selected installed offline recognizer.",
            fallbackReason: route.fallbackReason
        )
        do {
            try await startCapture(
                identity: identity,
                owner: requestOwner,
                language: language,
                sampleRate: 16_000,
                maximumPayloadBytes: payloadBytes,
                route: speechRoute,
                cancellation: cancellation,
                onSilence: { [weak self] in
                    Task {
                        _ = try? await self?.finishListen(context: context)
                    }
                },
                onNoSpeech: { [weak self] in
                    Task {
                        await self?.failListen(
                            identity: identity,
                            error: .noSpeech("no speech was detected within the listening time limit")
                        )
                    }
                },
                onLimit: { [weak self] in
                    Task { await self?.failListen(identity: identity, error: .mediaTooLarge("captured audio exceeds the configured payload limit")) }
                }
            )
            activeListenContexts[key] = context
            if consumePendingListenFinish(context) {
                Task { [weak self] in _ = try? await self?.finishListen(context: context) }
            }
            let transcript = try await waiter.value()
            try ensureCurrent(identity)
            listenWaiters.removeValue(forKey: key)
            activeListenContexts.removeValue(forKey: key)
            listenFinalizers.removeValue(forKey: key)
            if let selectedModel { await models.release(modelID: selectedModel) }
            return .transcript(text: transcript.text, language: transcript.language, confidence: transcript.confidence)
        } catch {
            listenWaiters.removeValue(forKey: key)
            activeListenContexts.removeValue(forKey: key)
            listenFinalizers.removeValue(forKey: key)
            removePendingListenFinish(key)
            if let selectedModel { await models.release(modelID: selectedModel) }
            await stopCaptureIfCurrent(identity: identity)
            throw error
        }
    }

    private func finishListen(context: ActiveListenContext) async throws -> HelperTranscript {
        let key = helperAudioIdentityKey(context.identity)
        if let existing = listenFinalizers[key] { return try await existing.value }
        let task = Task { try await self.finalizeListen(context: context) }
        listenFinalizers[key] = task
        return try await task.value
    }

    private func finalizeListen(context: ActiveListenContext) async throws -> HelperTranscript {
        let key = helperAudioIdentityKey(context.identity)
        do {
            try ensureCurrent(context.identity)
            try context.cancellation.check()
            guard let session = activeSession, listeningIdentity == context.identity else {
                throw HelperError.cancelled("the live listening operation is no longer active")
            }
            let captured = await session.stop()
            try ensureCurrent(context.identity)
            try context.cancellation.check()
            releaseCaptureLease(identity: context.identity, owner: context.owner.helperOwner)
            if let captureFailure = captured.4 { throw captureFailure }
            guard !captured.0.isEmpty else { throw HelperError.noSpeech("no speech was captured") }
            let effective = try requireRoute(context.route)
            let transcript: HelperTranscript
            if effective.source == .system {
                if let liveTranscript = captured.1 { transcript = liveTranscript }
                else if let failure = captured.3 {
                    throw HelperError.unavailable("system Speech failed after listening began: \(failure)")
                } else {
                    throw HelperError.noSpeech("no speech was recognized")
                }
            } else {
                guard let modelID = effective.modelId else { throw HelperError.modelMissing("the selected offline recognizer is unavailable") }
                let decoded = try await Task.detached(priority: .userInitiated) {
                    try SherpaRuntime.transcribe(
                        samples: captured.0,
                        sampleRate: captured.2,
                        languageIdentifier: context.language,
                        modelID: modelID,
                        root: self.storageRoot
                    )
                }.value
                try ensureCurrent(context.identity)
                try context.cancellation.check()
                transcript = decoded
            }
            try ensureCurrent(context.identity)
            try context.cancellation.check()
            recognitionSnapshot = HelperRecognitionSnapshot(
                requestedMode: context.route.requested.source == .offline ? "localOnly" : "automatic",
                effectiveBackend: effective.source == .system ? "apple" : "sherpa",
                effectiveLanguage: context.language,
                detail: effective.source == .system ? "Apple Speech completed this live listen." : "The installed offline model completed this live listen.",
                fallbackReason: context.route.fallbackReason
            )
            listenWaiters[key]?.complete(.success(transcript))
            await publishRecognitionProgress(owner: context.owner.helperOwner, text: transcript.text, isFinal: true)
            await publishOwnerChanged()
            await publishSnapshotChanged()
            return transcript
        } catch let error as HelperError {
            listenWaiters[key]?.complete(.failure(error))
            await stopCaptureIfCurrent(identity: context.identity)
            await publishError(error, owner: context.owner.helperOwner)
            throw error
        } catch {
            let failure = HelperError.native(error.localizedDescription)
            listenWaiters[key]?.complete(.failure(failure))
            await stopCaptureIfCurrent(identity: context.identity)
            await publishError(failure, owner: context.owner.helperOwner)
            throw failure
        }
    }

    private func failListen(identity: HelperAudioOperationIdentity, error: HelperError) async {
        let key = helperAudioIdentityKey(identity)
        cancellationForActive(identity)?.cancel(with: error)
        listenWaiters.removeValue(forKey: key)?.complete(.failure(error))
        await stopCaptureIfCurrent(identity: identity)
        await publishError(error, owner: activeOperations[key]?.owner.helperOwner)
    }

    private func cancellationForActive(_ identity: HelperAudioOperationIdentity) -> AudioCancellationFlag? {
        activeOperations[helperAudioIdentityKey(identity)]?.cancellation
    }

    private func startPhysicalPlayback(identity: HelperAudioOperationIdentity, owner requestOwner: HelperAudioOwner) async throws -> PlaybackSession {
        try ensureCurrent(identity)
        try reservePhysical(identity: identity, owner: requestOwner.helperOwner, activity: "speaking")
        let session = await MainActor.run { PlaybackSession() }
        activeSpeech = session
        speechIdentity = identity
        speechRenderOwner = requestOwner
        speechRenderIdentity = identity
        owner = requestOwner.helperOwner
        activeLanguage = currentLocaleTag()
        return session
    }

    private func resolveEffectiveVoice(_ effective: AudioRouteResolution.Effective, language: String) throws -> (voiceID: String, modelID: String?) {
        if effective.source == .system {
            return ("system:\(effective.voiceId ?? "default")", nil)
        }
        guard let modelID = effective.modelId else { throw HelperError.modelMissing("the selected offline speech model is unavailable") }
        let selectedVoice = effective.voiceId
            ?? GeneratedVoiceModelCatalog.byID(modelID)?.voices.first(where: { languageBase($0.language) == languageBase(language) })?.id
        guard let selectedVoice else { throw HelperError.voiceMissing("the selected offline model has no voice for this language") }
        return ("sherpa:\(modelID):\(selectedVoice)", modelID)
    }

    private func automaticOfflineSpeechFallback(
        from requestedRoute: AudioRouteResolution,
        configuration: AudioConfigurationV3,
        language: String
    ) async -> AudioRouteResolution? {
        guard configuration.speech.source == .automatic, configuration.speech.voice == nil else { return nil }
        let fallbackPreference = AudioSpeechPreference(
            source: .offline,
            offlineModelId: configuration.speech.offlineModelId,
            voice: nil
        )
        let fallback = resolveAudioRoute(AudioRouteRequest(
            kind: .speech,
            preference: fallbackPreference,
            language: language,
            systemStatus: .unavailable,
            offlineModels: await offlineModelAvailability()
        ))
        guard fallback.status == .ready, let effective = fallback.effective, effective.source == .offline else { return nil }
        return AudioRouteResolution(
            requested: requestedRoute.requested,
            effective: effective,
            status: .ready,
            reason: "ready",
            fallbackReason: "systemStartupUnavailable"
        )
    }

    private func renderSpeech(
        text: String,
        voiceID: String,
        language: String,
        rate: Double,
        maximumPayloadBytes: Int,
        cancellation: AudioCancellationFlag,
        allowSystem: Bool
    ) async throws -> (pcm16: Data, sampleRate: Int32, modelID: String?) {
        try cancellation.check()
        if voiceID.hasPrefix("system:") {
            guard allowSystem else { throw HelperError.invalidRequest("system rendering is not allowed for this operation") }
            let voice = try resolvePlaybackVoice(voiceID, language: language, root: storageRoot)
            guard case let .system(systemVoice, _) = voice else { throw HelperError.voiceMissing("the selected system voice is unavailable") }
            let rendered = try await renderSystemPCM(
                text: text,
                voice: systemVoice,
                rate: rate,
                maximumPayloadBytes: maximumPayloadBytes,
                cancellation: cancellation
            )
            return (rendered.0, rendered.1, nil)
        }
        guard let route = SherpaRuntime.voiceEntry(for: voiceID, languageIdentifier: language, root: storageRoot) else {
            if voiceID.hasPrefix("sherpa:") { throw HelperError.voiceMissing("the selected offline voice is unavailable") }
            throw HelperError.modelMissing("the selected offline speech model is unavailable")
        }
        let modelID = route.0.id
        guard !offlineRenderModels.contains(modelID) else {
            throw HelperError.busy("the selected offline model is already rendering audio")
        }
        offlineRenderModels.insert(modelID)
        var retained = false
        do {
            _ = try await models.retain(modelID: modelID)
            retained = true
            let generated = try await Task.detached(priority: .userInitiated) {
                try SherpaRuntime.synthesize(
                    text: text,
                    voiceID: voiceID,
                    languageIdentifier: language,
                    rate: rate,
                    maximumPayloadBytes: maximumPayloadBytes,
                    cancellation: cancellation,
                    root: self.storageRoot
                )
            }.value
            try cancellation.check()
            guard !generated.0.isEmpty, generated.0.count.isMultiple(of: 2),
                  generated.0.count <= maximumPayloadBytes,
                  (8_000...768_000).contains(generated.1) else {
                throw HelperError.synthesisFailed("the offline synthesizer returned invalid PCM audio")
            }
            await models.release(modelID: modelID)
            retained = false
            offlineRenderModels.remove(modelID)
            return (generated.0, generated.1, modelID)
        } catch {
            if retained { await models.release(modelID: modelID) }
            offlineRenderModels.remove(modelID)
            throw error
        }
    }

    private func synthesize(
        identity: HelperAudioOperationIdentity,
        owner requestOwner: HelperAudioOwner,
        operation: [String: Any],
        configuration: AudioConfigurationV3,
        configurationRevision: UInt64,
        maxPayloadBytes: UInt64,
        cancellation: AudioCancellationFlag
    ) async throws -> HelperEngineResult {
        guard let text = operation["text"] as? String, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              text.count <= 100_000 else {
            throw HelperError.invalidRequest("synthesis text must be nonempty and within the supported size limit")
        }
        let language = resolvedLanguage(operation["language"], configuration: configuration)
        let utteranceConfiguration = configurationForSingleUtterance(configuration, operation: operation)
        let override = voiceOverride(operation["voice"])
        let route = await resolveSpeechRoute(configuration: utteranceConfiguration, language: language, voiceOverride: override)
        updateOperationTrace(identity: identity, route: route)
        let effective = try requireRoute(route)
        let voice = try resolveEffectiveVoice(effective, language: language)
        let limit = try payloadLimit(maxPayloadBytes)
        if effective.source == .system {
            guard owner == nil, reservedPhysicalIdentity == nil, systemRenderIdentity == nil else {
                throw HelperError.busy("system speech rendering conflicts with active device audio")
            }
            systemRenderIdentity = identity
        }
        defer { if systemRenderIdentity == identity { systemRenderIdentity = nil } }
        playbackSnapshot = try resolvePlaybackVoice(voice.voiceID, language: language, root: storageRoot).snapshot
        let rendered = try await renderSpeech(
            text: text,
            voiceID: voice.voiceID,
            language: language,
            rate: (operation["rate"] as? NSNumber)?.doubleValue ?? configuration.rate,
            maximumPayloadBytes: limit,
            cancellation: cancellation,
            allowSystem: true
        )
        try ensureCurrent(identity)
        let encoded = rendered.pcm16.base64EncodedString()
        try ensureInlineAudioFits(encoded, label: "synthesized")
        return .synthesized(pcmBase64: encoded, sampleRateHz: UInt32(rendered.sampleRate))
    }

    private func speak(
        identity: HelperAudioOperationIdentity,
        owner requestOwner: HelperAudioOwner,
        operation: [String: Any],
        configuration: AudioConfigurationV3,
        configurationRevision: UInt64,
        maxPayloadBytes: UInt64,
        cancellation: AudioCancellationFlag
    ) async throws -> HelperEngineResult {
        guard let text = operation["text"] as? String, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              text.count <= 100_000 else {
            throw HelperError.invalidRequest("speech text must be nonempty and within the supported size limit")
        }
        let language = resolvedLanguage(operation["language"], configuration: configuration)
        let utteranceConfiguration = configurationForSingleUtterance(configuration, operation: operation)
        let route = await resolveSpeechRoute(
            configuration: utteranceConfiguration,
            language: language,
            voiceOverride: voiceOverride(operation["voice"])
        )
        updateOperationTrace(identity: identity, route: route)
        let effective = try requireRoute(route)
        let voice = try resolveEffectiveVoice(effective, language: language)
        let session = try await startPhysicalPlayback(identity: identity, owner: requestOwner)
        let startedAt = Date()
        do {
            try cancellation.check()
            if effective.source == .system {
                let resolved = try resolvePlaybackVoice(voice.voiceID, language: language, root: storageRoot)
                guard case let .system(systemVoice, snapshot) = resolved else { throw HelperError.voiceMissing("the selected system voice is unavailable") }
                playbackSnapshot = snapshot
                do {
                    try await session.playSystem(
                        text: text,
                        voice: systemVoice,
                        rate: Float(max(0.5, min(2, (operation["rate"] as? NSNumber)?.doubleValue ?? configuration.rate))) * 0.28,
                        cancellation: cancellation
                    )
                } catch let startupFailure as HelperError {
                    let didStartSystemOutput = await MainActor.run { session.hasStartedOutput }
                    guard startupFailure.code == "unavailable",
                          !didStartSystemOutput,
                          cancellation.isCancelled == false,
                          let fallbackRoute = await automaticOfflineSpeechFallback(
                              from: route,
                              configuration: utteranceConfiguration,
                              language: language
                          ),
                          isAudioFallbackAllowed(.unavailable, operationStarted: didStartSystemOutput) else {
                        throw startupFailure
                    }
                    updateOperationTrace(identity: identity, route: fallbackRoute)
                    let fallbackEffective = try requireRoute(fallbackRoute)
                    let fallbackVoice = try resolveEffectiveVoice(fallbackEffective, language: language)
                    let rendered = try await renderSpeech(
                        text: text,
                        voiceID: fallbackVoice.voiceID,
                        language: language,
                        rate: (operation["rate"] as? NSNumber)?.doubleValue ?? configuration.rate,
                        maximumPayloadBytes: try payloadLimit(maxPayloadBytes),
                        cancellation: cancellation,
                        allowSystem: false
                    )
                    try ensureCurrent(identity)
                    playbackSnapshot = try resolvePlaybackVoice(fallbackVoice.voiceID, language: language, root: storageRoot).snapshot
                    try await session.playPCM16(rendered.pcm16, sampleRate: rendered.sampleRate, cancellation: cancellation)
                }
            } else {
        let rendered = try await renderSpeech(
                    text: text,
                    voiceID: voice.voiceID,
                    language: language,
                    rate: (operation["rate"] as? NSNumber)?.doubleValue ?? configuration.rate,
                    maximumPayloadBytes: try payloadLimit(maxPayloadBytes),
                    cancellation: cancellation,
                    allowSystem: false
                )
                try cancellation.check()
                playbackSnapshot = try resolvePlaybackVoice(voice.voiceID, language: language, root: storageRoot).snapshot
                try await session.playPCM16(rendered.pcm16, sampleRate: rendered.sampleRate, cancellation: cancellation)
            }
            try ensureCurrent(identity)
            clearPhysicalLease(identity: identity)
            await publishSpeechState(owner: requestOwner.helperOwner, state: "finished")
            await publishOwnerChanged()
            await publishSnapshotChanged()
            _ = configurationRevision
            return .playbackCompleted(durationMs: UInt64(max(0, Date().timeIntervalSince(startedAt) * 1000)))
        } catch {
            if speechIdentity == identity || reservedPhysicalIdentity == identity {
                clearPhysicalLease(identity: identity)
                await publishSpeechState(owner: requestOwner.helperOwner, state: "interrupted")
                await publishOwnerChanged()
                await publishSnapshotChanged()
            }
            throw error
        }
    }

    @MainActor
    private func renderSystemPCM(
        text: String,
        voice: AVSpeechSynthesisVoice?,
        rate: Double,
        maximumPayloadBytes: Int,
        cancellation: AudioCancellationFlag
    ) async throws -> (Data, Int32) {
        let collector = SystemPCMCollector(maximumBytes: maximumPayloadBytes, cancellation: cancellation)
        let synthesizerHandle = SpeechSynthesizerCancellationHandle(AVSpeechSynthesizer())
        let synthesizer = synthesizerHandle.synthesizer
        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = voice
        utterance.rate = Float(max(0.5, min(2, rate))) * 0.28
        let cancellationHandler = cancellation.installCancellationHandler {
            collector.cancel(with: cancellation.terminalError() ?? .cancelled("system audio synthesis was cancelled"))
            Task { @MainActor in synthesizerHandle.synthesizer.stopSpeaking(at: .immediate) }
        }
        defer { cancellation.removeCancellationHandler(cancellationHandler) }
        try cancellation.check()
        let result = try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<(Data, Int32), Error>) in
                collector.install(continuation)
                synthesizer.write(utterance) { buffer in collector.append(buffer) }
            }
        } onCancel: {
            cancellation.cancel()
        }
        try cancellation.check()
        return result
    }

    private func recordingLimitReached(identity: HelperAudioOperationIdentity) async {
        guard listeningIdentity == identity, let activeSession else { return }
        let result = await activeSession.stop()
        guard listeningIdentity == identity, self.activeSession === activeSession else { return }
        recordingFailure = result.4 ?? .mediaTooLarge("captured audio exceeds the configured payload limit")
        releaseCaptureLease(identity: identity, owner: recordingOwner?.helperOwner)
        await publishError(recordingFailure!, owner: recordingOwner?.helperOwner)
        await publishSnapshotChanged()
    }

    private func rollbackRecording(identity: HelperAudioOperationIdentity?, publishEvents: Bool = true) async {
        guard let identity, recordingOriginIdentity == identity else { return }
        if let activeSession, listeningIdentity == identity {
            _ = await activeSession.stop()
        }
        guard recordingOriginIdentity == identity else { return }
        clearRecordingLease(identity: identity)
        if publishEvents {
            await publishOwnerChanged()
            await publishSnapshotChanged()
        }
    }

    private func clearRecordingLease(identity: HelperAudioOperationIdentity) {
        guard recordingOriginIdentity == identity else { return }
        if listeningIdentity == identity {
            listeningIdentity = nil
            listeningOwner = nil
            activeSession = nil
        }
        if reservedPhysicalIdentity == identity {
            reservedPhysicalIdentity = nil
            reservedPhysicalOwner = nil
        }
        if owner == recordingOwner?.helperOwner {
            owner = nil
            activity = "idle"
        }
        recordingHandle = nil
        recordingOwner = nil
        recordingOriginIdentity = nil
        recordingFailure = nil
    }

    private func releaseCaptureLease(identity: HelperAudioOperationIdentity, owner expectedOwner: HelperOwner?) {
        guard listeningIdentity == identity else { return }
        listeningIdentity = nil
        listeningOwner = nil
        activeSession = nil
        if reservedPhysicalIdentity == identity {
            reservedPhysicalIdentity = nil
            reservedPhysicalOwner = nil
        }
        if owner == expectedOwner {
            owner = nil
            activity = "idle"
        }
    }

    private func stopCaptureIfCurrent(identity: HelperAudioOperationIdentity, publishEvents: Bool = true) async {
        guard listeningIdentity == identity, let activeSession else { return }
        _ = await activeSession.stop()
        if listeningIdentity == identity {
            releaseCaptureLease(identity: identity, owner: listeningOwner)
            if recordingOriginIdentity == identity { clearRecordingLease(identity: identity) }
            if publishEvents {
                await publishOwnerChanged()
                await publishSnapshotChanged()
            }
        }
    }

    private func clearPhysicalLease(identity: HelperAudioOperationIdentity) {
        let expectedOwner = reservedPhysicalOwner
            ?? activeOperations[helperAudioIdentityKey(identity)]?.owner.helperOwner
            ?? listeningOwner
            ?? speechRenderOwner?.helperOwner
        if speechIdentity == identity {
            speechIdentity = nil
            activeSpeech = nil
        }
        if reservedPhysicalIdentity == identity {
            reservedPhysicalIdentity = nil
            reservedPhysicalOwner = nil
        }
        if speechRenderIdentity == identity {
            speechRenderIdentity = nil
            speechRenderOwner = nil
        }
        if expectedOwner != nil, owner == expectedOwner {
            owner = nil
            activity = "idle"
        } else if owner == nil, reservedPhysicalIdentity == nil, listeningIdentity == nil, speechIdentity == nil {
            activity = "idle"
        }
    }

    private func endAudioOwner(identity: HelperAudioOperationIdentity, owner requestOwner: HelperAudioOwner) async throws {
        let ownerKey = helperAudioOwnerKey(requestOwner)
        endingOwners[ownerKey, default: 0] += 1
        removePendingListenFinishes(ownerKey: ownerKey)
        let active = activeOperations.filter { key, value in
            key != helperAudioIdentityKey(identity)
                && value.operation != "end_owner"
                && helperAudioOwnerKey(value.owner) == ownerKey
        }
        for (key, operation) in active {
            rememberCancelledIdentity(key)
            operation.cancellation.cancel()
            listenWaiters.removeValue(forKey: key)?.complete(.failure(.cancelled("the audio owner ended this operation")))
        }
        if let origin = recordingOriginIdentity, recordingOwner.map(helperAudioOwnerKey) == ownerKey {
            await rollbackRecording(identity: origin)
        }
        if let listeningIdentity,
           active.first(where: { $0.value.identity == listeningIdentity }) != nil {
            await stopCaptureIfCurrent(identity: listeningIdentity)
        }
        if let speechIdentity,
           active.first(where: { $0.value.identity == speechIdentity }) != nil {
            if let activeSpeech { await MainActor.run { activeSpeech.stop() } }
            clearPhysicalLease(identity: speechIdentity)
        }
        let settled = await waitForOperations(Array(active.keys), timeoutMs: 2_000)
        let endCount = endingOwners[ownerKey, default: 1]
        if endCount <= 1 { endingOwners.removeValue(forKey: ownerKey) }
        else { endingOwners[ownerKey] = endCount - 1 }
        guard settled else {
            throw HelperError.native("audio owner teardown timed out waiting for native operations to stop")
        }
        await publishOwnerChanged()
        await publishSnapshotChanged()
    }

    private func waitForOperations(_ keys: [String], timeoutMs: UInt64) async -> Bool {
        guard !keys.isEmpty else { return true }
        return await withTaskGroup(of: Bool.self, returning: Bool.self) { group in
            for key in keys {
                group.addTask { await self.waitForOperation(key, timeoutMs: timeoutMs) }
            }
            var allSettled = true
            for await settled in group {
                if !settled { allSettled = false }
            }
            return allSettled
        }
    }

    private func waitForOperation(_ key: String, timeoutMs: UInt64) async -> Bool {
        guard activeOperations[key] != nil || finishingOperations.contains(key) else { return true }
        let token = UUID()
        return await withCheckedContinuation { continuation in
            guard activeOperations[key] != nil || finishingOperations.contains(key) else {
                continuation.resume(returning: true)
                return
            }
            operationCompletionWaiters[key, default: [:]][token] = continuation
            Task { [weak self] in
                do { try await Task.sleep(nanoseconds: timeoutMs * 1_000_000) }
                catch { return }
                await self?.expireOperationWaiter(key: key, token: token)
            }
        }
    }

    private func expireOperationWaiter(key: String, token: UUID) {
        guard let continuation = operationCompletionWaiters[key]?.removeValue(forKey: token) else { return }
        if operationCompletionWaiters[key]?.isEmpty == true {
            operationCompletionWaiters.removeValue(forKey: key)
        }
        continuation.resume(returning: false)
    }

    private func timeoutOperation(identity: HelperAudioOperationIdentity) async {
        let key = helperAudioIdentityKey(identity)
        guard let active = activeOperations[key] else { return }
        rememberCancelledIdentity(key)
        removePendingListenFinish(key)
        active.cancellation.cancel(with: .timeout("the audio operation timed out"))
        listenWaiters.removeValue(forKey: key)?.complete(.failure(.timeout("the audio operation timed out")))
        if let recordingToRollback = recordingOriginToRollback(
            for: identity,
            operation: active.operation,
            owner: active.owner,
            recordingOrigin: recordingOriginIdentity,
            recordingOwner: recordingOwner
        ) {
            await rollbackRecording(identity: recordingToRollback)
        }
        if listeningIdentity == identity { await stopCaptureIfCurrent(identity: identity) }
        if speechIdentity == identity {
            if let activeSpeech { await MainActor.run { activeSpeech.stop() } }
            clearPhysicalLease(identity: identity)
        }
        await publishError(.timeout("the audio operation timed out"), owner: active.owner.helperOwner)
        await publishSnapshotChanged()
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
            return "cancelled"
        case .timeout:
            return "timeout"
        case .noSpeech:
            return "no_speech"
        case .notRecording:
            return "not_recording"
        case .unsupported:
            return "unsupported"
        case .modelMissing:
            return "model_missing"
        case .voiceMissing:
            return "voice_missing"
        case .invalidRequest:
            return "invalid_request"
        case .synthesisFailed:
            return "synthesis_failed"
        case .mediaTooLarge:
            return "media_too_large"
        case .unavailable:
            return "unavailable"
        case .download, .checksum, .native:
            return "native_failure"
        }
    }
}

private actor HelperInputTasks {
    private struct Entry {
        let identityKey: String?
        let ownerKey: String?
        let operation: String?
        let task: Task<Void, Never>
    }

    private let maximumInFlight = 16
    private var entries: [String: Entry] = [:]

    func start(
        id: String,
        identityKey: String?,
        ownerKey: String? = nil,
        operationType: String? = nil,
        operation: @escaping @Sendable () async -> Void
    ) -> Bool {
        guard entries.count < maximumInFlight, entries[id] == nil else { return false }
        let task = Task {
            await operation()
            self.finished(id: id)
        }
        entries[id] = Entry(identityKey: identityKey, ownerKey: ownerKey, operation: operationType, task: task)
        return true
    }

    func cancel(identityKey: String) {
        for entry in entries.values where entry.identityKey == identityKey {
            entry.task.cancel()
        }
    }

    func cancel(ownerKey: String, excludingID: String) {
        for (id, entry) in entries
            where id != excludingID && entry.ownerKey == ownerKey && entry.operation != "end_owner" {
            entry.task.cancel()
        }
    }

    func cancelAll() {
        for entry in entries.values { entry.task.cancel() }
        entries.removeAll()
    }

    private func finished(id: String) {
        entries.removeValue(forKey: id)
    }
}

func audioIdentityKey(_ value: Any?) -> String? {
    guard let identity = value as? [String: Any],
          let id = identity["id"] as? String,
          let generation = identity["generation"] as? NSNumber,
          let serviceEpoch = identity["service_epoch"] as? NSNumber else {
        return nil
    }
    return "\(serviceEpoch.uint64Value):\(generation.uint64Value):\(id)"
}

func readInputLines<S: AsyncSequence>(
    _ lines: S,
    handle: @escaping @Sendable ([String: Any]) async -> Void,
    cancel: @escaping @Sendable ([String: Any]) async -> Void,
    reject: @escaping @Sendable ([String: Any]) async -> Void = { _ in }
) async where S.Element == String {
    let tasks = HelperInputTasks()
    var iterator = lines.makeAsyncIterator()
    while true {
        let nextLine: String?
        do { nextLine = try await iterator.next() }
        catch { break }
        guard let line = nextLine else { break }
        guard let data = line.data(using: .utf8), !data.isEmpty,
              let parsed = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { continue }
        let id = parsed["id"] as? String ?? UUID().uuidString
        let command = parsed["command"] as? [String: Any]
        if command?["type"] as? String == "cancel_operation",
           let identityKey = audioIdentityKey(command?["identity"]) {
            await cancel(parsed)
            await tasks.cancel(identityKey: identityKey)
            if !(await tasks.start(id: id, identityKey: nil) { await handle(parsed) }) {
                await reject(parsed)
            }
            continue
        }
        let request = parsed["request"] as? [String: Any] ?? [:]
        let operation = request["operation"] as? [String: Any] ?? [:]
        let owner = parseHelperAudioOwner(request["owner"] as? [String: Any] ?? [:])
        let identityKey = parsed["kind"] as? String == "engine_request"
            ? audioIdentityKey(request["identity"])
            : nil
        let ownerKey = owner.map(helperAudioOwnerKey)
        let accepted = await tasks.start(
            id: id,
            identityKey: identityKey,
            ownerKey: ownerKey,
            operationType: operation["type"] as? String
        ) { await handle(parsed) }
        if accepted, operation["type"] as? String == "end_owner", let ownerKey {
            await tasks.cancel(ownerKey: ownerKey, excludingID: id)
        } else if !accepted {
            await reject(parsed)
        }
    }
    await tasks.cancelAll()
}

func handleHelperInputEnvelope(_ parsed: [String: Any], state: HelperStateStore) async {
    let id = parsed["id"] as? String ?? UUID().uuidString
    guard parsed["kind"] as? String == "engine_request" else {
        await state.handleCommand(id: id, input: parsed["command"] as? [String: Any] ?? [:])
        return
    }
    let request = parsed["request"] as? [String: Any] ?? [:]
    guard let identity = parseHelperAudioIdentity(request["identity"] as? [String: Any] ?? [:]),
          let owner = parseHelperAudioOwner(request["owner"] as? [String: Any] ?? [:]) else {
        await state.rejectOverloaded(parsed)
        return
    }
    let operation = request["operation"] as? [String: Any] ?? [:]
    let configuration = AudioConfigurationNormalizer.normalize(parsed["configuration"])
    let revision = (parsed["configurationRevision"] as? NSNumber)?.uint64Value ?? 0
    let maxPayloadBytes = (request["max_payload_bytes"] as? NSNumber)?.uint64Value
        ?? UInt64(maxInlineAudioBase64Bytes * 3 / 4)
    let timeoutBudgetMs = (request["timeout_budget_ms"] as? NSNumber)?.uint64Value
    await state.handleEngineRequest(
        id: id,
        identity: identity,
        owner: owner,
        operation: operation,
        configuration: configuration,
        configurationRevision: revision,
        maxPayloadBytes: maxPayloadBytes,
        timeoutBudgetMs: timeoutBudgetMs
    )
}

func cancelHelperInputEnvelope(_ parsed: [String: Any], state: HelperStateStore) async {
    let command = parsed["command"] as? [String: Any] ?? [:]
    await state.cancelOperation(command["identity"] as? [String: Any] ?? [:])
}

func runHelperInputLoop<S: AsyncSequence>(_ lines: S, state: HelperStateStore) async where S.Element == String {
    await readInputLines(lines, handle: { parsed in
        await handleHelperInputEnvelope(parsed, state: state)
    }, cancel: { parsed in
        await cancelHelperInputEnvelope(parsed, state: state)
    }, reject: { parsed in
        await state.rejectOverloaded(parsed)
    })
    await state.shutdown()
}

func readInputLoop(state: HelperStateStore) async {
    await runHelperInputLoop(FileHandle.standardInput.bytes.lines, state: state)
}

@main
struct LingXiAudioHelperApp {
    static func main() async {
        do {
            if let permissions = try permissionRequestFromArguments(CommandLine.arguments) {
                try await requestForegroundSystemPermissions(permissions)
                return
            }
        } catch {
            let message = (error as? HelperError)?.message ?? error.localizedDescription
            if let data = "audio permission helper failed: \(message)\n".data(using: .utf8) {
                try? FileHandle.standardError.write(contentsOf: data)
            }
            return
        }
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
