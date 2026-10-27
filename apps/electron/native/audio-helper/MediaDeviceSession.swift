import Foundation
import AudioToolbox
@preconcurrency import AVFoundation

private let helperMediaCaptureSampleRate = 24_000
private let helperMediaMaximumFrameBytes = 8 * 1024 * 1024
private let helperMediaMaximumCaptureFrameBytes = 64 * 1024
private let helperMediaCaptureQueueLimit = 32
private let helperMediaPlaybackQueueSeconds = 2
private let helperMediaPlaybackChunkLimit = 128

struct HelperMediaOpenRequest: Sendable {
    let identity: HelperAudioOperationIdentity
    let owner: HelperAudioOwner
    let sourceID: String
    let maxPayloadBytes: Int
}

struct HelperMediaPcmInput: Sendable {
    let sampleRateHz: Int
    let channels: Int
    let bytes: Data
}

enum HelperMediaUpdate: Sendable {
    case setAudioEnabled(Bool)
    case selectAudioSource(String)
    case setPlaybackGain(Float)
}

enum HelperMediaDeviceOperation: Sendable {
    case open(HelperMediaOpenRequest)
    case nextInput(String)
    case writeAudio(String, HelperMediaPcmInput)
    case writeVideo(String)
    case flushAudio(String)
    case interruptAudio(String)
    case update(String, HelperMediaUpdate)
    case stop(String)
}

struct HelperMediaDeviceRequest: Sendable {
    let requestID: String
    let operation: HelperMediaDeviceOperation
}

private func mediaIdentifier(_ value: Any?, label: String) throws -> String {
    guard let value = value as? String,
          !value.isEmpty,
          value.utf8.count <= 256,
          !value.contains("\0") else {
        throw HelperError.invalidRequest("invalid \(label)")
    }
    return value
}

private func exactKeys(_ value: [String: Any], _ allowed: Set<String>, label: String) throws {
    guard Set(value.keys).isSubset(of: allowed) else {
        throw HelperError.invalidRequest("invalid \(label) fields")
    }
}

private func mediaDictionary(_ value: Any?, label: String) throws -> [String: Any] {
    guard let value = value as? [String: Any] else {
        throw HelperError.invalidRequest("invalid \(label)")
    }
    return value
}

private func mediaInteger(_ value: Any?, label: String, minimum: Int, maximum: Int) throws -> Int {
    guard let number = value as? NSNumber else {
        throw HelperError.invalidRequest("invalid \(label)")
    }
    let double = number.doubleValue
    guard double.isFinite, double.rounded(.towardZero) == double,
          double >= Double(minimum), double <= Double(maximum) else {
        throw HelperError.invalidRequest("invalid \(label)")
    }
    return number.intValue
}

private func mediaBytes(_ value: Any?, maximum: Int, label: String) throws -> Data {
    guard let numbers = value as? [NSNumber],
          !numbers.isEmpty,
          numbers.count <= maximum else {
        throw HelperError.mediaTooLarge("\(label) exceeds its byte limit")
    }
    var bytes = Data(capacity: numbers.count)
    for number in numbers {
        let double = number.doubleValue
        guard double.isFinite, double.rounded(.towardZero) == double,
              double >= 0, double <= 255 else {
            throw HelperError.invalidRequest("invalid \(label) byte")
        }
        bytes.append(UInt8(number.intValue))
    }
    return bytes
}

func parseHelperMediaDeviceRequest(_ raw: Any?) throws -> HelperMediaDeviceRequest {
    let envelope = try mediaDictionary(raw, label: "media device request envelope")
    try exactKeys(envelope, ["request_id", "request"], label: "media device request envelope")
    let requestID = try mediaIdentifier(envelope["request_id"], label: "media request ID")
    let request = try mediaDictionary(envelope["request"], label: "media device request")
    guard let type = request["type"] as? String else {
        throw HelperError.invalidRequest("media device request has no type")
    }

    switch type {
    case "open":
        try exactKeys(request, ["type", "request"], label: "media open request")
        let open = try mediaDictionary(request["request"], label: "media open request body")
        try exactKeys(open, ["identity", "owner", "initiator", "timeout_budget_ms", "max_payload_bytes", "request"], label: "media open request body")
        let rawIdentity = try mediaDictionary(open["identity"], label: "media operation identity")
        try exactKeys(rawIdentity, ["id", "generation", "service_epoch"], label: "media operation identity")
        guard let identity = parseHelperAudioIdentity(rawIdentity),
              identity.serviceEpoch > 0 else {
            throw HelperError.invalidRequest("media open requires a valid operation identity")
        }
        let rawOwner = try mediaDictionary(open["owner"], label: "media owner")
        let ownerType = rawOwner["type"] as? String ?? ""
        if ownerType == "session" {
            try exactKeys(rawOwner, ["type", "session_id"], label: "media session owner")
        } else {
            try exactKeys(rawOwner, ["type", "instance_id"], label: "media app owner")
        }
        guard let owner = parseHelperAudioOwner(rawOwner) else {
            throw HelperError.invalidRequest("media open requires a valid owner")
        }
        if let initiator = open["initiator"], !(initiator is NSNull) {
            let value = try mediaDictionary(initiator, label: "media initiator")
            try exactKeys(value, ["agent_id", "tool_use_id", "request_id"], label: "media initiator")
            for key in ["agent_id", "tool_use_id", "request_id"] {
                if let item = value[key], !(item is NSNull) {
                    _ = try mediaIdentifier(item, label: "media initiator ID")
                }
            }
        }
        if let timeout = open["timeout_budget_ms"], !(timeout is NSNull) {
            _ = try mediaInteger(timeout, label: "media timeout", minimum: 1, maximum: 3_600_000)
        }
        let maxPayloadBytes = try mediaInteger(open["max_payload_bytes"], label: "media payload limit", minimum: 1, maximum: helperMediaMaximumFrameBytes)
        let sessionRequest = try mediaDictionary(open["request"], label: "media session request")
        try exactKeys(sessionRequest, ["mode", "intent", "audio_source", "video_source"], label: "media session request")
        guard sessionRequest["mode"] as? String == "audio" else {
            throw HelperError.unsupported("the Desktop native media helper supports audio sessions only")
        }
        guard ["observation", "conversation"].contains(sessionRequest["intent"] as? String ?? "") else {
            throw HelperError.invalidRequest("invalid media session intent")
        }
        if let videoSource = sessionRequest["video_source"], !(videoSource is NSNull) {
            throw HelperError.unsupported("the Desktop native media helper has no video capture backend")
        }
        let sourceID: String
        if let audioSource = sessionRequest["audio_source"], !(audioSource is NSNull) {
            let source = try mediaDictionary(audioSource, label: "media audio source")
            try exactKeys(source, ["id"], label: "media audio source")
            sourceID = try mediaIdentifier(source["id"], label: "media audio source reference")
        } else {
            sourceID = "media-runtime:microphone"
        }
        guard sourceID == "media-runtime:microphone" else {
            throw HelperError.notRecording("the selected native microphone source is not available")
        }
        return HelperMediaDeviceRequest(
            requestID: requestID,
            operation: .open(HelperMediaOpenRequest(
                identity: identity,
                owner: owner,
                sourceID: sourceID,
                maxPayloadBytes: maxPayloadBytes
            ))
        )
    case "next_input", "stop":
        try exactKeys(request, ["type", "lease_id"], label: "media lease request")
        let leaseID = try mediaIdentifier(request["lease_id"], label: "media lease ID")
        return HelperMediaDeviceRequest(requestID: requestID, operation: type == "next_input" ? .nextInput(leaseID) : .stop(leaseID))
    case "write_audio":
        try exactKeys(request, ["type", "request"], label: "media audio write")
        let write = try mediaDictionary(request["request"], label: "media audio write request")
        try exactKeys(write, ["lease_id", "frame"], label: "media audio write request")
        let leaseID = try mediaIdentifier(write["lease_id"], label: "media lease ID")
        let frame = try mediaDictionary(write["frame"], label: "PCM frame")
        try exactKeys(frame, ["sample_rate_hz", "channels", "pcm16le"], label: "PCM frame")
        let sampleRate = try mediaInteger(frame["sample_rate_hz"], label: "PCM sample rate", minimum: 8_000, maximum: 768_000)
        let channels = try mediaInteger(frame["channels"], label: "PCM channel count", minimum: 1, maximum: 8)
        let bytes = try mediaBytes(frame["pcm16le"], maximum: helperMediaMaximumFrameBytes, label: "PCM audio")
        guard bytes.count.isMultiple(of: channels * 2) else {
            throw HelperError.invalidRequest("PCM audio contains a partial sample frame")
        }
        return HelperMediaDeviceRequest(requestID: requestID, operation: .writeAudio(leaseID, HelperMediaPcmInput(sampleRateHz: sampleRate, channels: channels, bytes: bytes)))
    case "write_video":
        try exactKeys(request, ["type", "request"], label: "media video write")
        let write = try mediaDictionary(request["request"], label: "media video write request")
        try exactKeys(write, ["lease_id", "frame"], label: "media video write request")
        let leaseID = try mediaIdentifier(write["lease_id"], label: "media lease ID")
        return HelperMediaDeviceRequest(requestID: requestID, operation: .writeVideo(leaseID))
    case "flush_audio", "interrupt_audio":
        try exactKeys(request, ["type", "request"], label: "media audio control")
        let control = try mediaDictionary(request["request"], label: "media audio control request")
        try exactKeys(control, ["lease_id"], label: "media audio control request")
        let leaseID = try mediaIdentifier(control["lease_id"], label: "media lease ID")
        return HelperMediaDeviceRequest(requestID: requestID, operation: type == "flush_audio" ? .flushAudio(leaseID) : .interruptAudio(leaseID))
    case "update":
        try exactKeys(request, ["type", "request"], label: "media update")
        let updateRequest = try mediaDictionary(request["request"], label: "media update request")
        try exactKeys(updateRequest, ["lease_id", "update"], label: "media update request")
        let leaseID = try mediaIdentifier(updateRequest["lease_id"], label: "media lease ID")
        let update = try mediaDictionary(updateRequest["update"], label: "media update body")
        guard let updateType = update["type"] as? String else { throw HelperError.invalidRequest("media update has no type") }
        let parsedUpdate: HelperMediaUpdate
        switch updateType {
        case "set_enabled":
            try exactKeys(update, ["type", "kind", "enabled"], label: "media enable update")
            guard update["kind"] as? String == "audio", let enabled = update["enabled"] as? Bool else {
                throw HelperError.unsupported("the Desktop native media helper supports audio updates only")
            }
            parsedUpdate = .setAudioEnabled(enabled)
        case "select_source":
            try exactKeys(update, ["type", "kind", "source"], label: "media source update")
            guard update["kind"] as? String == "audio" else {
                throw HelperError.unsupported("the Desktop native media helper supports audio source updates only")
            }
            let source = try mediaDictionary(update["source"], label: "media audio source")
            try exactKeys(source, ["id"], label: "media audio source")
            parsedUpdate = .selectAudioSource(try mediaIdentifier(source["id"], label: "media audio source reference"))
        case "set_playback_gain":
            try exactKeys(update, ["type", "gain"], label: "media playback gain update")
            guard let number = update["gain"] as? NSNumber,
                  number.doubleValue.isFinite, (0...1).contains(number.doubleValue) else {
                throw HelperError.invalidRequest("media playback gain must be between zero and one")
            }
            parsedUpdate = .setPlaybackGain(number.floatValue)
        default:
            throw HelperError.invalidRequest("unsupported media update type")
        }
        return HelperMediaDeviceRequest(requestID: requestID, operation: .update(leaseID, parsedUpdate))
    default:
        throw HelperError.invalidRequest("unsupported media device request")
    }
}

struct HelperMediaErrorDto: Encodable, Sendable {
    let kind: String
    let message: String
}

struct HelperMediaSourceRefDto: Encodable, Sendable {
    let id: String
}

struct HelperMediaPcmFrameDto: Encodable, Sendable {
    let sampleRateHz: UInt32
    let channels: UInt16
    let pcm16le: [UInt8]

    enum CodingKeys: String, CodingKey {
        case sampleRateHz = "sample_rate_hz"
        case channels
        case pcm16le
    }
}

struct HelperMediaFramePayloadDto: Encodable, Sendable {
    let type = "audio"
    let frame: HelperMediaPcmFrameDto
}

struct HelperMediaFrameDto: Encodable, Sendable {
    let source: HelperMediaSourceRefDto
    let sequence: UInt64
    let capturedAtUnixMs: UInt64
    let payload: HelperMediaFramePayloadDto

    enum CodingKeys: String, CodingKey {
        case source
        case sequence
        case capturedAtUnixMs = "captured_at_unix_ms"
        case payload
    }
}

struct HelperMediaLeaseDto: Encodable, Sendable {
    let leaseID: String
    enum CodingKeys: String, CodingKey { case leaseID = "lease_id" }
}

struct HelperMediaStatusDto: Encodable, Sendable {
    let state: String
    let audioEnabled: Bool
    let videoEnabled = false
    enum CodingKeys: String, CodingKey {
        case state
        case audioEnabled = "audio_enabled"
        case videoEnabled = "video_enabled"
    }
}

struct HelperMediaRenderReceiptDto: Encodable, Sendable {
    let acceptedSampleFrames: UInt32
    let playedSampleFrames: UInt32
    let renderedAtUnixMs: UInt64?
    enum CodingKeys: String, CodingKey {
        case acceptedSampleFrames = "accepted_sample_frames"
        case playedSampleFrames = "played_sample_frames"
        case renderedAtUnixMs = "rendered_at_unix_ms"
    }
}

enum HelperMediaDeviceResultDto: Encodable, Sendable {
    case opened(String)
    case input(HelperMediaFrameDto?)
    case audioWritten(HelperMediaRenderReceiptDto)
    case audioFlushed(HelperMediaRenderReceiptDto)
    case audioInterrupted(HelperMediaRenderReceiptDto)
    case videoWritten
    case updated(HelperMediaStatusDto)
    case stopped
    case error(HelperMediaErrorDto)

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .opened(leaseID):
            try container.encode("opened", forKey: .type)
            try container.encode(HelperMediaLeaseDto(leaseID: leaseID), forKey: .lease)
        case let .input(frame):
            try container.encode("input", forKey: .type)
            try container.encode(frame, forKey: .frame)
        case let .audioWritten(receipt):
            try container.encode("audio_written", forKey: .type)
            try container.encode(receipt, forKey: .receipt)
        case let .audioFlushed(receipt):
            try container.encode("audio_flushed", forKey: .type)
            try container.encode(receipt, forKey: .receipt)
        case let .audioInterrupted(receipt):
            try container.encode("audio_interrupted", forKey: .type)
            try container.encode(receipt, forKey: .receipt)
        case .videoWritten:
            try container.encode("video_written", forKey: .type)
        case let .updated(status):
            try container.encode("updated", forKey: .type)
            try container.encode(status, forKey: .status)
        case .stopped:
            try container.encode("stopped", forKey: .type)
        case let .error(error):
            try container.encode("error", forKey: .type)
            try container.encode(error, forKey: .error)
        }
    }

    private enum CodingKeys: String, CodingKey {
        case type, lease, frame, receipt, status, error
    }
}

struct HelperMediaDeviceResponseDto: Encodable, Sendable {
    let requestID: String
    let result: HelperMediaDeviceResultDto
    enum CodingKeys: String, CodingKey { case requestID = "request_id", result }
}

struct HelperMediaOutputEnvelope: Encodable, Sendable {
    let id: String
    let type = "media_response"
    let response: HelperMediaDeviceResponseDto
}

func helperMediaErrorDto(_ error: Error) -> HelperMediaErrorDto {
    let kind: String
    let message: String
    if let helperError = error as? HelperError {
        message = helperError.message
        switch helperError {
        case .permission: kind = "permission_denied"
        case .busy: kind = "busy"
        case .cancelled: kind = "cancelled"
        case .timeout: kind = "timeout"
        case .noSpeech, .notRecording: kind = "not_found"
        case .unavailable: kind = "unavailable"
        case .unsupported: kind = "unsupported"
        case .modelMissing, .voiceMissing: kind = "model_missing"
        case .mediaTooLarge: kind = "media_too_large"
        case .invalidRequest: kind = "invalid_request"
        case .synthesisFailed: kind = "processing_failed"
        case .download, .checksum, .native: kind = "native_failure"
        }
    } else if error is CancellationError {
        kind = "cancelled"
        message = "the media request was cancelled"
    } else {
        kind = "native_failure"
        message = error.localizedDescription
    }
    var bounded = message
    while bounded.utf8.count > 1_024 { bounded.removeLast() }
    return HelperMediaErrorDto(kind: kind, message: bounded.isEmpty ? "native media request failed" : bounded)
}

private enum HelperMediaInputPop {
    case frame(HelperMediaFrameDto)
    case empty
    case ended
    case failed(HelperError)
}

/// Lock-protected, bounded queue shared with AVAudioEngine's real-time tap.
/// The lock protects only a 32-entry value queue; it is never held across await.
private final class HelperMediaInputQueue: @unchecked Sendable {
    private let lock = NSLock()
    private let sourceID: String
    private let maximumFrameBytes: Int
    private var frames: [HelperMediaFrameDto] = []
    private var sequence: UInt64 = 0
    private var enabled = true
    private var ended = false
    private var failure: HelperError?

    init(sourceID: String, maximumPayloadBytes: Int) {
        self.sourceID = sourceID
        self.maximumFrameBytes = min(maximumPayloadBytes, helperMediaMaximumCaptureFrameBytes)
    }

    func append(pcm: Data, sampleRate: Int, capturedAtUnixMs: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        guard enabled, !ended, failure == nil else { return }
        guard !pcm.isEmpty, pcm.count.isMultiple(of: 2), pcm.count <= maximumFrameBytes else {
            failure = .mediaTooLarge("captured media frame exceeds its byte limit")
            frames.removeAll(keepingCapacity: true)
            return
        }
        guard frames.count < helperMediaCaptureQueueLimit else {
            failure = .mediaTooLarge("the native media input queue overflowed")
            frames.removeAll(keepingCapacity: true)
            return
        }
        sequence &+= 1
        frames.append(HelperMediaFrameDto(
            source: HelperMediaSourceRefDto(id: sourceID),
            sequence: sequence,
            capturedAtUnixMs: capturedAtUnixMs,
            payload: HelperMediaFramePayloadDto(frame: HelperMediaPcmFrameDto(
                sampleRateHz: UInt32(sampleRate),
                channels: 1,
                pcm16le: Array(pcm)
            ))
        ))
    }

    func fail(_ error: HelperError) {
        lock.lock()
        defer { lock.unlock() }
        guard !ended else { return }
        failure = error
        frames.removeAll(keepingCapacity: true)
    }

    func setEnabled(_ value: Bool) {
        lock.lock()
        defer { lock.unlock() }
        guard !ended else { return }
        enabled = value
        if !value { frames.removeAll(keepingCapacity: true) }
    }

    func pop() -> HelperMediaInputPop {
        lock.lock()
        defer { lock.unlock() }
        if let failure { return .failed(failure) }
        if !frames.isEmpty { return .frame(frames.removeFirst()) }
        return ended ? .ended : .empty
    }

    func finish() {
        lock.lock()
        defer { lock.unlock() }
        ended = true
        frames.removeAll(keepingCapacity: false)
    }
}

struct HelperMediaPlaybackReceiptLedger {
    private(set) var playedFramesByGeneration: [UInt64: UInt64] = [:]
    private(set) var renderedAtByGeneration: [UInt64: UInt64] = [:]
    private(set) var lastClaimedGeneration: UInt64?

    mutating func recordPlayed(_ frames: UInt64, generation: UInt64, renderedAtUnixMs: UInt64?) {
        let current = playedFramesByGeneration[generation, default: 0]
        playedFramesByGeneration[generation] = current.addingReportingOverflow(frames).overflow
            ? UInt64.max
            : current + frames
        if let renderedAtUnixMs {
            renderedAtByGeneration[generation] = renderedAtUnixMs
        }
    }

    mutating func take(generation: UInt64) -> (playedFrames: UInt64, renderedAtUnixMs: UInt64?)? {
        if let lastClaimedGeneration, generation <= lastClaimedGeneration { return nil }
        lastClaimedGeneration = generation
        return (
            playedFramesByGeneration.removeValue(forKey: generation) ?? 0,
            renderedAtByGeneration.removeValue(forKey: generation)
        )
    }
}

@MainActor
final class HelperMediaDeviceSession {
    nonisolated static let supportedMediaSessionModes: [String] = ["audio"]

    let owner: HelperAudioOwner
    let identity: HelperAudioOperationIdentity
    let sourceID: String

    private let maximumPayloadBytes: Int
    private let inputQueue: HelperMediaInputQueue
    private let engine: AVAudioEngine
    private let player: AVAudioPlayerNode
    private let outputFormat: AVAudioFormat
    private let inputFormat: AVAudioFormat
    private let captureEpochOffset: Double
    private var inputTapInstalled = false
    private var isOpen = false
    private var audioEnabled = true
    private var playbackGain: Float = 1
    private var playbackGeneration: UInt64 = 1
    private var nextChunkID: UInt64 = 1
    private var pendingChunks: [PlaybackChunk] = []
    private var totalAcceptedInputFrames: UInt64 = 0
    private var totalPlayedInputFrames: UInt64 = 0
    private var receiptLedger = HelperMediaPlaybackReceiptLedger()
    private var activeFlushGeneration: UInt64?
    private var totalOutputFramesScheduled: UInt64 = 0
    private var playerSampleTimeBase: Int64?
    private var queuedOutputFrames = 0

    private struct PlaybackChunk {
        let id: UInt64
        let generation: UInt64
        let inputStart: UInt64
        let inputFrames: UInt64
        let outputStart: UInt64
        let outputFrames: UInt64
    }

    init(request: HelperMediaOpenRequest) throws {
        guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
            throw HelperError.permission("microphone access is required")
        }
        let engine = AVAudioEngine()
        let inputFormat = engine.inputNode.outputFormat(forBus: 0)
        let deviceOutputFormat = engine.outputNode.outputFormat(forBus: 0)
        guard inputFormat.sampleRate > 0, inputFormat.channelCount > 0,
              deviceOutputFormat.sampleRate >= 8_000, deviceOutputFormat.sampleRate <= 768_000,
              deviceOutputFormat.channelCount > 0 else {
            throw HelperError.unavailable("native audio input or output is unavailable")
        }
        let outputChannels = AVAudioChannelCount(min(2, Int(deviceOutputFormat.channelCount)))
        guard let outputFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: deviceOutputFormat.sampleRate,
            channels: outputChannels,
            interleaved: false
        ) else {
            throw HelperError.unavailable("native audio output format is unavailable")
        }
        let player = AVAudioPlayerNode()
        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: outputFormat)
        self.owner = request.owner
        self.identity = request.identity
        self.sourceID = request.sourceID
        self.maximumPayloadBytes = request.maxPayloadBytes
        self.inputQueue = HelperMediaInputQueue(sourceID: request.sourceID, maximumPayloadBytes: request.maxPayloadBytes)
        self.engine = engine
        self.player = player
        self.outputFormat = outputFormat
        self.inputFormat = inputFormat
        self.captureEpochOffset = Date().timeIntervalSince1970 - AVAudioTime.seconds(forHostTime: AudioGetCurrentHostTime())
    }

    func start() throws {
        guard !isOpen else { throw HelperError.busy("the native media device is already open") }
        try installInputTap()
        engine.prepare()
        do {
            try engine.start()
            player.play()
            isOpen = true
        } catch {
            engine.inputNode.removeTap(onBus: 0)
            inputTapInstalled = false
            engine.stop()
            engine.detach(player)
            throw HelperError.native("native duplex audio could not start: \(error.localizedDescription)")
        }
    }

    func nextInput() async throws -> HelperMediaFrameDto? {
        while true {
            try Task.checkCancellation()
            switch inputQueue.pop() {
            case let .frame(frame): return frame
            case .ended: return nil
            case let .failed(error): throw error
            case .empty: try await Task.sleep(nanoseconds: 8_000_000)
            }
        }
    }

    func writeAudio(_ input: HelperMediaPcmInput) throws -> HelperMediaRenderReceiptDto {
        try Task.checkCancellation()
        guard isOpen else { throw HelperError.cancelled("the native media lease has ended") }
        guard audioEnabled else { throw HelperError.unsupported("audio is disabled for this native media lease") }
        guard activeFlushGeneration != playbackGeneration else {
            throw HelperError.busy("native audio playback is settling a flush barrier")
        }
        guard input.bytes.count <= maximumPayloadBytes else {
            throw HelperError.mediaTooLarge("rendered native audio frame exceeds the requested payload limit")
        }
        let inputFrames = input.bytes.count / (input.channels * 2)
        guard inputFrames > 0 else { throw HelperError.invalidRequest("PCM audio frame is empty") }
        let outputFrames = max(1, Int((Double(inputFrames) * outputFormat.sampleRate / Double(input.sampleRateHz)).rounded()))
        let maximumQueuedFrames = Int(outputFormat.sampleRate) * helperMediaPlaybackQueueSeconds
        guard outputFrames <= maximumQueuedFrames,
              queuedOutputFrames <= maximumQueuedFrames - outputFrames,
              pendingChunks.count < helperMediaPlaybackChunkLimit else {
            throw HelperError.busy("the native audio playback queue is full")
        }
        guard outputFrames <= Int(UInt32.max) else {
            throw HelperError.mediaTooLarge("the converted native audio frame is too large")
        }
        let channels = try convertPCM(input, outputFrames: outputFrames)
        try Task.checkCancellation()
        guard let buffer = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: AVAudioFrameCount(outputFrames)),
              let targetChannels = buffer.floatChannelData else {
            throw HelperError.native("native audio buffer allocation failed")
        }
        buffer.frameLength = AVAudioFrameCount(outputFrames)
        for channel in 0..<Int(outputFormat.channelCount) {
            channels[channel].withUnsafeBufferPointer { source in
                guard let baseAddress = source.baseAddress else { return }
                targetChannels[channel].update(from: baseAddress, count: outputFrames)
            }
        }

        if playerSampleTimeBase == nil {
            playerSampleTimeBase = currentPlayerSampleTime()
        }
        let chunk = PlaybackChunk(
            id: nextChunkID,
            generation: playbackGeneration,
            inputStart: totalAcceptedInputFrames,
            inputFrames: UInt64(inputFrames),
            outputStart: totalOutputFramesScheduled,
            outputFrames: UInt64(outputFrames)
        )
        nextChunkID &+= 1
        totalAcceptedInputFrames &+= UInt64(inputFrames)
        totalOutputFramesScheduled &+= UInt64(outputFrames)
        queuedOutputFrames += outputFrames
        pendingChunks.append(chunk)
        engine.mainMixerNode.outputVolume = playbackGain
        let generation = playbackGeneration
        player.scheduleBuffer(buffer, completionCallbackType: .dataPlayedBack) { [weak self] _ in
            // AVAudioPlayerNode calls from its render callback; serialize lease
            // counters on MainActor before any callback result is observed.
            Task { @MainActor [weak self] in
                self?.confirmPlayed(chunkID: chunk.id, generation: generation)
            }
        }
        let renderedFrames = estimatedPlayedInputFrames()
        let played = min(UInt64(inputFrames), renderedFrames > chunk.inputStart ? renderedFrames - chunk.inputStart : 0)
        return HelperMediaRenderReceiptDto(
            acceptedSampleFrames: UInt32(inputFrames),
            playedSampleFrames: UInt32(played),
            renderedAtUnixMs: played > 0 ? currentUnixMilliseconds() : nil
        )
    }

    func flushAudio() async throws -> HelperMediaRenderReceiptDto {
        guard isOpen else { throw HelperError.cancelled("the native media lease has ended") }
        guard activeFlushGeneration == nil else {
            throw HelperError.busy("native audio already has a flush barrier")
        }
        let generation = playbackGeneration
        let targetChunkID = nextChunkID - 1
        activeFlushGeneration = generation
        defer {
            if activeFlushGeneration == generation { activeFlushGeneration = nil }
        }
        while pendingChunks.contains(where: { $0.generation == generation && $0.id <= targetChunkID }) {
            try Task.checkCancellation()
            guard isOpen else { throw HelperError.cancelled("native audio playback was interrupted") }
            if playbackGeneration != generation { break }
            try await Task.sleep(nanoseconds: 8_000_000)
        }
        let receipt = takeBarrierReceipt(generation: generation)
        if playbackGeneration == generation { playbackGeneration &+= 1 }
        return HelperMediaRenderReceiptDto(
            acceptedSampleFrames: 0,
            playedSampleFrames: UInt32(min(UInt64(UInt32.max), receipt.playedFrames)),
            renderedAtUnixMs: receipt.playedFrames > 0 ? receipt.renderedAtUnixMs : nil
        )
    }

    func interruptAudio() throws -> HelperMediaRenderReceiptDto {
        let generation = playbackGeneration
        let estimated = max(totalPlayedInputFrames, estimatedPlayedInputFrames())
        let partialFrames = estimated - totalPlayedInputFrames
        totalPlayedInputFrames = estimated
        receiptLedger.recordPlayed(
            partialFrames,
            generation: generation,
            renderedAtUnixMs: partialFrames > 0 ? currentUnixMilliseconds() : nil
        )
        cutPlayback()
        let receipt = takeBarrierReceipt(generation: generation)
        return HelperMediaRenderReceiptDto(
            acceptedSampleFrames: 0,
            playedSampleFrames: UInt32(min(UInt64(UInt32.max), receipt.playedFrames)),
            renderedAtUnixMs: receipt.playedFrames > 0 ? receipt.renderedAtUnixMs : nil
        )
    }

    func update(_ update: HelperMediaUpdate) async throws -> HelperMediaStatusDto {
        guard isOpen else { throw HelperError.notRecording("the native media lease was not found") }
        switch update {
        case let .setAudioEnabled(enabled):
            if audioEnabled != enabled {
                audioEnabled = enabled
                inputQueue.setEnabled(enabled)
                if enabled {
                    try installInputTap()
                } else {
                    if inputTapInstalled {
                        engine.inputNode.removeTap(onBus: 0)
                        inputTapInstalled = false
                    }
                    _ = try interruptAudio()
                }
            }
        case let .selectAudioSource(source):
            guard source == sourceID else {
                throw HelperError.notRecording("the selected microphone source is not available")
            }
        case let .setPlaybackGain(gain):
            guard gain.isFinite, (0...1).contains(gain) else {
                throw HelperError.invalidRequest("native audio playback gain must be between zero and one")
            }
            playbackGain = gain
            engine.mainMixerNode.outputVolume = gain
        }
        return status()
    }

    func status() -> HelperMediaStatusDto {
        HelperMediaStatusDto(state: isOpen ? "ready" : "ended", audioEnabled: isOpen && audioEnabled)
    }

    func hasPendingPlayback() -> Bool { !pendingChunks.isEmpty }

    func stop() async {
        guard isOpen else { return }
        isOpen = false
        audioEnabled = false
        inputQueue.finish()
        if inputTapInstalled {
            engine.inputNode.removeTap(onBus: 0)
            inputTapInstalled = false
        }
        let generation = playbackGeneration
        let estimated = max(totalPlayedInputFrames, estimatedPlayedInputFrames())
        let partialFrames = estimated - totalPlayedInputFrames
        totalPlayedInputFrames = estimated
        receiptLedger.recordPlayed(partialFrames, generation: generation, renderedAtUnixMs: nil)
        _ = takeBarrierReceipt(generation: generation)
        cutPlayback()
        engine.stop()
        engine.detach(player)
    }

    private func installInputTap() throws {
        guard !inputTapInstalled else { return }
        let queue = inputQueue
        let epochOffset = captureEpochOffset
        let frameByteLimit = min(maximumPayloadBytes, helperMediaMaximumCaptureFrameBytes)
        let format = inputFormat
        engine.inputNode.installTap(onBus: 0, bufferSize: 1_024, format: format) { buffer, time in
            guard let inputChannels = buffer.floatChannelData,
                  buffer.frameLength > 0,
                  buffer.format.sampleRate > 0 else {
                queue.fail(.unavailable("native microphone returned an unsupported PCM format"))
                return
            }
            let frameCount = Int(buffer.frameLength)
            let channels = Int(buffer.format.channelCount)
            let sourceRate = buffer.format.sampleRate
            guard channels > 0, channels <= 8 else {
                queue.fail(.unsupported("native microphone channel count is unsupported"))
                return
            }
            var mono = [Float](repeating: 0, count: frameCount)
            for frameIndex in 0..<frameCount {
                var value: Float = 0
                for channel in 0..<channels { value += inputChannels[channel][frameIndex] }
                mono[frameIndex] = value / Float(channels)
            }
            let outputCount = max(1, Int((Double(frameCount) * Double(helperMediaCaptureSampleRate) / sourceRate).rounded()))
            let converted = Self.resample(mono, sourceRate: sourceRate, targetRate: Double(helperMediaCaptureSampleRate), outputCount: outputCount)
            let pcm = pcm16Data(from: converted)
            guard pcm.count <= frameByteLimit else {
                queue.fail(.mediaTooLarge("captured native audio frame exceeds the requested payload limit"))
                return
            }
            let hostSeconds = time.isHostTimeValid ? AVAudioTime.seconds(forHostTime: time.hostTime) : 0
            let unixSeconds = time.isHostTimeValid ? epochOffset + hostSeconds : Date().timeIntervalSince1970
            let timestamp = UInt64(max(1, min(Double(UInt64.max), (unixSeconds * 1_000).rounded())))
            queue.append(pcm: pcm, sampleRate: helperMediaCaptureSampleRate, capturedAtUnixMs: timestamp)
        }
        inputTapInstalled = true
    }

    private func convertPCM(_ input: HelperMediaPcmInput, outputFrames: Int) throws -> [[Float]] {
        let inputFrames = input.bytes.count / (input.channels * 2)
        let outputChannelCount = Int(outputFormat.channelCount)
        var decoded = [[Float]](repeating: [Float](repeating: 0, count: inputFrames), count: input.channels)
        let bytes = Array(input.bytes)
        for frame in 0..<inputFrames {
            for channel in 0..<input.channels {
                let offset = (frame * input.channels + channel) * 2
                let sample = UInt16(bytes[offset]) | (UInt16(bytes[offset + 1]) << 8)
                decoded[channel][frame] = Float(Int16(bitPattern: sample)) / 32_768
            }
        }
        var mapped = [[Float]](repeating: [Float](repeating: 0, count: inputFrames), count: outputChannelCount)
        if outputChannelCount == 1 {
            for frame in 0..<inputFrames {
                mapped[0][frame] = decoded.reduce(0) { $0 + $1[frame] } / Float(input.channels)
            }
        } else if input.channels == 1 {
            mapped[0] = decoded[0]
            mapped[1] = decoded[0]
        } else {
            let left = Array(stride(from: 0, to: input.channels, by: 2))
            let right = Array(stride(from: 1, to: input.channels, by: 2))
            for frame in 0..<inputFrames {
                mapped[0][frame] = left.map { decoded[$0][frame] }.reduce(0, +) / Float(max(1, left.count))
                mapped[1][frame] = right.map { decoded[$0][frame] }.reduce(0, +) / Float(max(1, right.count))
            }
        }
        return mapped.map { Self.resample($0, sourceRate: Double(input.sampleRateHz), targetRate: outputFormat.sampleRate, outputCount: outputFrames) }
    }

    private func confirmPlayed(chunkID: UInt64, generation: UInt64) {
        guard generation == playbackGeneration,
              let index = pendingChunks.firstIndex(where: { $0.id == chunkID && $0.generation == generation }) else { return }
        let chunk = pendingChunks.remove(at: index)
        totalPlayedInputFrames = max(totalPlayedInputFrames, chunk.inputStart + chunk.inputFrames)
        receiptLedger.recordPlayed(
            chunk.inputFrames,
            generation: generation,
            renderedAtUnixMs: currentUnixMilliseconds()
        )
        queuedOutputFrames = max(0, queuedOutputFrames - Int(chunk.outputFrames))
    }

    private func currentPlayerSampleTime() -> Int64? {
        guard let renderTime = player.lastRenderTime,
              let playerTime = player.playerTime(forNodeTime: renderTime),
              playerTime.isSampleTimeValid else { return nil }
        return playerTime.sampleTime
    }

    private func estimatedPlayedInputFrames() -> UInt64 {
        guard let sampleTime = currentPlayerSampleTime() else { return totalPlayedInputFrames }
        guard let playerSampleTimeBase else { return totalPlayedInputFrames }
        let relative = max(Int64(0), sampleTime - playerSampleTimeBase)
        let head = min(totalOutputFramesScheduled, UInt64(relative))
        var estimate = totalPlayedInputFrames
        for chunk in pendingChunks {
            guard head > chunk.outputStart else { break }
            let rendered = min(chunk.outputFrames, head - chunk.outputStart)
            let inputProgress = chunk.outputFrames == 0 ? 0 : chunk.inputFrames * rendered / chunk.outputFrames
            estimate = max(estimate, chunk.inputStart + inputProgress)
            if rendered < chunk.outputFrames { break }
        }
        return min(totalAcceptedInputFrames, estimate)
    }

    private func takeBarrierReceipt(generation: UInt64) -> (playedFrames: UInt64, renderedAtUnixMs: UInt64?) {
        receiptLedger.take(generation: generation) ?? (0, nil)
    }

    private func cutPlayback() {
        playbackGeneration &+= 1
        player.stop()
        player.reset()
        pendingChunks.removeAll(keepingCapacity: false)
        queuedOutputFrames = 0
        totalOutputFramesScheduled = 0
        playerSampleTimeBase = nil
        if isOpen { player.play() }
    }

    private func currentUnixMilliseconds() -> UInt64 {
        UInt64(max(1, min(Double(UInt64.max), (Date().timeIntervalSince1970 * 1_000).rounded())))
    }

    private static func resample(_ input: [Float], sourceRate: Double, targetRate: Double, outputCount: Int) -> [Float] {
        guard !input.isEmpty, sourceRate > 0, targetRate > 0, sourceRate != targetRate else { return input }
        let ratio = targetRate / sourceRate
        return (0..<outputCount).map { index in
            let sourcePosition = Double(index) / ratio
            let lowerIndex = min(input.count - 1, Int(sourcePosition.rounded(.down)))
            let upperIndex = min(input.count - 1, lowerIndex + 1)
            let fraction = Float(sourcePosition - Double(lowerIndex))
            return input[lowerIndex] + ((input[upperIndex] - input[lowerIndex]) * fraction)
        }
    }
}
