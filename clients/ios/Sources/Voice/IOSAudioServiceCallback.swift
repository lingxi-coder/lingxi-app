import Foundation

/// UniFFI may ask for capabilities from a Rust worker thread. Keep the callback
/// synchronous and serve an immutable value from this locked cache; audio work
/// itself always hops to the app-scoped MainActor service.
final class IOSAudioServiceCallbackAdapter: IosAudioService, @unchecked Sendable {
    @MainActor
    static let shared = IOSAudioServiceCallbackAdapter(service: .shared)

    private let service: IOSAudioService
    private let capabilityCache: IOSAudioCapabilitySnapshotCache

    @MainActor
    init(service: IOSAudioService) {
        self.service = service
        capabilityCache = IOSAudioCapabilitySnapshotCache(
            IOSAudioServiceCallbackAdapter.capabilityDto(
                state: service.capabilityState(),
                maximumPayloadBytes: service.maximumPayloadBytes ?? maxAudioPayloadBytes()
            )
        )
        service.installCapabilityCache(capabilityCache)
    }

    func capabilities() -> AudioCapabilitySnapshotDto {
        capabilityCache.read()
    }

    func execute(request: AudioOperationRequestDto) async -> AudioOperationResultDto {
        let result = await service.execute(Self.localRequest(request))
        await service.publishCapabilitySnapshot()
        return Self.dtoResult(result)
    }

    func cancel(identity: AudioOperationIdDto) async throws {
        await service.cancel(Self.localIdentity(identity))
        await service.publishCapabilitySnapshot()
    }

    private static func localRequest(_ request: AudioOperationRequestDto) -> IOSAudioOperationRequest {
        IOSAudioOperationRequest(
            identity: localIdentity(request.identity),
            owner: localOwner(request.owner),
            initiator: request.initiator.map {
                IOSAudioInitiator(agentID: $0.agentId, toolUseID: $0.toolUseId, requestID: $0.requestId)
            },
            timeoutBudgetMs: request.timeoutBudgetMs,
            maxPayloadBytes: request.maxPayloadBytes,
            operation: localOperation(request.operation)
        )
    }

    private static func localIdentity(_ identity: AudioOperationIdDto) -> IOSAudioOperationIdentity {
        IOSAudioOperationIdentity(
            id: identity.id,
            generation: identity.generation,
            serviceEpoch: identity.serviceEpoch
        )
    }

    private static func localOwner(_ owner: AudioOwnerDto) -> IOSAudioOwner {
        switch owner {
        case let .session(sessionId): .session(sessionID: sessionId)
        case let .localApp(appId, runtimeGeneration): .localApp(appID: appId, runtimeGeneration: runtimeGeneration)
        case let .ui(instanceId): .ui(instanceID: instanceId)
        case let .system(instanceId): .system(instanceID: instanceId)
        }
    }

    private static func localOperation(_ operation: AudioOperationDto) -> IOSAudioOperation {
        switch operation {
        case let .startRecording(sampleRateHz, format): .startRecording(sampleRateHz: sampleRateHz, format: format)
        case let .stopRecording(handle): .stopRecording(handle: handle)
        case let .listen(language): .listen(language: language)
        case let .synthesize(text, language, rate, voice): .synthesize(text: text, language: language, rate: rate, voice: voice)
        case let .speak(text, language, rate, voice): .speak(text: text, language: language, rate: rate, voice: voice)
        case let .status(handle): .status(handle: handle)
        case .endOwner: .endOwner
        }
    }

    private static func dtoResult(_ result: IOSAudioOperationResult) -> AudioOperationResultDto {
        switch result {
        case let .recordingStarted(handle): .recordingStarted(handle: handle)
        case let .recording(data, mimeType): .recording(audioBase64: data.base64EncodedString(), mimeType: mimeType)
        case let .transcript(text, language, confidence): .transcript(text: text, language: language, confidence: confidence)
        case let .synthesized(pcm, sampleRateHz): .synthesized(pcmBase64: pcm.base64EncodedString(), sampleRateHz: sampleRateHz)
        case let .playbackCompleted(durationMs): .playbackCompleted(durationMs: durationMs)
        case let .status(recording, playing): .status(status: AudioStatusDto(recording: recording, playing: playing))
        case .ownerEnded: .ownerEnded
        case let .failed(error): .failed(error: dtoError(error))
        }
    }

    private static func dtoError(_ error: IOSAudioError) -> AudioErrorDto {
        let kind: AudioErrorKindDto = switch error.kind {
        case .permissionDenied: .permissionDenied
        case .busy: .busy
        case .cancelled: .cancelled
        case .timeout: .timeout
        case .noSpeech: .noSpeech
        case .notRecording: .notRecording
        case .unavailable: .unavailable
        case .unsupported: .unsupported
        case .modelMissing: .modelMissing
        case .voiceMissing: .voiceMissing
        case .invalidRequest: .invalidRequest
        case .synthesisFailed: .synthesisFailed
        case .nativeFailure: .nativeFailure
        case .mediaTooLarge: .mediaTooLarge
        }
        return AudioErrorDto(kind: kind, message: error.message)
    }

    static func capabilityDto(
        state: IOSAudioCapabilityState,
        maximumPayloadBytes: UInt64
    ) -> AudioCapabilitySnapshotDto {
        let supported = state.supportedOperations.sorted { Self.operationOrder($0) < Self.operationOrder($1) }
            .map(Self.dtoOperation)
        let readiness = state.supportedOperations.sorted { Self.operationOrder($0) < Self.operationOrder($1) }
            .map { operation in
                AudioOperationReadinessDto(
                    operation: Self.dtoOperation(operation),
                    state: Self.dtoReadiness(state.readiness[operation] ?? .unavailable)
                )
            }
        return AudioCapabilitySnapshotDto(
            serviceEpoch: state.serviceEpoch,
            supportRevision: state.supportRevision,
            supportedOperations: supported,
            readiness: readiness,
            maxPayloadBytes: maximumPayloadBytes
        )
    }

    private static func operationOrder(_ operation: IOSAudioCapabilityState.Operation) -> Int {
        switch operation {
        case .record: 0
        case .listen: 1
        case .synthesize: 2
        case .speak: 3
        }
    }

    private static func dtoOperation(_ operation: IOSAudioCapabilityState.Operation) -> AudioOperationKindDto {
        switch operation {
        case .record: .record
        case .listen: .listen
        case .synthesize: .synthesize
        case .speak: .speak
        }
    }

    private static func dtoReadiness(_ readiness: IOSAudioCapabilityState.Readiness) -> AudioReadinessStateDto {
        switch readiness {
        case .ready: .ready
        case .needsPermission: .needsPermission
        case .busy: .busy
        case .missingModel: .missingModel
        case .unavailable: .unavailable
        }
    }
}

final class IOSAudioCapabilitySnapshotCache: @unchecked Sendable {
    private let lock = NSLock()
    private var snapshot: AudioCapabilitySnapshotDto

    init(_ initialSnapshot: AudioCapabilitySnapshotDto) {
        snapshot = initialSnapshot
    }

    func read() -> AudioCapabilitySnapshotDto {
        lock.lock()
        defer { lock.unlock() }
        return snapshot
    }

    func update(_ snapshot: AudioCapabilitySnapshotDto) {
        lock.lock()
        self.snapshot = snapshot
        lock.unlock()
    }
}
