import Foundation

struct IOSAudioOperationIdentity: Hashable, Sendable {
    let id: String
    let generation: UInt64
    let serviceEpoch: UInt64
}

enum IOSAudioOwner: Hashable, Sendable {
    case session(sessionID: String)
    case ui(instanceID: String)
    case system(instanceID: String)

    var stableKey: String {
        switch self {
        case let .session(sessionID): "session:\(sessionID)"
        case let .ui(instanceID): "ui:\(instanceID)"
        case let .system(instanceID): "system:\(instanceID)"
        }
    }
}

struct IOSAudioInitiator: Sendable {
    let agentID: String?
    let toolUseID: String?
    let requestID: String?
}

struct IOSAudioOperationRequest: Sendable {
    let identity: IOSAudioOperationIdentity
    let owner: IOSAudioOwner
    let initiator: IOSAudioInitiator?
    let timeoutBudgetMs: UInt64?
    let maxPayloadBytes: UInt64
    let operation: IOSAudioOperation
}

enum IOSAudioOperation: Sendable {
    case startRecording(sampleRateHz: UInt32, format: String)
    case stopRecording(handle: String)
    case capture(sampleRateHz: UInt32, format: String)
    case play(pcm: Data, sampleRateHz: UInt32)
    case transcribe(audio: Data, mimeType: String, language: String?)
    case listen(language: String?)
    case synthesize(text: String, language: String?, rate: Float?, voice: String?)
    case speak(text: String, language: String?, rate: Float?, voice: String?)
    case status(handle: String?)
    case endOwner
}

enum IOSAudioErrorKind: String, Equatable, Sendable {
    case permissionDenied
    case busy
    case cancelled
    case timeout
    case noSpeech
    case notRecording
    case unavailable
    case unsupported
    case modelMissing
    case voiceMissing
    case invalidRequest
    case synthesisFailed
    case nativeFailure
    case mediaTooLarge
}

struct IOSAudioError: Equatable, Sendable {
    let kind: IOSAudioErrorKind
    let message: String
}

enum IOSAudioOperationResult: Sendable {
    case recordingStarted(handle: String)
    case recording(data: Data, mimeType: String)
    case transcript(text: String, language: String?, confidence: Float?)
    case synthesized(pcm: Data, sampleRateHz: UInt32)
    case playbackCompleted(durationMs: UInt64)
    case status(recording: Bool, playing: Bool)
    case ownerEnded
    case failed(IOSAudioError)
}

struct IOSAudioCapabilityState: Sendable {
    let serviceEpoch: UInt64
    let supportRevision: UInt64
    let supportedOperations: Set<Operation>
    let readiness: [Operation: Readiness]

    enum Operation: Hashable, Sendable { case record, capture, play, transcribe, listen, synthesize, speak }
    enum Readiness: Equatable, Sendable { case ready, needsPermission, busy, missingModel, unavailable }
}

struct IOSAudioOperationDiagnostics: Equatable, Sendable {
    let identity: IOSAudioOperationIdentity
    let ownerKey: String
    let operation: String
    let configurationRevision: UInt64
    var requestedSource: String?
    var effectiveSource: String?
    var fallbackReason: String?
    var phase: String
}

struct IOSAudioServiceDiagnostics: Sendable {
    let serviceEpoch: UInt64
    let configurationRevision: UInt64
    let activeLeasePurpose: String?
    let leaseAwaitingOwnerCleanup: Bool
    let pendingOperations: [IOSAudioOperationDiagnostics]
    let lastOperation: IOSAudioOperationDiagnostics?
    let activeRecordingCount: Int
    let activePlaybackOwner: String?
    let activeListenOwner: String?
    let activeBargeInSessionCount: Int
}

extension IOSAudioOperationRequest {
    var ownerKey: String { owner.stableKey }
    var operationID: IOSAudioOperationIdentity { identity }
}
