import AVFoundation
import Foundation
import Observation
import UIKit

/// One process-wide owner for iOS audio operations. Wire adapters translate
/// generated UniFFI DTOs to these local values and back; this class owns the
/// operation identities, stable owners, cancellation and all native resources.
@MainActor
final class IOSAudioService: VoiceSpeechPlaying, VoiceBargeInRecognizing {
    static let shared = IOSAudioService()

    private struct PendingOperation {
        let request: IOSAudioOperationRequest
        let task: Task<Void, Never>
        var continuation: CheckedContinuation<IOSAudioOperationResult, Never>?
        var isTerminal = false
        var terminalError: IOSAudioError?
        var diagnostics: IOSAudioOperationDiagnostics
    }

    private struct RecordingOwner {
        let owner: IOSAudioOwner
        let startIdentity: IOSAudioOperationIdentity
    }

    private let configurationStore: AudioConfigurationStore
    private let stt: SttImpl
    private let tts: TtsImpl
    private let recorder: any AudioRecordingDriving
    private let speechImplementation: SystemVoiceSpeechPlayer
    private let bargeInImplementation: VoiceBargeInRecognizer
    private let coordinator: VoiceAudioSessionCoordinator
    private let onInvalidation: (@MainActor (VoiceAudioSessionCoordinator.Invalidation) async -> Void)?
    private weak var capabilityCache: IOSAudioCapabilitySnapshotCache?
    private var invalidationTask: Task<Void, Never>?
    private var scheduledCapabilityRefresh: Task<Void, Never>?
    private var capabilityObservers: [NSObjectProtocol] = []
    private var pending: [IOSAudioOperationIdentity: PendingOperation] = [:]
    private var seenIdentities: Set<IOSAudioOperationIdentity> = []
    private var seenIdentityOrder: [IOSAudioOperationIdentity] = []
    private var retiredIdentities: [IOSAudioOperationIdentity: ContinuousClock.Instant] = [:]
    private var retiredIdentityOrder: [IOSAudioOperationIdentity] = []
    private var endingOwners: Set<IOSAudioOwner> = []
    private var ownerEndWaiters: [IOSAudioOwner: [CheckedContinuation<Void, Never>]] = [:]
    private var recordings: [String: RecordingOwner] = [:]
    private var activePlaybackIdentity: IOSAudioOperationIdentity?
    private var activePlaybackOwner: IOSAudioOwner?
    private var activeListenIdentity: IOSAudioOperationIdentity?
    private var activeListenOwner: IOSAudioOwner?
    private var activeSystemRenderIdentity: IOSAudioOperationIdentity?
    private var modelRenderOwners: [String: IOSAudioOperationIdentity] = [:]
    private(set) var lastOperationDiagnostics: IOSAudioOperationDiagnostics?
    private var supportRevision: UInt64 = 1
    private struct CapabilityFingerprint: Equatable {
        let supportedOperations: Set<IOSAudioCapabilityState.Operation>
        let readiness: [IOSAudioCapabilityState.Operation: IOSAudioCapabilityState.Readiness]
        let maximumPayloadBytes: UInt64
    }
    private var lastCapabilityFingerprint: CapabilityFingerprint?
    private(set) var maximumPayloadBytes: UInt64?
    private(set) var serviceEpoch: UInt64

    /// The UI receives the service itself as its speech/player façade rather
    /// than reaching around it to construct another synthesizer.
    var speechPlayer: any VoiceSpeechPlaying { self }
    var bargeInRecognizer: any VoiceBargeInRecognizing { self }

    init(
        configurationStore: AudioConfigurationStore? = nil,
        stt: SttImpl? = nil,
        tts: TtsImpl? = nil,
        recorder: (any AudioRecordingDriving)? = nil,
        speechImplementation: SystemVoiceSpeechPlayer? = nil,
        bargeInImplementation: VoiceBargeInRecognizer? = nil,
        coordinator: VoiceAudioSessionCoordinator? = nil,
        serviceEpoch: UInt64? = nil,
        maximumPayloadBytes: UInt64? = nil,
        onInvalidation: (@MainActor (VoiceAudioSessionCoordinator.Invalidation) async -> Void)? = nil
    ) {
        let coordinator = coordinator ?? VoiceAudioSessionCoordinator.shared
        self.coordinator = coordinator
        self.configurationStore = configurationStore ?? .shared
        self.stt = stt ?? SttImpl(coordinator: coordinator)
        self.tts = tts ?? TtsImpl()
        self.recorder = recorder ?? VoiceImpl(coordinator: coordinator)
        self.speechImplementation = speechImplementation ?? SystemVoiceSpeechPlayer(coordinator: coordinator)
        self.bargeInImplementation = bargeInImplementation ?? VoiceBargeInRecognizer(coordinator: coordinator)
        self.serviceEpoch = serviceEpoch ?? Self.freshServiceEpoch()
        let frameworkPayloadLimit = maxAudioPayloadBytes()
        self.maximumPayloadBytes = min(maximumPayloadBytes ?? frameworkPayloadLimit, frameworkPayloadLimit)
        self.onInvalidation = onInvalidation
        self.recorder.installActivityChangeHandler { [weak self] in
            Task { @MainActor [weak self] in self?.publishCapabilitySnapshot() }
        }
        self.bargeInImplementation.onSessionsChanged = { [weak self] in
            self?.publishCapabilitySnapshot()
        }
        let notificationCenter = NotificationCenter.default
        capabilityObservers.append(notificationCenter.addObserver(
            forName: AudioConfigurationStore.didChangeNotification,
            object: self.configurationStore,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in self?.scheduleCapabilityRefresh() }
        })
        capabilityObservers.append(notificationCenter.addObserver(
            forName: VoiceModelStore.didChangeNotification,
            object: VoiceModelStore.shared,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in self?.scheduleCapabilityRefresh() }
        })
        capabilityObservers.append(notificationCenter.addObserver(
            forName: UIApplication.willEnterForegroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in self?.scheduleCapabilityRefresh() }
        })
        invalidationTask = Task { @MainActor [weak self, coordinator] in
            let events = await coordinator.invalidationEvents()
            for await event in events {
                guard let self else { return }
                await self.handleInvalidation(event)
            }
        }
    }

    deinit {
        invalidationTask?.cancel()
        scheduledCapabilityRefresh?.cancel()
        capabilityObservers.forEach(NotificationCenter.default.removeObserver)
    }

    var hasActivePlayback: Bool {
        activePlaybackIdentity != nil || speechImplementation.hasActivePlayback
    }

    func installMaximumPayloadBytes(_ value: UInt64) {
        guard value > 0, value <= 9_007_199_254_740_991 else { return }
        maximumPayloadBytes = min(value, maxAudioPayloadBytes())
        publishCapabilitySnapshot()
    }

    func installCapabilityCache(_ cache: IOSAudioCapabilitySnapshotCache) {
        capabilityCache = cache
        publishCapabilitySnapshot()
    }

    func publishCapabilitySnapshot() {
        guard let capabilityCache else { return }
        let limit = maximumPayloadBytes ?? maxAudioPayloadBytes()
        let initialState = capabilityState()
        let fingerprint = CapabilityFingerprint(
            supportedOperations: initialState.supportedOperations,
            readiness: initialState.readiness,
            maximumPayloadBytes: limit
        )
        if let lastCapabilityFingerprint,
           lastCapabilityFingerprint != fingerprint,
           supportRevision < 9_007_199_254_740_991 {
            supportRevision += 1
        }
        self.lastCapabilityFingerprint = fingerprint
        capabilityCache.update(IOSAudioServiceCallbackAdapter.capabilityDto(
            state: capabilityState(),
            maximumPayloadBytes: limit
        ))
    }

    private func scheduleCapabilityRefresh() {
        scheduledCapabilityRefresh?.cancel()
        scheduledCapabilityRefresh = Task { @MainActor [weak self] in
            do { try await Task.sleep(for: .milliseconds(25)) } catch { return }
            guard let self else { return }
            self.scheduledCapabilityRefresh = nil
            self.publishCapabilitySnapshot()
        }
    }

    func diagnostics() async -> IOSAudioServiceDiagnostics {
        let leaseState = await coordinator.leaseStateForDiagnostics()
        return IOSAudioServiceDiagnostics(
            serviceEpoch: serviceEpoch,
            configurationRevision: configurationStore.revision,
            activeLeasePurpose: leaseState.purpose.map { String(describing: $0) },
            leaseAwaitingOwnerCleanup: leaseState.awaitingOwnerCleanup,
            pendingOperations: pending.values.map(\.diagnostics),
            lastOperation: lastOperationDiagnostics,
            activeRecordingCount: recordings.filter {
                recorder.isRecordingOwned(handle: $0.key, ownerID: $0.value.owner.stableKey)
            }.count,
            activePlaybackOwner: activePlaybackOwner?.stableKey,
            activeListenOwner: activeListenOwner?.stableKey,
            activeBargeInSessionCount: bargeInImplementation.activeSessionCount
        )
    }

    func capabilityState() -> IOSAudioCapabilityState {
        let all: Set<IOSAudioCapabilityState.Operation> = [.record, .listen, .synthesize, .speak]
        let microphone = AVAudioApplication.shared.recordPermission
        let microphoneReadiness: IOSAudioCapabilityState.Readiness = switch microphone {
        case .granted: .ready
        case .undetermined: .needsPermission
        case .denied: .unavailable
        @unknown default: .unavailable
        }
        let recognitionRoute = AudioConfigurationRuntime.route(
            kind: .recognition,
            snapshot: configurationStore.snapshot
        )
        let speechRoute = AudioConfigurationRuntime.route(
            kind: .speech,
            snapshot: configurationStore.snapshot
        )
        let recognitionReadiness = Self.readiness(for: recognitionRoute)
        let speechReadiness = Self.readiness(for: speechRoute)
        let rawRecordingActive = recordings.contains { handle, recording in
            recorder.isRecordingOwned(handle: handle, ownerID: recording.owner.stableKey)
        } || pending.values.contains {
            if case .startRecording = $0.request.operation { return !$0.isTerminal }
            return false
        }
        let isListeningActive = activeListenIdentity != nil || pending.values.contains {
            if case .listen = $0.request.operation { return !$0.isTerminal }
            return false
        } || bargeInImplementation.activeSessionCount > 0
        let isSynthesizingActive = activeSystemRenderIdentity != nil || !modelRenderOwners.isEmpty || pending.values.contains {
            if case .synthesize = $0.request.operation { return !$0.isTerminal }
            return false
        }
        let isPlaybackActive = activePlaybackIdentity != nil || speechImplementation.hasActivePlayback || pending.values.contains {
            if case .speak = $0.request.operation { return !$0.isTerminal }
            return false
        }
        let usesSystemSpeech = speechRoute.effective?.source == .system
        let isSynthesizingBusy = isSynthesizingActive
            || (usesSystemSpeech && (rawRecordingActive || isListeningActive || isPlaybackActive))
        let isListeningBusy = isListeningActive
            || rawRecordingActive
            || isPlaybackActive
            || (usesSystemSpeech && isSynthesizingActive)
        let isPlaybackBusy = isPlaybackActive
            || rawRecordingActive
            || isListeningActive
            || (usesSystemSpeech && isSynthesizingActive)
        let listenReadiness: IOSAudioCapabilityState.Readiness = switch microphoneReadiness {
        case .unavailable: .unavailable
        case .needsPermission: .needsPermission
        case .busy: .busy
        case .missingModel: .missingModel
        case .ready: recognitionReadiness
        }
        return IOSAudioCapabilityState(
            serviceEpoch: serviceEpoch,
            supportRevision: supportRevision,
            supportedOperations: all,
            readiness: [
                .record: Self.rawRecordingReadiness(
                    microphoneReadiness: microphoneReadiness,
                    recordingOwned: rawRecordingActive,
                    listening: isListeningActive,
                    playback: isPlaybackActive,
                    systemRender: usesSystemSpeech && isSynthesizingActive
                ),
                .listen: isListeningBusy ? .busy : listenReadiness,
                .synthesize: isSynthesizingBusy ? .busy : speechReadiness,
                .speak: isPlaybackBusy ? .busy : speechReadiness,
            ]
        )
    }

    static func rawRecordingReadiness(
        microphoneReadiness: IOSAudioCapabilityState.Readiness,
        recordingOwned: Bool,
        listening: Bool,
        playback: Bool,
        systemRender: Bool
    ) -> IOSAudioCapabilityState.Readiness {
        if recordingOwned || listening || playback || systemRender { return .busy }
        return microphoneReadiness
    }

    static func recognitionSessionPurpose(
        for owner: IOSAudioOwner
    ) -> VoiceAudioSessionCoordinator.Purpose {
        owner == .ui(instanceID: "flow-barge-in") ? .flowDuplex : .recognition
    }

    /// Executes one immutable request snapshot. `cancel` addresses the exact
    /// identity and owner-bound resources survive this method only for a
    /// successful raw recording handle.
    func execute(
        _ request: IOSAudioOperationRequest,
        configurationSnapshot pinnedSnapshot: AudioConfigurationSnapshot? = nil,
        automaticEndpointAfterSilence: Duration? = nil,
        routeResolution pinnedRoute: AudioRouteResolution? = nil
    ) async -> IOSAudioOperationResult {
        if Task.isCancelled {
            if Self.isValidIdentity(request.identity), request.identity.serviceEpoch == serviceEpoch {
                _ = rememberSeenIdentity(request.identity)
                retire(request.identity)
            }
            return .failed(.init(kind: .cancelled, message: "The audio operation was cancelled before admission."))
        }
        guard Self.isValidIdentity(request.identity),
              request.identity.serviceEpoch == serviceEpoch,
              request.maxPayloadBytes > 0,
              request.maxPayloadBytes <= (maximumPayloadBytes ?? maxAudioPayloadBytes())
        else {
            return .failed(.init(kind: .invalidRequest, message: "Invalid audio operation identity or bounds."))
        }
        pruneRetiredIdentities()
        if retiredIdentities[request.identity] != nil {
            _ = rememberSeenIdentity(request.identity)
            clearRetired(request.identity)
            return .failed(.init(kind: .cancelled, message: "The audio operation was cancelled before admission."))
        }
        guard rememberSeenIdentity(request.identity) else {
            return .failed(.init(kind: .invalidRequest, message: "Audio operation identity was already used."))
        }
        if let timeout = request.timeoutBudgetMs, timeout == 0 {
            return .failed(.init(kind: .timeout, message: "The audio operation timed out before it started."))
        }
        guard !endingOwners.contains(request.owner) else {
            return .failed(.init(kind: .busy, message: "The audio owner is being cleaned up."))
        }

        let operationSnapshot = pinnedSnapshot ?? configurationStore.snapshot
        let diagnostic = IOSAudioOperationDiagnostics(
            identity: request.identity,
            ownerKey: request.ownerKey,
            operation: Self.operationName(request.operation),
            configurationRevision: operationSnapshot.revision,
            requestedSource: Self.requestedSource(for: request.operation, snapshot: operationSnapshot)?.rawValue,
            effectiveSource: nil,
            fallbackReason: nil,
            phase: "admitted"
        )
        lastOperationDiagnostics = diagnostic
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            let result = await self.perform(
                request,
                configurationSnapshot: operationSnapshot,
                automaticEndpointAfterSilence: automaticEndpointAfterSilence,
                routeResolution: pinnedRoute
            )
            await self.completeOperation(request.identity, result: result)
        }
        pending[request.identity] = PendingOperation(
            request: request,
            task: task,
            diagnostics: diagnostic
        )
        publishCapabilitySnapshot()

        var timeoutTask: Task<Void, Never>?
        if let timeout = request.timeoutBudgetMs {
            let identity = request.identity
            timeoutTask = Task { @MainActor [weak self] in
                do { try await Task.sleep(for: .milliseconds(Int64(clamping: timeout))) } catch { return }
                await self?.expire(identity)
            }
        }

        let result = await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<IOSAudioOperationResult, Never>) in
                guard var operation = pending[request.identity] else {
                    continuation.resume(returning: .failed(.init(
                        kind: .cancelled,
                        message: "The audio operation was retired."
                    )))
                    return
                }
                operation.continuation = continuation
                pending[request.identity] = operation
            }
        } onCancel: {
            Task { @MainActor [weak self] in await self?.cancel(request.identity) }
        }
        timeoutTask?.cancel()
        pending.removeValue(forKey: request.identity)
        publishCapabilitySnapshot()
        return result
    }

    /// Cancellation is scoped to one UUID/generation/epoch. It retires
    /// admission immediately, stops any matching native resource, and waits
    /// only when an operation owns active native I/O (never for a permission
    /// sheet that has not opened a resource).
    func cancel(
        _ identity: IOSAudioOperationIdentity,
        terminalError: IOSAudioError = .init(kind: .cancelled, message: "The audio operation was cancelled.")
    ) async {
        guard Self.isValidIdentity(identity), identity.serviceEpoch == serviceEpoch else { return }
        retire(identity)
        guard var operation = pending[identity], !operation.isTerminal else {
            if let (handle, _) = recordings.first(where: { $0.value.startIdentity == identity }) {
                await recorder.cancel(startOperationID: identity.id)
                recordings.removeValue(forKey: handle)
            }
            return
        }
        guard operation.terminalError == nil else { return }
        let ownsNativeIO: Bool = switch operation.request.operation {
        case .startRecording:
            recorder.isRecordingOwned(handle: nil, ownerID: operation.request.ownerKey)
        case .listen:
            activeListenIdentity == identity && stt.hasActiveNativeOperation
        case .speak:
            activePlaybackIdentity == identity && speechImplementation.hasActivePlayback
        case .synthesize:
            activeSystemRenderIdentity == identity || modelRenderOwners.values.contains(identity)
        case .stopRecording, .status, .endOwner:
            false
        }
        operation.terminalError = terminalError
        operation.diagnostics.phase = terminalError.kind.rawValue
        pending[identity] = operation
        lastOperationDiagnostics = operation.diagnostics
        operation.task.cancel()

        switch operation.request.operation {
        case .startRecording:
            await recorder.cancel(startOperationID: identity.id)
        case .listen:
            if activeListenIdentity == identity {
                stt.cancelRecognition()
                activeListenIdentity = nil
                activeListenOwner = nil
            }
        case .speak:
            if activePlaybackIdentity == identity {
                await speechImplementation.stopAndWait()
                clearPlaybackIfCurrent(identity)
            }
        case .stopRecording, .synthesize, .status, .endOwner:
            break
        }

        if ownsNativeIO { _ = await operation.task.result }
        settle(identity, result: .failed(terminalError))
    }

    private func completeOperation(_ identity: IOSAudioOperationIdentity, result: IOSAudioOperationResult) async {
        guard let operation = pending[identity] else {
            if retiredIdentities[identity] != nil {
                await cleanupLateResult(result, identity: identity)
                clearRetired(identity)
            }
            return
        }
        if operation.terminalError != nil {
            if lastOperationDiagnostics?.identity == identity {
                lastOperationDiagnostics = operation.diagnostics
            }
            await cleanupLateResult(result, identity: identity)
            clearRetired(identity)
            return
        }
        guard !operation.isTerminal else {
            await cleanupLateResult(result, identity: identity)
            clearRetired(identity)
            return
        }
        var completed = operation
        completed.diagnostics.phase = Self.diagnosticPhase(for: result)
        if lastOperationDiagnostics?.identity == identity {
            lastOperationDiagnostics = completed.diagnostics
        }
        settle(identity, result: result)
    }

    private func settle(_ identity: IOSAudioOperationIdentity, result: IOSAudioOperationResult) {
        guard var operation = pending[identity], !operation.isTerminal else { return }
        operation.isTerminal = true
        let continuation = operation.continuation
        operation.continuation = nil
        pending[identity] = operation
        continuation?.resume(returning: result)
    }

    func endOwner(_ owner: IOSAudioOwner, excluding excludedIdentity: IOSAudioOperationIdentity? = nil) async {
        guard !endingOwners.contains(owner) else {
            await withCheckedContinuation { continuation in
                ownerEndWaiters[owner, default: []].append(continuation)
            }
            return
        }
        endingOwners.insert(owner)
        defer {
            endingOwners.remove(owner)
            let waiters = ownerEndWaiters.removeValue(forKey: owner) ?? []
            waiters.forEach { $0.resume() }
        }

        let requests = pending.values
            .filter { $0.request.owner == owner && $0.request.identity != excludedIdentity }
            .map(\.request.identity)
        for identity in requests { await cancel(identity) }
        await recorder.end(ownerID: owner.stableKey)
        recordings = recordings.filter { $0.value.owner != owner }
        if activePlaybackOwner == owner {
            await speechImplementation.stopAndWait()
            activePlaybackIdentity = nil
            activePlaybackOwner = nil
        }
        if activeListenOwner == owner {
            stt.cancelRecognition()
            activeListenIdentity = nil
            activeListenOwner = nil
        }
        if owner == .ui(instanceID: "flow-barge-in") {
            await bargeInImplementation.stopAll()
        }
        publishCapabilitySnapshot()
    }

    func status(owner: IOSAudioOwner, handle: String?) throws -> (recording: Bool, playing: Bool) {
        let recording: Bool
        if let handle {
            guard recordings[handle]?.owner == owner else { throw AudioServiceFailure.notRecording }
            recording = recorder.isRecordingOwned(handle: handle, ownerID: owner.stableKey)
        } else {
            recording = recordings.contains { $0.value.owner == owner }
                && recorder.isRecordingOwned(handle: nil, ownerID: owner.stableKey)
        }
        let playing = activePlaybackOwner == owner && hasActivePlayback
        return (recording, playing)
    }

    /// `VoiceTranscriptionSession` delegates to this entry point so hold-to-talk
    /// and engine Listen use the same configuration snapshot and native STT.
    func transcribeFromUI(
        language: String?,
        automaticEndpointAfterSilence: Duration?,
        configurationSnapshot: AudioConfigurationSnapshot
    ) async throws -> String {
        let identity = Self.newIdentity(epoch: serviceEpoch, generation: 1)
        let request = IOSAudioOperationRequest(
            identity: identity,
            owner: .ui(instanceID: "voice-capture"),
            initiator: nil,
            timeoutBudgetMs: nil,
            maxPayloadBytes: maximumPayloadBytes ?? maxAudioPayloadBytes(),
            operation: .listen(language: language)
        )
        let result = await execute(
            request,
            configurationSnapshot: configurationSnapshot,
            automaticEndpointAfterSilence: automaticEndpointAfterSilence
        )
        switch result {
        case let .transcript(text, _, _): return text
        case let .failed(error): throw Self.error(from: error)
        default: throw AudioServiceFailure.nativeFailure("audio service returned an unexpected transcription result")
        }
    }

    func transcribeForBargeIn(
        language: String?,
        configurationSnapshot: AudioConfigurationSnapshot,
        route: AudioRouteResolution,
        automaticEndpointAfterSilence: Duration
    ) async throws -> String {
        let request = IOSAudioOperationRequest(
            identity: Self.newIdentity(epoch: serviceEpoch, generation: 1),
            owner: .ui(instanceID: "flow-barge-in"),
            initiator: nil,
            timeoutBudgetMs: nil,
            maxPayloadBytes: maximumPayloadBytes ?? maxAudioPayloadBytes(),
            operation: .listen(language: language)
        )
        let result = await execute(
            request,
            configurationSnapshot: configurationSnapshot,
            automaticEndpointAfterSilence: automaticEndpointAfterSilence,
            routeResolution: route
        )
        switch result {
        case let .transcript(text, _, _): return text
        case let .failed(error): throw Self.error(from: error)
        default: throw AudioServiceFailure.nativeFailure("audio service returned an unexpected barge-in result")
        }
    }

    func cancelTranscription(owner: IOSAudioOwner) async {
        guard let identity = activeListenIdentity, activeListenOwner == owner else { return }
        await cancel(identity)
    }

    func finishUITranscription() {
        stt.finishRecording()
    }

    func cancelUITranscription() {
        guard let activeListenIdentity,
              pending[activeListenIdentity]?.request.owner == .ui(instanceID: "voice-capture")
        else { return }
        let identity = activeListenIdentity
        Task { @MainActor [weak self] in await self?.cancel(identity) }
    }

    // MARK: VoiceSpeechPlaying facade

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        let snapshot = configurationStore.snapshot
        let route = request.route ?? AudioConfigurationRuntime.route(
            kind: .speech,
            snapshot: snapshot,
            languageOverride: request.languageIdentifier,
            voiceOverride: request.voiceIdentifier
        )
        try Task.checkCancellation()
        guard route.status == .ready, route.effective != nil else { throw AudioServiceFailure.unavailable }
        let identity = Self.newIdentity(epoch: serviceEpoch, generation: 1)
        guard activePlaybackOwner == nil, !speechImplementation.hasActivePlayback else {
            throw AudioServiceFailure.busy
        }
        beginStandaloneDiagnostic(
            identity: identity,
            owner: .ui(instanceID: "voice-speech"),
            operation: "speak",
            configurationRevision: request.configurationRevision ?? snapshot.revision,
            route: route
        )
        activePlaybackIdentity = identity
        activePlaybackOwner = .ui(instanceID: "voice-speech")
        publishCapabilitySnapshot()
        do {
            var resolved = request
            resolved.route = route
            resolved.maxPayloadBytes = resolved.maxPayloadBytes ?? maximumPayloadBytes
            let outcome = try await speechImplementation.speak(resolved)
            clearPlaybackIfCurrent(identity)
            finishStandaloneDiagnostic(identity, phase: outcome == .completed ? "completed" : "interrupted")
            publishCapabilitySnapshot()
            return outcome
        } catch {
            clearPlaybackIfCurrent(identity)
            finishStandaloneDiagnostic(identity, phase: "failed")
            publishCapabilitySnapshot()
            throw error
        }
    }

    func stop() {
        let identity = activePlaybackIdentity
        speechImplementation.stop()
        if let identity { clearPlaybackIfCurrent(identity) }
        if let identity { finishStandaloneDiagnostic(identity, phase: "cancelled") }
        publishCapabilitySnapshot()
    }

    func openStream(
        configuration: VoiceSpeechConfiguration,
        managesAudioSession: Bool
    ) async throws -> any VoiceSpeechStreamingSession {
        let route = configuration.route ?? AudioConfigurationRuntime.route(
            kind: .speech,
            snapshot: configurationStore.snapshot,
            languageOverride: configuration.languageIdentifier,
            voiceOverride: configuration.voiceIdentifier
        )
        try Task.checkCancellation()
        guard route.status == .ready, route.effective != nil else { throw AudioServiceFailure.unavailable }
        guard activePlaybackOwner == nil, !speechImplementation.hasActivePlayback else {
            throw AudioServiceFailure.busy
        }
        let identity = Self.newIdentity(epoch: serviceEpoch, generation: 1)
        beginStandaloneDiagnostic(
            identity: identity,
            owner: .ui(instanceID: "flow-speech"),
            operation: "speak.stream",
            configurationRevision: configuration.configurationRevision ?? configurationStore.revision,
            route: route
        )
        activePlaybackIdentity = identity
        activePlaybackOwner = .ui(instanceID: "flow-speech")
        publishCapabilitySnapshot()
        do {
            var resolved = configuration
            resolved.route = route
            resolved.maxPayloadBytes = resolved.maxPayloadBytes ?? maximumPayloadBytes
            let base = try await speechImplementation.openStream(
                configuration: resolved,
                managesAudioSession: managesAudioSession
            )
            return ServiceTrackedSpeechSession(session: base) { [weak self] phase in
                self?.clearPlaybackIfCurrent(identity)
                self?.finishStandaloneDiagnostic(identity, phase: phase)
                self?.publishCapabilitySnapshot()
            }
        } catch {
            clearPlaybackIfCurrent(identity)
            finishStandaloneDiagnostic(identity, phase: "failed")
            publishCapabilitySnapshot()
            throw error
        }
    }

    // MARK: VoiceBargeInRecognizing facade

    func start(language: String?, prefersOnDevice: Bool) async throws -> any VoiceBargeInSession {
        let snapshot = configurationStore.snapshot
        return try await start(
            language: language,
            prefersOnDevice: prefersOnDevice,
            configurationSnapshot: snapshot
        )
    }

    func start(
        language: String?,
        prefersOnDevice: Bool,
        configurationSnapshot: AudioConfigurationSnapshot
    ) async throws -> any VoiceBargeInSession {
        let route = AudioConfigurationRuntime.route(
            kind: .recognition,
            snapshot: configurationSnapshot,
            languageOverride: language
        )
        let identity = Self.newIdentity(epoch: serviceEpoch, generation: 1)
        beginStandaloneDiagnostic(
            identity: identity,
            owner: .ui(instanceID: "flow-barge-in"),
            operation: "listen.monitor",
            configurationRevision: configurationSnapshot.revision,
            route: route
        )
        do {
            let session = try await bargeInImplementation.start(
                language: language,
                prefersOnDevice: prefersOnDevice,
                configurationSnapshot: configurationSnapshot
            )
            publishCapabilitySnapshot()
            return ServiceTrackedBargeInSession(session: session) { [weak self] phase in
                self?.finishStandaloneDiagnostic(identity, phase: phase)
                self?.publishCapabilitySnapshot()
            }
        } catch {
            finishStandaloneDiagnostic(identity, phase: "failed")
            publishCapabilitySnapshot()
            throw error
        }
    }

    // MARK: Operation implementation

    private func perform(
        _ request: IOSAudioOperationRequest,
        configurationSnapshot pinnedSnapshot: AudioConfigurationSnapshot?,
        automaticEndpointAfterSilence: Duration?,
        routeResolution pinnedRoute: AudioRouteResolution?
    ) async -> IOSAudioOperationResult {
        do {
            try Task.checkCancellation()
            switch request.operation {
            case let .startRecording(sampleRateHz, format):
                let handle = try await recorder.startRecordingOwned(
                    operationID: request.identity.id,
                    ownerID: request.ownerKey,
                    sampleRateHz: sampleRateHz,
                    format: format,
                    maximumBytes: request.maxPayloadBytes
                )
                recordings[handle] = RecordingOwner(owner: request.owner, startIdentity: request.identity)
                return .recordingStarted(handle: handle)

            case let .stopRecording(handle):
                guard recordings[handle]?.owner == request.owner else { throw AudioServiceFailure.notRecording }
                do {
                    let recording = try await recorder.stopRecordingOwned(
                        handle: handle,
                        ownerID: request.ownerKey,
                        maximumBytes: request.maxPayloadBytes
                    )
                    recordings.removeValue(forKey: handle)
                    return .recording(data: recording.audioBytes, mimeType: recording.mimeType)
                } catch {
                    if !recorder.isRecordingOwned(handle: handle, ownerID: request.ownerKey) {
                        recordings.removeValue(forKey: handle)
                    }
                    throw error
                }

            case let .listen(language):
                guard activeListenIdentity == nil else { throw AudioServiceFailure.busy }
                let snapshot = pinnedSnapshot ?? configurationStore.snapshot
                let route = if let pinnedRoute {
                    pinnedRoute
                } else {
                    await MainActor.run {
                    AudioConfigurationRuntime.route(
                        kind: .recognition,
                        snapshot: snapshot,
                        languageOverride: language
                    )
                }
                }
                recordRouteResolution(route, for: request.identity, phase: "running")
                try Task.checkCancellation()
                activeListenIdentity = request.identity
                activeListenOwner = request.owner
                let transcript = try await stt.transcribe(
                    language: language,
                    automaticEndpointAfterSilence: automaticEndpointAfterSilence,
                    routeResolution: route,
                    configurationSnapshot: snapshot,
                    maximumPayloadBytes: request.maxPayloadBytes,
                    audioSessionPurpose: Self.recognitionSessionPurpose(for: request.owner),
                    routeResolutionChanged: { [weak self] changedRoute in
                        self?.recordRouteResolution(changedRoute, for: request.identity, phase: "running")
                    }
                )
                if activeListenIdentity == request.identity {
                    activeListenIdentity = nil
                    activeListenOwner = nil
                }
                let resolvedLanguage = resolveAudioLanguageForNativeDevice(
                    configured: language ?? snapshot.configuration.language,
                    deviceLocale: Locale.autoupdatingCurrent.identifier
                )
                return .transcript(text: transcript, language: resolvedLanguage, confidence: nil)

            case let .synthesize(text, language, rate, voice):
                let snapshot = pinnedSnapshot ?? configurationStore.snapshot
                var configuration = snapshot.configuration
                if let language { configuration.language = language }
                if let rate {
                    guard rate.isFinite, rate >= Float(audioMinimumRate), rate <= Float(audioMaximumRate) else {
                        throw AudioServiceFailure.invalidRequest
                    }
                    configuration.rate = Double(rate)
                }
                let route = await MainActor.run {
                    AudioConfigurationRuntime.route(
                        kind: .speech,
                        snapshot: snapshot,
                        languageOverride: language,
                        voiceOverride: voice
                    )
                }
                recordRouteResolution(route, for: request.identity, phase: "running")
                try Task.checkCancellation()
                guard route.status == .ready else { throw Self.routeFailure(route) }
                let modelID = route.effective?.source == .offline ? route.effective?.modelId : nil
                if let modelID, modelRenderOwners[modelID] != nil { throw AudioServiceFailure.busy }
                let modelReference = modelID.map { modelID in
                    modelRenderOwners[modelID] = request.identity
                    return VoiceModelStore.shared.retainForAudioUse(modelID)
                }
                defer {
                    if let modelID, modelRenderOwners[modelID] == request.identity {
                        modelRenderOwners.removeValue(forKey: modelID)
                    }
                    if let modelReference { VoiceModelStore.shared.releaseAudioUse(modelReference) }
                }
                var lease: VoiceAudioSessionCoordinator.Lease?
                if route.effective?.source == .system {
                    activeSystemRenderIdentity = request.identity
                    do { lease = try await coordinator.acquire(.playback) }
                    catch {
                        if activeSystemRenderIdentity == request.identity { activeSystemRenderIdentity = nil }
                        throw AudioServiceFailure.busy
                    }
                }
                do {
                    try Task.checkCancellation()
                    let output = try await tts.render(
                        text: text,
                        configuration: configuration,
                        route: route,
                        maxPayloadBytes: request.maxPayloadBytes
                    )
                    if let lease { await coordinator.release(lease) }
                    if activeSystemRenderIdentity == request.identity { activeSystemRenderIdentity = nil }
                    return .synthesized(pcm: output.pcm, sampleRateHz: output.sampleRateHz)
                } catch {
                    if let lease { await coordinator.release(lease) }
                    if activeSystemRenderIdentity == request.identity { activeSystemRenderIdentity = nil }
                    throw error
                }

            case let .speak(text, language, rate, voice):
                guard activePlaybackOwner == nil, !speechImplementation.hasActivePlayback else {
                    throw AudioServiceFailure.busy
                }
                let snapshot = pinnedSnapshot ?? configurationStore.snapshot
                var configuration = snapshot.configuration
                if let language { configuration.language = language }
                if let rate {
                    guard rate.isFinite, rate >= Float(audioMinimumRate), rate <= Float(audioMaximumRate) else {
                        throw AudioServiceFailure.invalidRequest
                    }
                    configuration.rate = Double(rate)
                }
                let route = await MainActor.run {
                    AudioConfigurationRuntime.route(
                        kind: .speech,
                        snapshot: snapshot,
                        languageOverride: language,
                        voiceOverride: voice
                    )
                }
                recordRouteResolution(route, for: request.identity, phase: "running")
                try Task.checkCancellation()
                guard route.status == .ready else { throw Self.routeFailure(route) }
                let effective = route.effective
                let voiceIdentifier: String
                switch effective?.source {
                case .system:
                    voiceIdentifier = effective?.voiceId.map { "system:\($0)" } ?? "system:default"
                case .offline:
                    voiceIdentifier = "sherpa:\(effective?.modelId ?? "unknown"):\(effective?.voiceId ?? "unknown")"
                default:
                    throw AudioServiceFailure.unavailable
                }
                activePlaybackIdentity = request.identity
                activePlaybackOwner = request.owner
                let started = ContinuousClock.now
                let outcome = try await speechImplementation.speak(VoiceSpeechRequest(
                    text: text,
                    voiceIdentifier: voiceIdentifier,
                    languageIdentifier: resolveAudioLanguageForNativeDevice(
                        configured: configuration.language,
                        deviceLocale: Locale.autoupdatingCurrent.identifier
                    ),
                    speed: configuration.rate,
                    route: route,
                    maxPayloadBytes: request.maxPayloadBytes
                ))
                clearPlaybackIfCurrent(request.identity)
                guard outcome == .completed else { throw AudioServiceFailure.cancelled }
                let duration = started.duration(to: .now)
                return .playbackCompleted(durationMs: Self.milliseconds(duration))

            case let .status(handle):
                let current = try status(owner: request.owner, handle: handle)
                return .status(recording: current.recording, playing: current.playing)

            case .endOwner:
                await endOwner(request.owner, excluding: request.identity)
                return .ownerEnded
            }
        } catch {
            if activeListenIdentity == request.identity {
                activeListenIdentity = nil
                activeListenOwner = nil
            }
            clearPlaybackIfCurrent(request.identity)
            return .failed(Self.audioError(error))
        }
    }

    private func expire(_ identity: IOSAudioOperationIdentity) async {
        guard let operation = pending[identity], !operation.isTerminal, operation.terminalError == nil else { return }
        await cancel(
            identity,
            terminalError: .init(kind: .timeout, message: "The audio operation timed out.")
        )
    }

    private func cleanupLateResult(_ result: IOSAudioOperationResult, identity: IOSAudioOperationIdentity) async {
        switch result {
        case let .recordingStarted(handle):
            await recorder.cancel(startOperationID: identity.id)
            recordings.removeValue(forKey: handle)
        case .playbackCompleted:
            if activePlaybackIdentity == identity { await speechImplementation.stopAndWait() }
        default:
            break
        }
    }

    func handleInvalidation(_ event: VoiceAudioSessionCoordinator.Invalidation) async {
        guard await coordinator.owns(event.lease) else { return }
        let affected = pending.values.compactMap { operation -> IOSAudioOperationIdentity? in
            switch (event.lease.purpose, operation.request.operation) {
            case (.recording, .startRecording), (.recording, .stopRecording),
                 (.recognition, .listen), (.flowDuplex, .listen),
                 (.playback, .speak):
                return operation.request.identity
            case (.playback, .synthesize):
                return activeSystemRenderIdentity == operation.request.identity
                    ? operation.request.identity : nil
            default:
                return nil
            }
        }
        let oldRecordings = recordings
        let oldListenIdentity = activeListenIdentity
        let oldPlaybackIdentity = activePlaybackIdentity
        let oldSystemRenderIdentity = activeSystemRenderIdentity
        for identity in affected { await cancel(identity) }
        if await coordinator.owns(event.lease) {
            switch event.lease.purpose {
            case .recording:
                await recorder.stopAll()
            case .recognition:
                stt.cancelRecognition()
            case .playback:
                await speechImplementation.stopAndWait()
            case .flowDuplex:
                stt.cancelRecognition()
                await bargeInImplementation.stopAll()
            }
        }
        if event.lease.purpose == .recording {
            for (handle, recordedOwner) in oldRecordings where recordings[handle]?.startIdentity == recordedOwner.startIdentity {
                recordings.removeValue(forKey: handle)
            }
        }
        if (event.lease.purpose == .recognition || event.lease.purpose == .flowDuplex),
           activeListenIdentity == oldListenIdentity {
            activeListenIdentity = nil
            activeListenOwner = nil
        }
        if event.lease.purpose == .playback, activePlaybackIdentity == oldPlaybackIdentity {
            activePlaybackIdentity = nil
            activePlaybackOwner = nil
        }
        if activeSystemRenderIdentity == oldSystemRenderIdentity,
           event.lease.purpose == .playback {
            activeSystemRenderIdentity = nil
        }
        publishCapabilitySnapshot()
        await onInvalidation?(event)
    }

    private func clearPlaybackIfCurrent(_ identity: IOSAudioOperationIdentity) {
        guard activePlaybackIdentity == identity else { return }
        activePlaybackIdentity = nil
        activePlaybackOwner = nil
    }

    private func recordRouteResolution(
        _ route: AudioRouteResolution,
        for identity: IOSAudioOperationIdentity,
        phase: String
    ) {
        if var operation = pending[identity] {
            operation.diagnostics.requestedSource = route.requested.source.rawValue
            operation.diagnostics.effectiveSource = route.effective?.source.rawValue
            operation.diagnostics.fallbackReason = route.fallbackReason
            operation.diagnostics.phase = phase
            pending[identity] = operation
            lastOperationDiagnostics = operation.diagnostics
        } else if var last = lastOperationDiagnostics, last.identity == identity {
            last.requestedSource = route.requested.source.rawValue
            last.effectiveSource = route.effective?.source.rawValue
            last.fallbackReason = route.fallbackReason
            last.phase = phase
            lastOperationDiagnostics = last
        }
        publishCapabilitySnapshot()
    }

    private func beginStandaloneDiagnostic(
        identity: IOSAudioOperationIdentity,
        owner: IOSAudioOwner,
        operation: String,
        configurationRevision: UInt64,
        route: AudioRouteResolution
    ) {
        lastOperationDiagnostics = IOSAudioOperationDiagnostics(
            identity: identity,
            ownerKey: owner.stableKey,
            operation: operation,
            configurationRevision: configurationRevision,
            requestedSource: route.requested.source.rawValue,
            effectiveSource: route.effective?.source.rawValue,
            fallbackReason: route.fallbackReason,
            phase: "running"
        )
    }

    private func finishStandaloneDiagnostic(_ identity: IOSAudioOperationIdentity, phase: String) {
        guard var diagnostic = lastOperationDiagnostics, diagnostic.identity == identity else { return }
        diagnostic.phase = phase
        lastOperationDiagnostics = diagnostic
    }

    private func rememberSeenIdentity(_ identity: IOSAudioOperationIdentity) -> Bool {
        guard !seenIdentities.contains(identity),
              pending[identity] == nil,
              !recordings.values.contains(where: { $0.startIdentity == identity })
        else { return false }
        seenIdentities.insert(identity)
        seenIdentityOrder.append(identity)
        if seenIdentityOrder.count > 4_096 {
            seenIdentities.remove(seenIdentityOrder.removeFirst())
        }
        return true
    }

    private func retire(_ identity: IOSAudioOperationIdentity) {
        pruneRetiredIdentities()
        retiredIdentities[identity] = ContinuousClock.now.advanced(by: .seconds(120))
        if !retiredIdentityOrder.contains(identity) { retiredIdentityOrder.append(identity) }
        while retiredIdentityOrder.count > 1_024 {
            let oldest = retiredIdentityOrder.removeFirst()
            retiredIdentities.removeValue(forKey: oldest)
        }
    }

    private func clearRetired(_ identity: IOSAudioOperationIdentity) {
        retiredIdentities.removeValue(forKey: identity)
        retiredIdentityOrder.removeAll { $0 == identity }
    }

    private func pruneRetiredIdentities() {
        let now = ContinuousClock.now
        let expired = retiredIdentities.compactMap { identity, expiration in
            expiration <= now ? identity : nil
        }
        for identity in expired { retiredIdentities.removeValue(forKey: identity) }
        retiredIdentityOrder.removeAll { retiredIdentities[$0] == nil }
    }

    private static func audioError(_ error: Error) -> IOSAudioError {
        if error is CancellationError {
            return IOSAudioError(kind: .cancelled, message: "The audio operation was cancelled.")
        }
        if let failure = error as? AudioServiceFailure {
            let kind: IOSAudioErrorKind = switch failure {
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
            return IOSAudioError(kind: kind, message: failure.localizedDescription)
        }
        if let speech = error as? SpeechRecognitionError {
            switch speech {
            case .PermissionDenied:
                return IOSAudioError(kind: .permissionDenied, message: "Audio permission was denied.")
            case .NoSpeech:
                return IOSAudioError(kind: .noSpeech, message: "No speech was recognized.")
            case .Unavailable:
                return IOSAudioError(kind: .unavailable, message: "The requested audio provider is unavailable.")
            case .Busy:
                return IOSAudioError(kind: .busy, message: "Another audio operation is using the device.")
            case let .Retriable(message), let .Other(message):
                return IOSAudioError(kind: .nativeFailure, message: message)
            }
        }
        return IOSAudioError(kind: .nativeFailure, message: error.localizedDescription)
    }

    private static func error(from error: IOSAudioError) -> AudioServiceFailure {
        switch error.kind {
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
        case .synthesisFailed: .synthesisFailed(error.message, operationStarted: false)
        case .nativeFailure: .nativeFailure(error.message)
        case .mediaTooLarge: .mediaTooLarge
        }
    }

    private static func routeFailure(_ route: AudioRouteResolution) -> AudioServiceFailure {
        if route.reason.localizedCaseInsensitiveContains("model") { return .modelMissing }
        if route.reason.localizedCaseInsensitiveContains("voice") { return .voiceMissing }
        if route.status == .invalidRequest { return .invalidRequest }
        return .unavailable
    }

    private static func readiness(for route: AudioRouteResolution) -> IOSAudioCapabilityState.Readiness {
        switch route.status {
        case .ready: .ready
        case .permissionRequired: .needsPermission
        case .invalidRequest: .unavailable
        case .unavailable:
            route.reason.localizedCaseInsensitiveContains("model") ? .missingModel : .unavailable
        }
    }

    private static func operationName(_ operation: IOSAudioOperation) -> String {
        switch operation {
        case .startRecording: "record.start"
        case .stopRecording: "record.stop"
        case .listen: "listen"
        case .synthesize: "synthesize"
        case .speak: "speak"
        case .status: "status"
        case .endOwner: "end_owner"
        }
    }

    private static func requestedSource(
        for operation: IOSAudioOperation,
        snapshot: AudioConfigurationSnapshot
    ) -> AudioSource? {
        switch operation {
        case .listen: snapshot.configuration.recognition.source
        case .synthesize, .speak: snapshot.configuration.speech.source
        case .startRecording, .stopRecording, .status, .endOwner: nil
        }
    }

    private static func diagnosticPhase(for result: IOSAudioOperationResult) -> String {
        if case let .failed(error) = result { return "failed:\(error.kind.rawValue)" }
        return switch result {
        case .recordingStarted: "recording"
        case .recording, .transcript, .synthesized, .playbackCompleted, .status, .ownerEnded: "completed"
        case .failed: "failed"
        }
    }

    private static func isValidIdentity(_ identity: IOSAudioOperationIdentity) -> Bool {
        guard let uuid = UUID(uuidString: identity.id) else { return false }
        let normalized = uuid.uuidString.lowercased()
        return normalized == identity.id.lowercased()
            && normalized.dropFirst(14).first == "4"
            && identity.generation <= 9_007_199_254_740_991
            && identity.serviceEpoch <= 9_007_199_254_740_991
    }

    private static func freshServiceEpoch() -> UInt64 {
        UInt64.random(in: 1 ... 9_007_199_254_740_991)
    }

    private static func newIdentity(epoch: UInt64, generation: UInt64) -> IOSAudioOperationIdentity {
        IOSAudioOperationIdentity(id: UUID().uuidString.lowercased(), generation: generation, serviceEpoch: epoch)
    }

    private static func milliseconds(_ duration: Duration) -> UInt64 {
        let components = duration.components
        let seconds = max(0, components.seconds)
        let millisecondsFromSeconds = UInt64(seconds) &* 1_000
        let millisecondsFromAttoseconds = UInt64(max(0, components.attoseconds) / 1_000_000_000_000_000)
        return millisecondsFromSeconds &+ millisecondsFromAttoseconds
    }
}

@MainActor
private final class ServiceTrackedSpeechSession: VoiceSpeechStreamingSession {
    private let session: any VoiceSpeechStreamingSession
    private let onTerminal: @MainActor (String) -> Void
    private var terminal = false

    init(session: any VoiceSpeechStreamingSession, onTerminal: @escaping @MainActor (String) -> Void) {
        self.session = session
        self.onTerminal = onTerminal
    }

    func enqueue(_ text: String) { session.enqueue(text) }
    func pause() { session.pause() }
    func resume() { session.resume() }

    func finish() async throws -> VoiceSpeechPlaybackOutcome {
        do {
            let result = try await session.finish()
            if result == .completed { finishTracking(phase: "completed") }
            else { finishTracking(phase: "interrupted") }
            return result
        } catch {
            finishTracking(phase: "failed")
            throw error
        }
    }

    func stop() async {
        await session.stop()
        finishTracking(phase: "cancelled")
    }

    private func finishTracking(phase: String) {
        guard !terminal else { return }
        terminal = true
        onTerminal(phase)
    }
}

@MainActor
private final class ServiceTrackedBargeInSession: VoiceBargeInSession {
    let events: AsyncStream<VoiceBargeInEvent>
    private let session: any VoiceBargeInSession
    private let onStop: @MainActor (String) -> Void
    private var stopped = false

    init(session: any VoiceBargeInSession, onStop: @escaping @MainActor (String) -> Void) {
        self.session = session
        events = session.events
        self.onStop = onStop
    }

    func stop() async {
        guard !stopped else { return }
        stopped = true
        await session.stop()
        onStop("cancelled")
    }
}
