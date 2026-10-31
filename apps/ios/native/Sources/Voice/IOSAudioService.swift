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

    private struct CancellationFlight {
        let token: UUID
        let task: Task<IOSAudioError?, Never>
    }

    private struct CancellationOutcome {
        let error: IOSAudioError?
        let expiresAt: ContinuousClock.Instant
    }

    private let configurationStore: AudioConfigurationStore
    private let stt: SttImpl
    private let tts: TtsImpl
    private let recorder: any AudioRecordingDriving
    private let speechImplementation: SystemVoiceSpeechPlayer
    private let bargeInImplementation: VoiceBargeInRecognizer
    private let coordinator: VoiceAudioSessionCoordinator
    private let pcmPlayback: any IOSPcmPlaybackDriving
    private let providerService: IOSAudioProviderService
    private let cancellationCompletionTimeout: Duration
    private var cloudCallbacks: [IOSAudioOperationIdentity: IOSAudioProviderService.PinnedOperation] = [:]
    private var activeCloudOperations: [IOSAudioOwner: IOSAudioProviderService.PinnedOperation] = [:]
    private var finishedCaptures: Set<IOSAudioOperationIdentity> = []
    private let onInvalidation: (@MainActor (VoiceAudioSessionCoordinator.Invalidation) async -> Void)?
    private weak var capabilityCache: IOSAudioCapabilitySnapshotCache?
    private var invalidationTask: Task<Void, Never>?
    private var scheduledCapabilityRefresh: Task<Void, Never>?
    private var capabilityObservers: [NSObjectProtocol] = []
    private var pending: [IOSAudioOperationIdentity: PendingOperation] = [:]
    private var cancellationFlights: [IOSAudioOperationIdentity: CancellationFlight] = [:]
    private var unconfirmedOperationTasks: [IOSAudioOperationIdentity: Task<Void, Never>] = [:]
    private let playbackStopReceipts = IOSAudioPlaybackStopReceipts()
    private var cancellationOutcomes: [IOSAudioOperationIdentity: CancellationOutcome] = [:]
    private var cancellationOutcomeOrder: [IOSAudioOperationIdentity] = []
    private var seenIdentities: Set<IOSAudioOperationIdentity> = []
    private var seenIdentityOrder: [IOSAudioOperationIdentity] = []
    private var retiredIdentities: [IOSAudioOperationIdentity: ContinuousClock.Instant] = [:]
    private var retiredIdentityOrder: [IOSAudioOperationIdentity] = []
    private var endingOwners: Set<IOSAudioOwner> = []
    private var cancellationUnconfirmedOwners: Set<IOSAudioOwner> = []
    private var unconfirmedCancellationOwnersByIdentity: [IOSAudioOperationIdentity: IOSAudioOwner] = [:]
    private var ownerEndQuarantines: Set<IOSAudioOwner> = []
    private var cancellationQuarantineOverflow = false
    private var ownerEndWaiters: [IOSAudioOwner: [CheckedContinuation<Void, Never>]] = [:]
    private var recordings: [String: RecordingOwner] = [:]
    /// Retained until this exact recorder incarnation produces a close
    /// receipt, including failed starts and captures whose handle was retired.
    private var recordingAttempts: [IOSAudioOperationIdentity: IOSAudioOwner] = [:]
    private var recordingCancelTasks: [IOSAudioOperationIdentity: Task<Void, Never>] = [:]
    private var recordingReceiptTasks: [IOSAudioOperationIdentity: Task<Bool, Never>] = [:]
    private var recordingEndTasks: [IOSAudioOwner: Task<Void, Never>] = [:]
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
        pcmPlayback: (any IOSPcmPlaybackDriving)? = nil,
        providerService: IOSAudioProviderService? = nil,
        serviceEpoch: UInt64? = nil,
        maximumPayloadBytes: UInt64? = nil,
        cancellationCompletionTimeout: Duration = .seconds(5),
        onInvalidation: (@MainActor (VoiceAudioSessionCoordinator.Invalidation) async -> Void)? = nil
    ) {
        let coordinator = coordinator ?? VoiceAudioSessionCoordinator.shared
        self.coordinator = coordinator
        self.pcmPlayback = pcmPlayback ?? IOSPcmPlayback(coordinator: coordinator)
        self.providerService = providerService ?? .shared
        self.cancellationCompletionTimeout = cancellationCompletionTimeout
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
        activePlaybackIdentity != nil || speechImplementation.hasActivePlayback || pcmPlayback.isPlaying
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
        var all: Set<IOSAudioCapabilityState.Operation> = [.record, .capture, .play, .listen, .synthesize, .speak]
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
        if configurationStore.configuration.recognition.source == .provider,
           providerService.capabilities[.recognition]?.route.supported == true { all.insert(.transcribe) }
        let recognitionReadiness = Self.readiness(for: recognitionRoute)
        let speechReadiness = Self.readiness(for: speechRoute)
        let rawRecordingActive = recordings.contains { handle, recording in
            recorder.isRecordingOwned(handle: handle, ownerID: recording.owner.stableKey)
        } || pending.values.contains {
            if case .startRecording = $0.request.operation { return !$0.isTerminal }
            if case .capture = $0.request.operation { return !$0.isTerminal }
            return false
        }
        let realtimeActive = IOSRealtimeAudioService.shared.isActive
        let isListeningActive = realtimeActive || activeListenIdentity != nil || pending.values.contains {
            if case .listen = $0.request.operation { return !$0.isTerminal }
            return false
        } || bargeInImplementation.activeSessionCount > 0
        let isSynthesizingActive = activeSystemRenderIdentity != nil || !modelRenderOwners.isEmpty || pending.values.contains {
            if case .synthesize = $0.request.operation { return !$0.isTerminal }
            return false
        }
        let isPlaybackActive = realtimeActive || activePlaybackIdentity != nil || speechImplementation.hasActivePlayback || pcmPlayback.isPlaying || pending.values.contains {
            if case .speak = $0.request.operation { return !$0.isTerminal }
            if case .play = $0.request.operation { return !$0.isTerminal }
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
                .capture: (rawRecordingActive || isListeningActive || isPlaybackActive) ? .busy : microphoneReadiness,
                .play: isPlaybackBusy ? .busy : .ready,
                .transcribe: recognitionReadiness,
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
        guard !endingOwners.contains(request.owner),
              !cancellationUnconfirmedOwners.contains(request.owner),
              !cancellationQuarantineOverflow else {
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

    /// Cancellation is scoped to one UUID/generation/epoch. A successful
    /// result means the operation task, cloud cancellation, and owned native
    /// resource have all stopped; unconfirmed cancellation is returned as an
    /// error instead of being reported as success.
    @discardableResult
    func cancel(
        _ identity: IOSAudioOperationIdentity,
        terminalError: IOSAudioError = .init(kind: .cancelled, message: "The audio operation was cancelled.")
    ) async -> IOSAudioError? {
        guard Self.isValidIdentity(identity), identity.serviceEpoch == serviceEpoch else {
            return IOSAudioError(kind: .invalidRequest, message: "Invalid audio cancellation identity.")
        }
        let cachedOutcome = cachedCancellationOutcome(for: identity)
        if cachedOutcome.found { return cachedOutcome.error }
        if let flight = cancellationFlights[identity] { return await flight.task.value }

        retire(identity)
        let token = UUID()
        let task = Task<IOSAudioError?, Never> { @MainActor [weak self] in
            guard let self else {
                return IOSAudioError(kind: .nativeFailure, message: "Audio cancellation service is unavailable.")
            }
            return await self.performCancellation(identity, terminalError: terminalError)
        }
        cancellationFlights[identity] = CancellationFlight(token: token, task: task)
        let outcome = await task.value
        if cancellationFlights[identity]?.token == token {
            cancellationFlights.removeValue(forKey: identity)
        }
        if outcome == nil { confirmCancellation(identity) }
        rememberCancellationOutcome(outcome, for: identity)
        return outcome
    }

    private func performCancellation(
        _ identity: IOSAudioOperationIdentity,
        terminalError: IOSAudioError
    ) async -> IOSAudioError? {
        guard var operation = pending[identity], !operation.isTerminal else {
            return await cancelOrphanedResources(identity)
        }
        guard operation.terminalError == nil else {
            return IOSAudioError(
                kind: .nativeFailure,
                message: "A previous cancellation is still unconfirmed for this operation."
            )
        }
        let owner = operation.request.owner
        let insertedEndingOwner = endingOwners.insert(owner).inserted
        defer {
            if insertedEndingOwner {
                endingOwners.remove(owner)
                let waiters = ownerEndWaiters.removeValue(forKey: owner) ?? []
                waiters.forEach { $0.resume() }
            }
        }
        operation.terminalError = terminalError
        operation.diagnostics.phase = terminalError.kind.rawValue
        pending[identity] = operation
        lastOperationDiagnostics = operation.diagnostics
        operation.task.cancel()
        let cloudCancellation = cloudCallbacks[identity].map { cloud in
            Task { @MainActor in await cancelCloudOperation(cloud) }
        }
        var playbackStopUnconfirmed = false
        var recorderCancellation: Task<Void, Never>?

        switch operation.request.operation {
        case .startRecording, .capture:
            recorderCancellation = requestRecordingCancellation(identity)
        case .listen:
            if activeListenIdentity == identity {
                recorderCancellation = requestRecordingCancellation(identity)
                stt.cancelRecognition()
                activeListenIdentity = nil
                activeListenOwner = nil
            }
        case .speak, .play:
            if activePlaybackIdentity == identity || playbackStopReceipts.contains(identity) {
                let stopTask = requestPlaybackStop(identity)
                playbackStopUnconfirmed = !(await waitForTaskCompletion(
                    stopTask,
                    timeout: cancellationCompletionTimeout
                ))
            }
        case .transcribe, .stopRecording, .synthesize, .status, .endOwner:
            break
        }

        var failures: [String] = []
        let recorderCancelSettled: Bool
        if let recorderCancellation {
            recorderCancelSettled = await waitForTaskCompletion(recorderCancellation, timeout: cancellationCompletionTimeout)
            if recorderCancelSettled { recordingCancelTasks.removeValue(forKey: identity) }
            else { failures.append("The recording cancellation did not finish before the deadline.") }
        } else {
            recorderCancelSettled = true
        }
        if playbackStopUnconfirmed {
            failures.append("Native playback teardown did not finish before the cancellation deadline.")
        }
        let taskSettled = await waitForTaskCompletion(operation.task, timeout: cancellationCompletionTimeout)
        if !taskSettled {
            unconfirmedOperationTasks[identity] = operation.task
            failures.append("The audio operation did not finish before the cancellation deadline.")
        } else {
            unconfirmedOperationTasks.removeValue(forKey: identity)
        }
        if taskSettled, recorderCancelSettled, recordingAttempts[identity] != nil {
            if await confirmRecordingClose(identity) {
                recorder.retireCloseReceipt(startOperationID: identity.recordingDriverKey)
                recordingAttempts.removeValue(forKey: identity)
                recordings = recordings.filter { $0.value.startIdentity != identity }
            } else {
                failures.append("The exact recording close is not confirmed.")
            }
        }
        if let cloudCancellation {
            if let cloudError = await cloudCancellation.value {
                failures.append(cloudError.message)
            }
        }
        if nativeResourceIsActive(for: operation.request) {
            failures.append("A native audio resource is still active after cancellation.")
        }
        let cancellationFailure = failures.isEmpty ? nil : IOSAudioError(
            kind: .nativeFailure,
            message: failures.joined(separator: " ")
        )
        if let cancellationFailure {
            operation.diagnostics.phase = cancellationFailure.kind.rawValue
            pending[identity] = operation
            if lastOperationDiagnostics?.identity == identity {
                lastOperationDiagnostics = operation.diagnostics
            }
            quarantineCancellationOwner(owner, identity: identity)
        }
        settle(identity, result: .failed(cancellationFailure ?? terminalError))
        return cancellationFailure
    }

    func cancelCloudOperation(
        _ cloud: IOSAudioProviderService.PinnedOperation
    ) async -> IOSAudioError? {
        let task = Task { @MainActor in
            do {
                try await cloud.host.cancel(operationId: cloud.id)
                return nil
            } catch {
                return IOSAudioError(
                    kind: .nativeFailure,
                    message: "Cloud provider cancellation failed: \(Self.audioError(error).message)"
                )
            }
        }
        guard await waitForTaskCompletion(task, timeout: cancellationCompletionTimeout) else {
            task.cancel()
            return IOSAudioError(
                kind: .nativeFailure,
                message: "The cloud provider did not confirm cancellation before the deadline."
            )
        }
        return await task.value
    }

    private func cancelOrphanedResources(_ identity: IOSAudioOperationIdentity) async -> IOSAudioError? {
        var failures: [String] = []
        var affectedOwners = Set<IOSAudioOwner>()
        var taskSettled = true
        if let operationTask = pending[identity]?.task ?? unconfirmedOperationTasks[identity] {
            operationTask.cancel()
            if !(await waitForTaskCompletion(operationTask, timeout: cancellationCompletionTimeout)) {
                unconfirmedOperationTasks[identity] = operationTask
                taskSettled = false
                failures.append("The audio operation remains in flight after cancellation.")
            } else {
                unconfirmedOperationTasks.removeValue(forKey: identity)
            }
            if let owner = pending[identity]?.request.owner {
                affectedOwners.insert(owner)
            } else if let owner = unconfirmedCancellationOwnersByIdentity[identity] {
                affectedOwners.insert(owner)
            }
        }
        if let (_, recording) = recordings.first(where: { $0.value.startIdentity == identity }) {
            affectedOwners.insert(recording.owner)
        }
        if let owner = recordingAttempts[identity] { affectedOwners.insert(owner) }
        if recordingAttempts[identity] != nil || recordingCancelTasks[identity] != nil {
            let cancellation = requestRecordingCancellation(identity)
            let cancellationSettled = await waitForTaskCompletion(cancellation, timeout: cancellationCompletionTimeout)
            if cancellationSettled { recordingCancelTasks.removeValue(forKey: identity) }
            if !cancellationSettled {
                failures.append("The recording cancellation remains in flight.")
            } else if recordingAttempts[identity] == nil {
                // The exact operation completed without admitting a recorder.
            } else if taskSettled, await confirmRecordingClose(identity) {
                recorder.retireCloseReceipt(startOperationID: identity.recordingDriverKey)
                recordingAttempts.removeValue(forKey: identity)
                recordings = recordings.filter { $0.value.startIdentity != identity }
            } else {
                failures.append("The exact recording close is not confirmed.")
            }
        }
        if activeListenIdentity == identity {
            if let activeListenOwner { affectedOwners.insert(activeListenOwner) }
            let cancellation = requestRecordingCancellation(identity)
            if await waitForTaskCompletion(cancellation, timeout: cancellationCompletionTimeout) {
                recordingCancelTasks.removeValue(forKey: identity)
            } else {
                failures.append("The recording cancellation remains in flight.")
            }
            stt.cancelRecognition()
            activeListenIdentity = nil
            activeListenOwner = nil
            if stt.hasActiveNativeOperation {
                failures.append("The speech recognition resource is still active after cancellation.")
            }
        }
        if activePlaybackIdentity == identity || playbackStopReceipts.contains(identity) {
            if let activePlaybackOwner { affectedOwners.insert(activePlaybackOwner) }
            let stopTask = requestPlaybackStop(identity)
            if !(await waitForTaskCompletion(stopTask, timeout: cancellationCompletionTimeout)) {
                failures.append("The playback resource did not finish stopping before the cancellation deadline.")
            }
        }
        if activeSystemRenderIdentity == identity || modelRenderOwners.values.contains(identity) {
            failures.append("The synthesis task has no completed cancellation record.")
        }
        if let cloud = cloudCallbacks[identity] {
            if let owner = activeCloudOperations.first(where: { $0.value.id == cloud.id })?.key {
                affectedOwners.insert(owner)
            }
            if let error = await cancelCloudOperation(cloud) {
                failures.append(error.message)
            }
        }
        guard !failures.isEmpty else { return nil }
        affectedOwners.forEach { quarantineCancellationOwner($0, identity: identity) }
        return IOSAudioError(kind: .nativeFailure, message: failures.joined(separator: " "))
    }

    private func confirmRecordingClose(_ identity: IOSAudioOperationIdentity) async -> Bool {
        let task = recordingReceiptTasks[identity] ?? Task { [recorder] in
            await recorder.cancelWithReceipt(startOperationID: identity.recordingDriverKey)
        }
        recordingReceiptTasks[identity] = task
        guard await waitForTaskCompletion(task, timeout: cancellationCompletionTimeout) else { return false }
        let result = await task.value
        recordingReceiptTasks.removeValue(forKey: identity)
        return result
    }

    private func requestRecordingCancellation(_ identity: IOSAudioOperationIdentity) -> Task<Void, Never> {
        if let task = recordingCancelTasks[identity] { return task }
        let task = Task { [recorder] in await recorder.cancel(startOperationID: identity.recordingDriverKey) }
        recordingCancelTasks[identity] = task
        return task
    }

    private func quarantineCancellationOwner(
        _ owner: IOSAudioOwner,
        identity: IOSAudioOperationIdentity? = nil,
        ownerEnd: Bool = false
    ) {
        guard !cancellationQuarantineOverflow else { return }
        if let identity {
            if let existing = unconfirmedCancellationOwnersByIdentity[identity], existing != owner {
                cancellationQuarantineOverflow = true
                cancellationUnconfirmedOwners.removeAll()
                unconfirmedCancellationOwnersByIdentity.removeAll()
                ownerEndQuarantines.removeAll()
                return
            }
            if unconfirmedCancellationOwnersByIdentity[identity] == nil {
                guard unconfirmedCancellationOwnersByIdentity.count < 1_024 else {
                    cancellationQuarantineOverflow = true
                    cancellationUnconfirmedOwners.removeAll()
                    unconfirmedCancellationOwnersByIdentity.removeAll()
                    ownerEndQuarantines.removeAll()
                    return
                }
                unconfirmedCancellationOwnersByIdentity[identity] = owner
            }
        }
        if ownerEnd { ownerEndQuarantines.insert(owner) }
        if cancellationUnconfirmedOwners.contains(owner) { return }
        if cancellationUnconfirmedOwners.count >= 256 {
            cancellationQuarantineOverflow = true
            cancellationUnconfirmedOwners.removeAll()
            unconfirmedCancellationOwnersByIdentity.removeAll()
            ownerEndQuarantines.removeAll()
            return
        }
        cancellationUnconfirmedOwners.insert(owner)
    }

    private func confirmCancellation(_ identity: IOSAudioOperationIdentity) {
        guard let owner = unconfirmedCancellationOwnersByIdentity.removeValue(forKey: identity),
              !ownerEndQuarantines.contains(owner),
              !unconfirmedCancellationOwnersByIdentity.values.contains(owner) else { return }
        cancellationUnconfirmedOwners.remove(owner)
    }

    private func nativeResourceIsActive(for request: IOSAudioOperationRequest) -> Bool {
        switch request.operation {
        case .startRecording, .capture:
            recorder.isRecordingOwned(handle: nil, ownerID: request.ownerKey)
        case .listen:
            ((activeListenIdentity == nil || activeListenIdentity == request.identity)
                && stt.hasActiveNativeOperation)
                || recorder.isRecordingOwned(handle: nil, ownerID: request.ownerKey)
        case .speak, .play:
            playbackStopReceipts.contains(request.identity)
                || (activePlaybackIdentity == request.identity
                    && (speechImplementation.hasActivePlayback || pcmPlayback.isPlaying))
        case .synthesize:
            activeSystemRenderIdentity == request.identity || modelRenderOwners.values.contains(request.identity)
        case .transcribe, .stopRecording, .status, .endOwner:
            false
        }
    }

    func waitForTaskCompletion<T: Sendable>(
        _ task: Task<T, Never>,
        timeout: Duration
    ) async -> Bool {
        await withCheckedContinuation { (continuation: CheckedContinuation<Bool, Never>) in
            let waiter = CancellationCompletionWaiter(continuation)
            let observer = Task { @MainActor in
                _ = await task.value
                waiter.resolve(true)
            }
            let timer = Task { @MainActor in
                do { try await Task.sleep(for: timeout) } catch { return }
                waiter.resolve(false)
            }
            waiter.install(observer: observer, timer: timer)
        }
    }

    private func cachedCancellationOutcome(
        for identity: IOSAudioOperationIdentity
    ) -> (found: Bool, error: IOSAudioError?) {
        pruneCancellationOutcomes()
        guard let outcome = cancellationOutcomes[identity] else { return (false, nil) }
        return (true, outcome.error)
    }

    private func rememberCancellationOutcome(
        _ error: IOSAudioError?,
        for identity: IOSAudioOperationIdentity
    ) {
        guard error == nil else { return }
        pruneCancellationOutcomes()
        cancellationOutcomes[identity] = CancellationOutcome(
            error: error,
            expiresAt: ContinuousClock.now.advanced(by: .seconds(120))
        )
        if !cancellationOutcomeOrder.contains(identity) {
            cancellationOutcomeOrder.append(identity)
        }
        while cancellationOutcomeOrder.count > 1_024 {
            let oldest = cancellationOutcomeOrder.removeFirst()
            cancellationOutcomes.removeValue(forKey: oldest)
        }
    }

    private func pruneCancellationOutcomes() {
        let now = ContinuousClock.now
        let expired = cancellationOutcomes.compactMap { identity, outcome in
            outcome.expiresAt <= now ? identity : nil
        }
        for identity in expired { cancellationOutcomes.removeValue(forKey: identity) }
        cancellationOutcomeOrder.removeAll { cancellationOutcomes[$0] == nil }
    }

    private func completeOperation(_ identity: IOSAudioOperationIdentity, result: IOSAudioOperationResult) async {
        guard let operation = pending[identity] else {
            if retiredIdentities[identity] != nil {
                await cleanupLateResult(result, identity: identity)
                clearRetired(identity)
            }
            unconfirmedOperationTasks.removeValue(forKey: identity)
            return
        }
        if operation.terminalError != nil {
            if lastOperationDiagnostics?.identity == identity {
                lastOperationDiagnostics = operation.diagnostics
            }
            await cleanupLateResult(result, identity: identity)
            clearRetired(identity)
            unconfirmedOperationTasks.removeValue(forKey: identity)
            return
        }
        guard !operation.isTerminal else {
            await cleanupLateResult(result, identity: identity)
            clearRetired(identity)
            unconfirmedOperationTasks.removeValue(forKey: identity)
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

    @discardableResult
    func endOwner(
        _ owner: IOSAudioOwner,
        excluding excludedIdentity: IOSAudioOperationIdentity? = nil
    ) async -> IOSAudioError? {
        guard !endingOwners.contains(owner) else {
            await withCheckedContinuation { continuation in
                ownerEndWaiters[owner, default: []].append(continuation)
            }
            return await endOwner(owner, excluding: excludedIdentity)
        }
        endingOwners.insert(owner)
        defer {
            endingOwners.remove(owner)
            let waiters = ownerEndWaiters.removeValue(forKey: owner) ?? []
            waiters.forEach { $0.resume() }
        }

        let requests = Set(pending.values
            .filter { $0.request.owner == owner && $0.request.identity != excludedIdentity }
            .map(\.request.identity)).union(recordingAttempts.compactMap {
                $0.value == owner && $0.key != excludedIdentity ? $0.key : nil
            })
        var failures: [String] = []
        for identity in requests {
            if let error = await cancel(identity) { failures.append(error.message) }
        }
        let recordingEnd = recordingEndTasks[owner] ?? Task { [recorder] in
            await recorder.end(ownerID: owner.stableKey)
        }
        recordingEndTasks[owner] = recordingEnd
        if await waitForTaskCompletion(recordingEnd, timeout: cancellationCompletionTimeout) {
            recordingEndTasks.removeValue(forKey: owner)
        } else {
            failures.append("Recording owner cleanup did not finish before the deadline.")
        }
        if activePlaybackOwner == owner {
            if let identity = activePlaybackIdentity {
                let stopTask = requestPlaybackStop(identity)
                await stopTask.value
            } else {
                pcmPlayback.stop()
                await speechImplementation.stopAndWait()
                activePlaybackOwner = nil
            }
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
        if recorder.isRecordingOwned(handle: nil, ownerID: owner.stableKey) {
            failures.append("A recording resource is still active after owner cleanup.")
        }
        if activeListenOwner == owner && stt.hasActiveNativeOperation {
            failures.append("Speech recognition is still active after owner cleanup.")
        }
        if activePlaybackOwner == owner && (speechImplementation.hasActivePlayback || pcmPlayback.isPlaying) {
            failures.append("Playback is still active after owner cleanup.")
        }
        guard !failures.isEmpty else {
            ownerEndQuarantines.remove(owner)
            if !unconfirmedCancellationOwnersByIdentity.values.contains(owner) {
                cancellationUnconfirmedOwners.remove(owner)
            }
            return nil
        }
        quarantineCancellationOwner(owner, ownerEnd: true)
        return IOSAudioError(kind: .nativeFailure, message: failures.joined(separator: " "))
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
        if configurationSnapshot.configuration.recognition.source == .provider {
            let cloud = try await providerService.pin(kind: .recognition, snapshot: configurationSnapshot)
            let owner = IOSAudioOwner.ui(instanceID: "voice-capture")
            guard activeListenIdentity == nil else { throw AudioServiceFailure.busy }
            let identity = Self.newIdentity(epoch: serviceEpoch, generation: 1)
            activeCloudOperations[owner] = cloud
            activeListenIdentity = identity
            activeListenOwner = owner
            defer {
                activeCloudOperations.removeValue(forKey: owner)
                if activeListenIdentity == identity {
                    activeListenIdentity = nil
                    activeListenOwner = nil
                }
                publishCapabilitySnapshot()
            }
            let result = await execute(IOSAudioOperationRequest(
                identity: identity, owner: owner, initiator: nil, timeoutBudgetMs: 60_000,
                maxPayloadBytes: maximumPayloadBytes ?? maxAudioPayloadBytes(),
                operation: .capture(sampleRateHz: 24_000, format: "m4a")
            ), configurationSnapshot: configurationSnapshot, automaticEndpointAfterSilence: automaticEndpointAfterSilence)
            switch result {
            case let .recording(data, mimeType):
                try Task.checkCancellation()
                return try await providerService.transcribe(cloud, recording: IOSAudioRecording(audioBytes: data, mimeType: mimeType)).text
            case let .failed(error): throw Self.error(from: error)
            default: throw AudioServiceFailure.nativeFailure("The audio service did not return a microphone recording.")
            }
        }
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
        for operation in pending.values where operation.request.owner == .ui(instanceID: "voice-capture") {
            if case .capture = operation.request.operation { finishedCaptures.insert(operation.request.identity) }
        }
        stt.finishRecording()
    }

    func cancelUITranscription() {
        if let cloud = activeCloudOperations[.ui(instanceID: "voice-capture")] {
            Task { @MainActor in try? await cloud.host.cancel(operationId: cloud.id) }
        }
        guard let activeListenIdentity,
              pending[activeListenIdentity]?.request.owner == .ui(instanceID: "voice-capture")
        else { return }
        let identity = activeListenIdentity
        Task { @MainActor [weak self] in await self?.cancel(identity) }
    }

    // MARK: VoiceSpeechPlaying facade

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        let snapshot = configurationStore.snapshot
        if snapshot.configuration.speech.source == .provider || request.route?.effective?.source == .provider {
            return try await speakCloud(request, snapshot: Self.cloudSnapshot(snapshot, route: request.route, language: request.languageIdentifier, rate: request.speed))
        }
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
        for (owner, cloud) in activeCloudOperations where owner != .ui(instanceID: "voice-capture") {
            Task { @MainActor in try? await cloud.host.cancel(operationId: cloud.id) }
        }
        if let identity {
            _ = requestPlaybackStop(identity)
        } else {
            pcmPlayback.stop()
            Task { @MainActor [speechImplementation] in await speechImplementation.stopAndWait() }
        }
        publishCapabilitySnapshot()
    }

    func openStream(
        configuration: VoiceSpeechConfiguration,
        managesAudioSession: Bool
    ) async throws -> any VoiceSpeechStreamingSession {
        if configuration.route?.effective?.source == .provider || configurationStore.configuration.speech.source == .provider {
            let snapshot = Self.cloudSnapshot(configurationStore.snapshot, route: configuration.route, language: configuration.languageIdentifier, rate: configuration.speed)
            let cloud = try await providerService.pin(kind: .speech, snapshot: snapshot, voice: snapshot.configuration.speech.voice?.id)
            let identity = Self.newIdentity(epoch: serviceEpoch, generation: 1)
            let owner = IOSAudioOwner.ui(instanceID: "flow-speech")
            guard activePlaybackOwner == nil, !speechImplementation.hasActivePlayback else { throw AudioServiceFailure.busy }
            activePlaybackIdentity = identity
            activePlaybackOwner = owner
            activeCloudOperations[owner] = cloud
            return IOSCloudSpeechStream(
                cloud: cloud,
                maximumBytes: configuration.maxPayloadBytes ?? maximumPayloadBytes ?? maxAudioPayloadBytes(),
                playback: pcmPlayback
            ) { [weak self] in
                self?.activeCloudOperations.removeValue(forKey: owner)
                self?.clearPlaybackIfCurrent(identity)
                self?.publishCapabilitySnapshot()
            }
        }
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

    private static func cloudSnapshot(_ snapshot: AudioConfigurationSnapshot, route: AudioRouteResolution?, language: String?, rate: Double) -> AudioConfigurationSnapshot {
        var configuration = snapshot.configuration
        if let effective = route?.effective, effective.source == .provider, let profileID = effective.profileId {
            configuration.speech.source = .provider
            configuration.speech.cloud = AudioCloudBinding(binding: "explicit_profile", profileId: profileID, modelId: effective.modelId)
            configuration.speech.voice = nil
            if let voice = effective.voiceId {
                configuration.speech.voice = AudioVoiceSelection(source: .provider, id: voice, modelId: effective.modelId, profileId: profileID)
            }
        }
        if let language { configuration.language = language }
        configuration.rate = rate
        return AudioConfigurationSnapshot(configuration: configuration, revision: snapshot.revision)
    }

    private func speakCloud(_ request: VoiceSpeechRequest, snapshot: AudioConfigurationSnapshot) async throws -> VoiceSpeechPlaybackOutcome {
        let owner = IOSAudioOwner.ui(instanceID: "voice-speech")
        guard activePlaybackOwner == nil, !speechImplementation.hasActivePlayback else { throw AudioServiceFailure.busy }
        let cloud = try await providerService.pin(
            kind: .speech, snapshot: snapshot,
            voice: snapshot.configuration.speech.voice?.source == .provider ? snapshot.configuration.speech.voice?.id : nil
        )
        let identity = Self.newIdentity(epoch: serviceEpoch, generation: 1)
        activePlaybackIdentity = identity
        activePlaybackOwner = owner
        activeCloudOperations[owner] = cloud
        defer {
            activeCloudOperations.removeValue(forKey: owner)
            clearPlaybackIfCurrent(identity)
            publishCapabilitySnapshot()
        }
        let output = try await providerService.synthesize(cloud, text: request.text, maximumBytes: request.maxPayloadBytes ?? maximumPayloadBytes ?? maxAudioPayloadBytes())
        try Task.checkCancellation()
        _ = try await pcmPlayback.play(pcm: output.pcm, sampleRateHz: output.sampleRateHz, maximumBytes: request.maxPayloadBytes ?? maximumPayloadBytes ?? maxAudioPayloadBytes())
        return .completed
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
            if request.routePolicy == .hostConfiguredLocalOnly {
                let snapshot = pinnedSnapshot ?? configurationStore.snapshot
                guard !routeResolvesToProvider(request.operation, snapshot: snapshot) else {
                    throw AudioServiceFailure.invalidRequest
                }
            }
            switch request.operation {
            case let .startRecording(sampleRateHz, format):
                try registerRecordingAttempt(request)
                let handle = try await recorder.startRecordingOwned(
                    operationID: request.identity.recordingDriverKey,
                    ownerID: request.ownerKey,
                    sampleRateHz: sampleRateHz,
                    format: format,
                    maximumBytes: request.maxPayloadBytes
                )
                recordings[handle] = RecordingOwner(owner: request.owner, startIdentity: request.identity)
                return .recordingStarted(handle: handle)

            case let .capture(sampleRateHz, format):
                let recording = try await captureOwned(request, sampleRateHz: sampleRateHz, format: format, automaticEndpointAfterSilence: automaticEndpointAfterSilence)
                return .recording(data: recording.audioBytes, mimeType: recording.mimeType)

            case let .play(pcm, sampleRateHz):
                guard activePlaybackOwner == nil, !speechImplementation.hasActivePlayback else {
                    throw AudioServiceFailure.busy
                }
                activePlaybackIdentity = request.identity
                activePlaybackOwner = request.owner
                let duration = try await pcmPlayback.play(
                    pcm: pcm, sampleRateHz: sampleRateHz, maximumBytes: request.maxPayloadBytes
                )
                clearPlaybackIfCurrent(request.identity)
                return .playbackCompleted(durationMs: duration)

            case let .stopRecording(handle):
                guard recordings[handle]?.owner == request.owner else { throw AudioServiceFailure.notRecording }
                do {
                    let recording = try await recorder.stopRecordingOwned(
                        handle: handle,
                        ownerID: request.ownerKey,
                        maximumBytes: request.maxPayloadBytes
                    )
                    if let origin = recordings.removeValue(forKey: handle)?.startIdentity {
                        recordingAttempts.removeValue(forKey: origin)
                    }
                    return .recording(data: recording.audioBytes, mimeType: recording.mimeType)
                } catch let closed as AudioRecordingClosedError {
                    if let origin = recordings.removeValue(forKey: handle)?.startIdentity {
                        recordingAttempts.removeValue(forKey: origin)
                    }
                    throw closed.underlying
                } catch {
                    throw error
                }

            case let .transcribe(audio, mimeType, language):
                guard !audio.isEmpty, UInt64(audio.count) <= request.maxPayloadBytes else { throw AudioServiceFailure.invalidRequest }
                var snapshot = pinnedSnapshot ?? configurationStore.snapshot
                guard snapshot.configuration.recognition.source == .provider else { throw AudioServiceFailure.unsupported }
                if let language { var configuration = snapshot.configuration; configuration.language = language; snapshot = AudioConfigurationSnapshot(configuration: configuration, revision: snapshot.revision) }
                let cloud = try await providerService.pin(kind: .recognition, snapshot: snapshot, owner: request.owner, operationID: request.identity.id, maximumBytes: request.maxPayloadBytes, timeoutBudgetMs: request.timeoutBudgetMs)
                try Task.checkCancellation()
                cloudCallbacks[request.identity] = cloud
                recordCloudRoute(cloud, snapshot: snapshot, kind: .recognition, identity: request.identity)
                defer { cloudCallbacks.removeValue(forKey: request.identity) }
                let transcript = try await providerService.transcribe(
                    cloud,
                    recording: IOSAudioRecording(audioBytes: audio, mimeType: mimeType),
                    cancellationManagedExternally: true
                )
                return .transcript(text: transcript.text, language: transcript.language ?? language, confidence: transcript.confidence)

            case let .listen(language):
                guard activeListenIdentity == nil else { throw AudioServiceFailure.busy }
                let snapshot = pinnedSnapshot ?? configurationStore.snapshot
                if snapshot.configuration.recognition.source == .provider {
                    activeListenIdentity = request.identity
                    activeListenOwner = request.owner
                    defer {
                        cloudCallbacks.removeValue(forKey: request.identity)
                        if activeListenIdentity == request.identity { activeListenIdentity = nil; activeListenOwner = nil }
                    }
                    var configuration = snapshot.configuration
                    if let language { configuration.language = language }
                    let cloudSnapshot = AudioConfigurationSnapshot(configuration: configuration, revision: snapshot.revision)
                    let cloud = try await providerService.pin(kind: .recognition, snapshot: cloudSnapshot, owner: request.owner, operationID: request.identity.id, maximumBytes: request.maxPayloadBytes, timeoutBudgetMs: request.timeoutBudgetMs)
                    try Task.checkCancellation()
                    cloudCallbacks[request.identity] = cloud
                    recordCloudRoute(cloud, snapshot: cloudSnapshot, kind: .recognition, identity: request.identity)
                    let captureRequest = IOSAudioOperationRequest(identity: request.identity, owner: request.owner, initiator: request.initiator,
                        timeoutBudgetMs: request.timeoutBudgetMs.map { max(1, $0 / 2) }, maxPayloadBytes: request.maxPayloadBytes,
                        operation: .capture(sampleRateHz: 24_000, format: "m4a"))
                    let recording = try await captureOwned(captureRequest, sampleRateHz: 24_000, format: "m4a", automaticEndpointAfterSilence: automaticEndpointAfterSilence ?? .milliseconds(1_200))
                    let transcript = try await providerService.transcribe(
                        cloud,
                        recording: recording,
                        cancellationManagedExternally: true
                    )
                    return .transcript(text: transcript.text, language: transcript.language ?? language, confidence: transcript.confidence)
                }
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
                if voice == "default" || voice == "auto" { configuration.speech.voice = nil }
                if configuration.speech.source == .provider {
                    let cloudSnapshot = AudioConfigurationSnapshot(configuration: configuration, revision: snapshot.revision)
                    let selectedVoice = voice.flatMap { $0 == "auto" || $0 == "default" ? nil : $0 } ?? configuration.speech.voice?.id
                    let cloud = try await providerService.pin(kind: .speech, snapshot: cloudSnapshot, voice: selectedVoice, owner: request.owner, operationID: request.identity.id, maximumBytes: request.maxPayloadBytes, timeoutBudgetMs: request.timeoutBudgetMs)
                    try Task.checkCancellation()
                    cloudCallbacks[request.identity] = cloud
                    recordCloudRoute(cloud, snapshot: cloudSnapshot, kind: .speech, identity: request.identity)
                    defer { cloudCallbacks.removeValue(forKey: request.identity) }
                    let output = try await providerService.synthesize(
                        cloud,
                        text: text,
                        maximumBytes: request.maxPayloadBytes,
                        cancellationManagedExternally: true
                    )
                    return .synthesized(pcm: output.pcm, sampleRateHz: output.sampleRateHz)
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
                if voice == "default" || voice == "auto" { configuration.speech.voice = nil }
                if configuration.speech.source == .provider {
                    activePlaybackIdentity = request.identity
                    activePlaybackOwner = request.owner
                    defer { cloudCallbacks.removeValue(forKey: request.identity); clearPlaybackIfCurrent(request.identity) }
                    let cloudSnapshot = AudioConfigurationSnapshot(configuration: configuration, revision: snapshot.revision)
                    let selectedVoice = voice.flatMap { $0 == "auto" || $0 == "default" ? nil : $0 } ?? configuration.speech.voice?.id
                    let cloud = try await providerService.pin(kind: .speech, snapshot: cloudSnapshot, voice: selectedVoice, owner: request.owner, operationID: request.identity.id, maximumBytes: request.maxPayloadBytes, timeoutBudgetMs: request.timeoutBudgetMs)
                    try Task.checkCancellation()
                    cloudCallbacks[request.identity] = cloud
                    recordCloudRoute(cloud, snapshot: cloudSnapshot, kind: .speech, identity: request.identity)
                    let output = try await providerService.synthesize(
                        cloud,
                        text: text,
                        maximumBytes: request.maxPayloadBytes,
                        cancellationManagedExternally: true
                    )
                    try Task.checkCancellation()
                    let duration = try await pcmPlayback.play(pcm: output.pcm, sampleRateHz: output.sampleRateHz, maximumBytes: request.maxPayloadBytes)
                    return .playbackCompleted(durationMs: duration)
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
                    voiceIdentifier = effective?.voiceId ?? ""
                case .offline:
                    voiceIdentifier = effective?.voiceId ?? ""
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
                if let error = await endOwner(request.owner, excluding: request.identity) {
                    throw AudioServiceFailure.nativeFailure(error.message)
                }
                return .ownerEnded
            }
        } catch {
            if recordingAttempts[request.identity] != nil {
                quarantineCancellationOwner(request.owner, identity: request.identity)
            }
            if activeListenIdentity == request.identity {
                activeListenIdentity = nil
                activeListenOwner = nil
            }
            clearPlaybackIfCurrent(request.identity)
            return .failed(Self.audioError(error))
        }
    }

    private func captureOwned(_ request: IOSAudioOperationRequest, sampleRateHz: UInt32, format: String, automaticEndpointAfterSilence: Duration?) async throws -> IOSAudioRecording {
                try registerRecordingAttempt(request)
                let handle = try await recorder.startRecordingOwned(
                    operationID: request.identity.recordingDriverKey,
                    ownerID: request.ownerKey,
                    sampleRateHz: sampleRateHz,
                    format: format,
                    maximumBytes: request.maxPayloadBytes
                )
                recordings[handle] = RecordingOwner(owner: request.owner, startIdentity: request.identity)
                defer {
                    recordings.removeValue(forKey: handle)
                    finishedCaptures.remove(request.identity)
                }
                let durationMs = Self.captureDurationMs(
                    timeoutBudgetMs: request.timeoutBudgetMs,
                    maximumBytes: request.maxPayloadBytes
                )
                let deadline = ContinuousClock.now.advanced(by: .milliseconds(durationMs))
                var lastSpeech: ContinuousClock.Instant?
                do {
                    while recorder.isRecordingOwned(handle: handle, ownerID: request.ownerKey),
                          !finishedCaptures.contains(request.identity), ContinuousClock.now < deadline {
                        if let silence = automaticEndpointAfterSilence {
                            if let level = recorder.recordingLevel(handle: handle, ownerID: request.ownerKey), level > -35 {
                                lastSpeech = .now
                            }
                            if let lastSpeech, lastSpeech.duration(to: .now) >= silence { break }
                        }
                        try await Task.sleep(for: .milliseconds(40))
                    }
                    try Task.checkCancellation()
                    let recording = try await recorder.stopRecordingOwned(
                        handle: handle, ownerID: request.ownerKey, maximumBytes: request.maxPayloadBytes
                    )
                    recordingAttempts.removeValue(forKey: request.identity)
                    return recording
                } catch let closed as AudioRecordingClosedError {
                    recordingAttempts.removeValue(forKey: request.identity)
                    throw closed.underlying
                } catch {
                    await recorder.cancel(startOperationID: request.identity.recordingDriverKey)
                    throw error
                }

    }

    private func registerRecordingAttempt(_ request: IOSAudioOperationRequest) throws {
        guard recordingAttempts[request.identity] == nil, recordingAttempts.count < 1_024 else {
            throw AudioServiceFailure.busy
        }
        recordingAttempts[request.identity] = request.owner
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
        case .recordingStarted:
            // The caller performing cancellation joins this operation task
            // before it accepts the driver's receipt and retires the claim.
            _ = await recorder.cancelWithReceipt(startOperationID: identity.recordingDriverKey)
        case .playbackCompleted:
            if activePlaybackIdentity == identity { await speechImplementation.stopAndWait() }
        default:
            break
        }
    }

    func handleInvalidation(_ event: VoiceAudioSessionCoordinator.Invalidation) async {
        guard await coordinator.owns(event.lease) else { return }
        let recordingOrigins = Set(recordingAttempts.keys.filter {
            recorder.ownsRecordingLease(event.lease, startOperationID: $0.recordingDriverKey)
        })
        let affected = pending.values.compactMap { operation -> IOSAudioOperationIdentity? in
            switch (event.lease.purpose, operation.request.operation) {
            case (.recording, .startRecording), (.recording, .capture):
                return recordingOrigins.contains(operation.request.identity) ? operation.request.identity : nil
            case let (.recording, .stopRecording(handle)):
                guard let origin = recordings[handle]?.startIdentity, recordingOrigins.contains(origin) else { return nil }
                return operation.request.identity
            case (.recognition, .listen), (.flowDuplex, .listen),
                 (.playback, .speak), (.playback, .play):
                return operation.request.identity
            case (.playback, .synthesize):
                return activeSystemRenderIdentity == operation.request.identity
                    ? operation.request.identity : nil
            default:
                return nil
            }
        }
        let oldListenIdentity = activeListenIdentity
        let oldPlaybackIdentity = activePlaybackIdentity
        let oldSystemRenderIdentity = activeSystemRenderIdentity
        for identity in affected { await cancel(identity) }
        for identity in recordingOrigins where !affected.contains(identity) { await cancel(identity) }
        if await coordinator.owns(event.lease) {
            switch event.lease.purpose {
            case .recording:
                break // Only the operation identities bound to this lease may be retired.
            case .recognition:
                stt.cancelRecognition()
            case .playback:
                if let oldPlaybackIdentity {
                    let stopTask = requestPlaybackStop(oldPlaybackIdentity)
                    await stopTask.value
                } else {
                    pcmPlayback.stop()
                    await speechImplementation.stopAndWait()
                }
            case .flowDuplex:
                stt.cancelRecognition()
                await bargeInImplementation.stopAll()
            }
        }
        if (event.lease.purpose == .recognition || event.lease.purpose == .flowDuplex),
           activeListenIdentity == oldListenIdentity {
            activeListenIdentity = nil
            activeListenOwner = nil
        }
        if event.lease.purpose == .playback, let oldPlaybackIdentity {
            clearPlaybackIfCurrent(oldPlaybackIdentity)
        }
        if activeSystemRenderIdentity == oldSystemRenderIdentity,
           event.lease.purpose == .playback {
            activeSystemRenderIdentity = nil
        }
        publishCapabilitySnapshot()
        await onInvalidation?(event)
    }

    private func requestPlaybackStop(_ identity: IOSAudioOperationIdentity) -> Task<Void, Never> {
        pcmPlayback.stop()
        let implementation = speechImplementation
        return playbackStopReceipts.begin(for: identity) { [weak self, implementation] in
            await implementation.stopAndWait()
            guard let self else { return }
            self.clearPlaybackIfCurrent(identity, afterStopReceipt: true)
            self.finishStandaloneDiagnostic(identity, phase: "cancelled")
            self.publishCapabilitySnapshot()
        }
    }

    private func clearPlaybackIfCurrent(
        _ identity: IOSAudioOperationIdentity,
        afterStopReceipt: Bool = false
    ) {
        guard activePlaybackIdentity == identity else { return }
        guard afterStopReceipt || !playbackStopReceipts.contains(identity) else { return }
        activePlaybackIdentity = nil
        activePlaybackOwner = nil
    }

    private func recordCloudRoute(_ cloud: IOSAudioProviderService.PinnedOperation, snapshot: AudioConfigurationSnapshot, kind: AudioProviderKind, identity: IOSAudioOperationIdentity) {
        let voice = kind == .speech ? snapshot.configuration.speech.voice : nil
        recordRouteResolution(AudioRouteResolution(
            requested: .init(source: .provider, offlineModelId: nil, voice: voice),
            effective: .init(source: .provider, modelId: cloud.modelID, voiceId: voice?.id, profileId: cloud.profileID),
            status: .ready, reason: "ready", fallbackReason: nil
        ), for: identity, phase: "running")
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
        pruneCancellationOutcomes()
        guard !seenIdentities.contains(identity),
              cancellationOutcomes[identity] == nil,
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
        case .capture: "capture"
        case .transcribe: "transcribe"
        case .play: "play"
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
        case .listen, .transcribe: snapshot.configuration.recognition.source
        case .synthesize, .speak: snapshot.configuration.speech.source
        case .startRecording, .stopRecording, .capture, .play, .status, .endOwner: nil
        }
    }

    private func routeResolvesToProvider(
        _ operation: IOSAudioOperation,
        snapshot: AudioConfigurationSnapshot
    ) -> Bool {
        let route: AudioRouteResolution
        switch operation {
        case let .listen(language):
            route = AudioConfigurationRuntime.route(
                kind: .recognition,
                snapshot: snapshot,
                languageOverride: language
            )
        case let .transcribe(_, _, language):
            route = AudioConfigurationRuntime.route(
                kind: .recognition,
                snapshot: snapshot,
                languageOverride: language
            )
        case let .synthesize(_, language, _, voice), let .speak(_, language, _, voice):
            route = AudioConfigurationRuntime.route(
                kind: .speech,
                snapshot: snapshot,
                languageOverride: language,
                voiceOverride: voice
            )
        case .startRecording, .stopRecording, .capture, .play, .status, .endOwner:
            return false
        }
        return route.requested.source == .provider
            || route.requested.voice?.source == .provider
            || route.effective?.source == .provider
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

    static func captureDurationMs(timeoutBudgetMs: UInt64?, maximumBytes: UInt64) -> Int64 {
        // AAC at 32 kbit/s. Leave headroom for the container and completion.
        let payloadMs = min(UInt64(60_000), maximumBytes / 4 * 9 / 10)
        let timeoutMs = timeoutBudgetMs.map { $0 > 200 ? $0 - 200 : $0 / 2 } ?? 60_000
        return Int64(max(1, min(payloadMs, timeoutMs)))
    }

    var playbackPositionMs: UInt64 { pcmPlayback.positionMs }

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
private final class CancellationCompletionWaiter {
    private var continuation: CheckedContinuation<Bool, Never>?
    private var observer: Task<Void, Never>?
    private var timer: Task<Void, Never>?
    private var resolved = false

    init(_ continuation: CheckedContinuation<Bool, Never>) {
        self.continuation = continuation
    }

    func install(observer: Task<Void, Never>, timer: Task<Void, Never>) {
        guard !resolved else {
            observer.cancel()
            timer.cancel()
            return
        }
        self.observer = observer
        self.timer = timer
    }

    func resolve(_ completed: Bool) {
        guard !resolved, let continuation else { return }
        resolved = true
        self.continuation = nil
        observer?.cancel()
        timer?.cancel()
        observer = nil
        timer = nil
        continuation.resume(returning: completed)
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
