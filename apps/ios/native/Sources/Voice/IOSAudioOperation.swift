import Foundation

/// The attachment identity carried by the bridge DTO. This only validates its
/// shape; it is not proof that the issuing engine attached this operation.
struct IOSMediaAttachmentIdentity: Hashable, Sendable {
    let hostID: String
    let generation: UInt64

    init?(hostID: String, generation: UInt64) {
        guard generation > 0, Self.isCanonicalUUIDv4(hostID) else { return nil }
        self.hostID = hostID
        self.generation = generation
    }

    private static func isCanonicalUUIDv4(_ value: String) -> Bool {
        let bytes = Array(value.utf8)
        guard bytes.count == 36,
              [8, 13, 18, 23].allSatisfy({ bytes[$0] == 45 }),
              bytes[14] == 52,
              [56, 57, 97, 98].contains(bytes[19]) else { return false }
        for index in bytes.indices where ![8, 13, 18, 23].contains(index) {
            let byte = bytes[index]
            guard (48 ... 57).contains(byte) || (97 ... 102).contains(byte) else { return false }
        }
        return true
    }
}

enum IOSMediaLogicalOwner: Hashable, Sendable {
    case session(sessionID: String)
    case ui(instanceID: String)
    case system(instanceID: String)

    fileprivate var stableKey: String {
        switch self {
        case let .session(sessionID): "s\(Self.field(sessionID))"
        case let .ui(instanceID): "u\(Self.field(instanceID))"
        case let .system(instanceID): "y\(Self.field(instanceID))"
        }
    }

    private static func field(_ value: String) -> String {
        "\(value.utf8.count)#\(value)"
    }
}

enum IOSMediaOwnerMappingError: Error {
    case invalidAttachedOwner
}

/// Converts bridge DTOs into structural local identity. The DTO's attachment
/// fields remain transport data until the engine/session pin is matched later.
enum IOSMediaOwnerMapper {
    static func audio(from owner: AudioOwnerDto) throws -> IOSAudioOwner {
        switch owner {
        case let .session(sessionID): .session(sessionID: sessionID)
        case let .ui(instanceID): .ui(instanceID: instanceID)
        case let .system(instanceID): .system(instanceID: instanceID)
        case let .attached(hostID, generation, kind, ownerID):
            try attached(hostID: hostID, generation: generation, kind: kind, ownerID: ownerID)
        }
    }

    static func media(from owner: MediaOwnerDto) throws -> IOSAudioOwner {
        switch owner {
        case let .session(sessionID): .session(sessionID: sessionID)
        case let .ui(instanceID): .ui(instanceID: instanceID)
        case let .system(instanceID): .system(instanceID: instanceID)
        case let .attached(hostID, generation, kind, ownerID):
            try attached(hostID: hostID, generation: generation, kind: kind, ownerID: ownerID)
        }
    }

    private static func attached(
        hostID: String,
        generation: UInt64,
        kind: MediaOwnerKindDto,
        ownerID: String
    ) throws -> IOSAudioOwner {
        guard let attachment = IOSMediaAttachmentIdentity(hostID: hostID, generation: generation),
              !ownerID.isEmpty, ownerID.utf8.count <= 256 else {
            throw IOSMediaOwnerMappingError.invalidAttachedOwner
        }
        let logicalOwner: IOSMediaLogicalOwner
        switch kind {
        case .session: logicalOwner = .session(sessionID: ownerID)
        case .ui: logicalOwner = .ui(instanceID: ownerID)
        case .system: logicalOwner = .system(instanceID: ownerID)
        }
        return .attached(attachment: attachment, logicalOwner: logicalOwner)
    }
}

struct IOSAudioOperationIdentity: Hashable, Sendable {
    let id: String
    let generation: UInt64
    let serviceEpoch: UInt64

    /// Recording-driver claims must include every part of the operation
    /// incarnation; a reused request id must not address an older recorder.
    var recordingDriverKey: String { "\(serviceEpoch):\(generation):\(id)" }
}

/// Retains the in-flight native playback teardown by operation identity.
/// UI stop is synchronous at its call site, while attachment cancellation is
/// async and must be able to join the exact stop before reporting a drain.
@MainActor
final class IOSAudioPlaybackStopReceipts {
    @MainActor
    private final class CompletionFlag {
        var completed = false
    }

    private struct Flight {
        let completion: CompletionFlag
        let task: Task<Void, Never>
    }

    private var flights: [IOSAudioOperationIdentity: Flight] = [:]

    @discardableResult
    func begin(
        for identity: IOSAudioOperationIdentity,
        stop: @escaping @MainActor @Sendable () async -> Void
    ) -> Task<Void, Never> {
        pruneCompletedFlights()
        if let flight = flights[identity] { return flight.task }

        let completion = CompletionFlag()
        let task = Task { @MainActor in
            await stop()
            completion.completed = true
        }
        flights[identity] = Flight(completion: completion, task: task)
        return task
    }

    func task(for identity: IOSAudioOperationIdentity) -> Task<Void, Never>? {
        pruneCompletedFlights()
        return flights[identity]?.task
    }

    func contains(_ identity: IOSAudioOperationIdentity) -> Bool {
        pruneCompletedFlights()
        return flights[identity] != nil
    }

    private func pruneCompletedFlights() {
        let completed = flights.compactMap { identity, flight in
            flight.completion.completed ? identity : nil
        }
        for identity in completed { flights.removeValue(forKey: identity) }
    }
}

enum IOSAudioOwner: Hashable, Sendable {
    case session(sessionID: String)
    case ui(instanceID: String)
    case system(instanceID: String)
    case attached(attachment: IOSMediaAttachmentIdentity, logicalOwner: IOSMediaLogicalOwner)

    var stableKey: String {
        switch self {
        case let .session(sessionID): "s\(Self.field(sessionID))"
        case let .ui(instanceID): "u\(Self.field(instanceID))"
        case let .system(instanceID): "y\(Self.field(instanceID))"
        case let .attached(attachment, logicalOwner):
            "a\(Self.field(attachment.hostID))#\(attachment.generation)#\(logicalOwner.stableKey)"
        }
    }

    private static func field(_ value: String) -> String {
        "\(value.utf8.count)#\(value)"
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
    let routePolicy: IOSAudioOperationRoutePolicy?

    init(
        identity: IOSAudioOperationIdentity,
        owner: IOSAudioOwner,
        initiator: IOSAudioInitiator?,
        timeoutBudgetMs: UInt64?,
        maxPayloadBytes: UInt64,
        operation: IOSAudioOperation,
        routePolicy: IOSAudioOperationRoutePolicy? = nil
    ) {
        self.identity = identity
        self.owner = owner
        self.initiator = initiator
        self.timeoutBudgetMs = timeoutBudgetMs
        self.maxPayloadBytes = maxPayloadBytes
        self.operation = operation
        self.routePolicy = routePolicy
    }
}

enum IOSAudioOperationRoutePolicy: Equatable, Sendable {
    case hostConfiguredLocalOnly
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
