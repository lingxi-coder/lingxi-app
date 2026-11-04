import Foundation
import Observation

enum IOSRealtimeAudioPhase: Equatable { case idle, connecting, listening, thinking, speaking, interrupted, failed }

struct IOSRealtimeTurnOwnership {
    private(set) var agentTurnInFlight = false
    private(set) var hasUnconfirmedToolWork = false
    private var agentTurnID: String?
    private var observedAgentTurnIDs = Set<String>()

    var hasPendingWork: Bool { agentTurnInFlight || hasUnconfirmedToolWork }

    mutating func beginInput() { agentTurnInFlight = true; agentTurnID = nil }
    mutating func markActive() { agentTurnInFlight = true }
    mutating func beginAgentTurn(turnID: String?) {
        agentTurnInFlight = true
        guard let turnID, !turnID.isEmpty, turnID.utf8.count <= 256 else { agentTurnID = nil; return }
        if agentTurnID == turnID { return }
        guard observedAgentTurnIDs.count < 4_096,
              observedAgentTurnIDs.insert(turnID).inserted else { agentTurnID = nil; return }
        agentTurnID = turnID
    }
    mutating func beginToolWork() { hasUnconfirmedToolWork = true; agentTurnInFlight = true }

    /// Only the native Agent's correlated terminal can release an input. A
    /// provider response may finish while a tool or continuation is pending.
    mutating func completeAgentTurn(turnID: String?) -> Bool {
        guard agentTurnInFlight, let turnID, !turnID.isEmpty,
              turnID == agentTurnID else { return false }
        agentTurnID = nil
        agentTurnInFlight = false
        hasUnconfirmedToolWork = false
        return true
    }
    mutating func resetAfterSessionEnd() { self = Self() }
}

enum IOSRealtimeAudioDrainPolicy {
    static func canClose(turnInFlight: Bool,
                         hasUnconfirmedToolWork: Bool,
                         pendingPlayback: Bool, pendingAcknowledgements: Bool) -> Bool {
        !turnInFlight && !hasUnconfirmedToolWork && !pendingPlayback && !pendingAcknowledgements
    }
}

@MainActor
protocol IOSRealtimeAudioDeviceDriving: AnyObject {
    var onPlaybackCompleted: ((String?) -> Void)? { get set }
    var hasPendingOutput: Bool { get }
    var currentPlaybackItemID: String? { get }
    func owns(_ event: VoiceAudioSessionCoordinator.Invalidation) async -> Bool
    func start(inputSampleRateHz: UInt32, outputSampleRateHz: UInt32) async throws -> AsyncThrowingStream<Data, Error>
    func stop() async -> Bool
    func enqueueOutput(pcm: Data, sampleRateHz: UInt32, itemID: String?) throws
    func markAllOutputCompleted()
    @discardableResult func interruptPlayback(itemID: String?) -> UInt64
}

extension IOSRealtimeAudioDevice: IOSRealtimeAudioDeviceDriving {}

/// Connects device media to the current Harness Agent driver. This service
/// never creates a second chat engine, executes a tool, or handles credentials.
@Observable
@MainActor
final class IOSRealtimeAudioService {
    static let shared = IOSRealtimeAudioService()
    private let device: any IOSRealtimeAudioDeviceDriving
    private let openOverride: (@MainActor (MobileEngineHandle, IOSAudioOwner, AudioConfigurationSnapshot) async throws -> IosRealtimeAudioSession)?
    private var engine: MobileEngineHandle?
    private var engineOwnerID: String?
    private var pendingEngineBinding: (engine: MobileEngineHandle, ownerID: String)?
    private var startupOwner: IOSAudioOwner?
    private var detachWhenStopped = false
    private var closeAfterCurrentTurn = false
    /// Provider response completion does not prove that an Agent tool or its
    /// continuation has finished. Keep this conservative until the native
    /// Agent supplies an authoritative per-turn completion signal.
    private var turnOwnership = IOSRealtimeTurnOwnership()
    private var session: IosRealtimeAudioSession?
    @ObservationIgnored private var eventTask: Task<Void, Never>?
    private var listener: IOSRealtimeAudioListenerAdapter?
    private var generation: UInt64 = 0
    @ObservationIgnored private var captureTask: Task<Void, Never>?
    @ObservationIgnored private var deviceStartTask: Task<Void, Never>?
    @ObservationIgnored private var openingTask: Task<Void, Error>?
    @ObservationIgnored private var playbackAcknowledgements: [UUID: Task<Void, Never>] = [:]
    @ObservationIgnored private var commands: Set<UUID> = []
    @ObservationIgnored private var commandDrainWaiters: [CheckedContinuation<Void, Never>] = []
    @ObservationIgnored private var stopFlight: (id: UUID, task: Task<Bool, Never>)?
    @ObservationIgnored private var stopWaiters: [UUID: CheckedContinuation<Bool, Never>] = [:]
    @ObservationIgnored private var stopTimeouts: [UUID: Task<Void, Never>] = [:]
    private let stopTimeout: Duration
    private var stopUnconfirmed = false
    private var retiringSession: IosRealtimeAudioSession?
    private var inputEnabled = false
    private var supportsTruncation = false
    private var pendingPlaybackAcknowledgements = 0
    @ObservationIgnored private var invalidationTask: Task<Void, Never>?
    private(set) var phase: IOSRealtimeAudioPhase = .idle
    private(set) var caption = ""
    private(set) var errorMessage: String?
    var onStateChange: ((IOSRealtimeAudioPhase, String, String?) -> Void)?

    init(
        device: (any IOSRealtimeAudioDeviceDriving)? = nil,
        stopTimeout: Duration = .seconds(2),
        observeInvalidation: Bool = true,
        openOverride: (@MainActor (MobileEngineHandle, IOSAudioOwner, AudioConfigurationSnapshot) async throws -> IosRealtimeAudioSession)? = nil
    ) {
        self.device = device ?? IOSRealtimeAudioDevice()
        self.stopTimeout = stopTimeout
        self.openOverride = openOverride
        if observeInvalidation { invalidationTask = Task { @MainActor [weak self] in
            let events = await VoiceAudioSessionCoordinator.shared.invalidationEvents()
            for await event in events {
                guard let self else { return }
                let generation = self.generation
                if await self.device.owns(event), self.generation == generation {
                    await self.stop(abort: true)
                }
            }
        } }
        self.device.onPlaybackCompleted = { [weak self] itemID in
            guard let self, let session = self.session else { return }
            let generation = self.generation
            self.pendingPlaybackAcknowledgements += 1
            let acknowledgementID = UUID()
            let task = Task { @MainActor [weak self] in
                defer { self?.playbackAcknowledgements.removeValue(forKey: acknowledgementID) }
                do { try await session.playbackCompleted(itemId: itemID) }
                catch {
                    guard let self, generation == self.generation else { return }
                    self.scheduleFailure(error.localizedDescription, generation: generation)
                    return
                }
                guard let self, generation == self.generation else { return }
                self.pendingPlaybackAcknowledgements -= 1
                if self.pendingPlaybackAcknowledgements == 0, !self.device.hasPendingOutput {
                    if self.closeAfterCurrentTurn {
                        self.closeAfterDrainIfReady()
                    } else if self.turnOwnership.hasPendingWork {
                        self.inputEnabled = false
                        self.update(.thinking)
                    } else {
                        self.inputEnabled = true
                        self.update(.listening)
                    }
                }
            }
            self.playbackAcknowledgements[acknowledgementID] = task
        }
    }

    deinit { eventTask?.cancel(); invalidationTask?.cancel(); captureTask?.cancel(); deviceStartTask?.cancel() }

    var isAttached: Bool { engine != nil }
    var isActive: Bool { startupOwner != nil || session != nil || phase == .connecting || stopUnconfirmed || stopFlight != nil }
    func isActive(ownerID: String) -> Bool { engineOwnerID == ownerID && isActive }

    func attach(engine: MobileEngineHandle, ownerID: String) {
        if isActive {
            // A user may navigate back to this owner while its old turn is
            // draining. Keep the latest visible binding for after teardown.
            pendingEngineBinding = (engine, ownerID)
            return
        }
        pendingEngineBinding = nil
        self.engine = engine
        engineOwnerID = ownerID
    }

    /// Reserve the exact selected engine before the asynchronous start can be
    /// scheduled. Deselecting it can then only request a drain, not replace it.
    func reserveStart(owner: IOSAudioOwner?) -> Bool {
        guard !isActive, let owner, let engine, let engineOwnerID,
              IOSAudioProviderService.shared.ownsSessionRoute(owner, ownerID: engineOwnerID, engine: engine) else { return false }
        startupOwner = owner
        closeAfterCurrentTurn = false
        return true
    }

    func restoreReservedStart(owner: IOSAudioOwner?) {
        guard let owner, startupOwner == owner else { return }
        closeAfterCurrentTurn = false
        detachWhenStopped = false
        if pendingEngineBinding?.ownerID == engineOwnerID { pendingEngineBinding = nil }
    }

    func detach(ownerID: String) {
        if pendingEngineBinding?.ownerID == ownerID { pendingEngineBinding = nil }
        guard engineOwnerID == ownerID else { return }
        if isActive {
            detachWhenStopped = true
            finishCurrentTurnAndClose()
            return
        }
        engine = nil
        engineOwnerID = nil
    }

    /// Navigation stops accepting new audio, but an already submitted realtime
    /// turn keeps its engine, callbacks and playback until the terminal event.
    func finishCurrentTurnAndClose() {
        guard isActive else { return }
        closeAfterCurrentTurn = true
        inputEnabled = false
        closeAfterDrainIfReady()
    }

    private func closeAfterDrainIfReady() {
        guard canCloseAfterDrain else { return }
        let scheduledGeneration = generation
        Task { @MainActor [weak self] in
            guard let self, self.generation == scheduledGeneration, self.canCloseAfterDrain else { return }
            _ = await self.stop(abort: false)
        }
    }

    private var canCloseAfterDrain: Bool {
        closeAfterCurrentTurn && startupOwner == nil && IOSRealtimeAudioDrainPolicy.canClose(
            turnInFlight: turnOwnership.agentTurnInFlight,
            hasUnconfirmedToolWork: turnOwnership.hasUnconfirmedToolWork,
            pendingPlayback: device.hasPendingOutput,
            pendingAcknowledgements: pendingPlaybackAcknowledgements != 0
        )
    }

    func start(snapshot: AudioConfigurationSnapshot) async throws {
        guard let engine, let owner = startupOwner else { throw AudioServiceFailure.nativeFailure(String(localized: "audio_realtime_unavailable")) }
        guard session == nil, phase != .connecting, !stopUnconfirmed, stopFlight == nil else { throw AudioServiceFailure.busy }
        guard IOSAudioProviderService.shared.sessionContext(for: owner) != nil else { throw AudioServiceFailure.unavailable }
        generation &+= 1
        let operation = generation
        guard snapshot.configuration.conversation.interaction == "turn_based" else { throw AudioServiceFailure.unsupported }
        turnOwnership.resetAfterSessionEnd()
        caption = ""
        errorMessage = nil
        update(.connecting)
        let open = Task { @MainActor [self] in
            if let openOverride {
                let opened = try await openOverride(engine, owner, snapshot)
                guard generation == operation, self.engine === engine, !Task.isCancelled else {
                    await opened.abort()
                    throw CancellationError()
                }
                session = opened
                return
            }
            let pinned = try await IOSAudioProviderService.shared.pinRealtime(snapshot: snapshot, owner: owner)
            try Task.checkCancellation()
            guard generation == operation, self.engine === engine else { throw CancellationError() }
            let callback = IOSRealtimeAudioListenerAdapter()
            listener = callback
            eventTask = Task { @MainActor [weak self] in
                do {
                    for try await event in callback.events {
                        guard let self, self.generation == operation, !Task.isCancelled else { return }
                        await self.receive(event, generation: operation, owner: pinned.owner, requestJSON: pinned.requestJSON)
                    }
                } catch is CancellationError {
                } catch {
                    guard let self, self.generation == operation else { return }
                    await self.fail(error.localizedDescription, generation: operation)
                }
            }
            let opened = try await startIosRealtimeAudio(engine: engine, providerHost: pinned.host, requestJson: pinned.requestJSON, listener: callback)
            guard generation == operation, self.engine === engine, !Task.isCancelled else {
                await opened.abort()
                throw CancellationError()
            }
            session = opened
        }
        openingTask = open
        do {
            try await withTaskCancellationHandler {
                try await open.value
            } onCancel: {
                open.cancel()
            }
            if generation == operation {
                openingTask = nil
                startupOwner = nil
                if closeAfterCurrentTurn { closeAfterDrainIfReady() }
            }
        } catch {
            if generation == operation { openingTask = nil }
            if generation == operation { await fail(error.localizedDescription, generation: operation) }
            throw error
        }
    }

    /// Admit the user action before scheduling the native call so navigation
    /// cannot close the owner between the tap and the first FFI suspension.
    func commitInput() {
        guard let session, phase == .listening, !closeAfterCurrentTurn,
              !turnOwnership.hasPendingWork else { return }
        let operation = generation
        let command = beginCommand()
        turnOwnership.beginInput()
        inputEnabled = false
        update(.thinking)
        Task { @MainActor [self, session] in
            defer { endCommand(command) }
            do { try await session.commitInput() }
            catch { scheduleFailure(error.localizedDescription, generation: operation) }
        }
    }

    func interrupt() async {
        guard let session else { return }
        let operation = generation
        guard supportsTruncation else {
            // The Agent must never keep audio history claiming unplayed media.
            await stop(abort: true)
            return
        }
        let command = beginCommand()
        defer { endCommand(command) }
        let itemID = device.currentPlaybackItemID
        let position = device.interruptPlayback(itemID: itemID)
        inputEnabled = false
        update(.interrupted)
        do {
            try await session.interrupt(itemId: itemID, audioEndMs: itemID == nil ? nil : UInt32(clamping: position))
            guard generation == operation else { return }
            inputEnabled = !turnOwnership.hasPendingWork && !closeAfterCurrentTurn
            update(turnOwnership.hasPendingWork ? .thinking : .listening)
        } catch { scheduleFailure(error.localizedDescription, generation: operation) }
    }

    @discardableResult
    func stop(abort: Bool) async -> Bool {
        if let flight = stopFlight { return await waitForStop(flight.id) }
        generation &+= 1
        startupOwner = nil
        let old = retiringSession ?? session
        session = nil
        retiringSession = old
        listener?.finish()
        listener = nil
        eventTask?.cancel(); eventTask = nil
        let opening = openingTask
        let capture = captureTask
        let starter = deviceStartTask
        let acknowledgements = Array(playbackAcknowledgements.values)
        opening?.cancel(); capture?.cancel(); starter?.cancel()
        acknowledgements.forEach { $0.cancel() }
        openingTask = nil; captureTask = nil; deviceStartTask = nil
        inputEnabled = false
        pendingPlaybackAcknowledgements = 0
        stopUnconfirmed = true
        let id = UUID()
        let task = Task { @MainActor [self, device] in
            if let opening { _ = try? await opening.value }
            if let starter { await starter.value }
            if let capture { await capture.value }
            for acknowledgement in acknowledgements { await acknowledgement.value }
            await waitForCommands()
            let deviceStopped = await device.stop()
            guard deviceStopped else { return false }
            if let old {
                if abort { await old.abort() }
                else {
                    do { try await old.close() }
                    catch { return false }
                }
            }
            return true
        }
        stopFlight = (id, task)
        Task { @MainActor [weak self] in
            let result = await task.value
            self?.finishStop(id, result: result)
        }
        return await waitForStop(id)
    }

    private func waitForStop(_ id: UUID) async -> Bool {
        guard stopFlight?.id == id else { return !stopUnconfirmed }
        guard stopWaiters.count < 64 else { return false }
        let waiterID = UUID()
        return await withCheckedContinuation { continuation in
            stopWaiters[waiterID] = continuation
            stopTimeouts[waiterID] = Task { @MainActor [weak self, stopTimeout] in
                do { try await Task.sleep(for: stopTimeout) } catch { return }
                guard let self, self.stopFlight?.id == id else { return }
                self.stopTimeouts.removeValue(forKey: waiterID)
                self.stopWaiters.removeValue(forKey: waiterID)?.resume(returning: false)
                self.errorMessage = "Realtime audio stop is not yet confirmed."
                self.update(.failed)
            }
        }
    }

    private func finishStop(_ id: UUID, result: Bool) {
        guard stopFlight?.id == id else { return }
        stopFlight = nil
        stopUnconfirmed = !result
        if result {
            retiringSession = nil
            errorMessage = nil
            closeAfterCurrentTurn = false
            turnOwnership.resetAfterSessionEnd()
            if let pendingEngineBinding {
                engine = pendingEngineBinding.engine
                engineOwnerID = pendingEngineBinding.ownerID
                self.pendingEngineBinding = nil
            } else if detachWhenStopped {
                engine = nil
                engineOwnerID = nil
            }
            detachWhenStopped = false
            update(.idle)
        } else {
            errorMessage = "Realtime audio stop could not be confirmed."
            update(.failed)
        }
        let waiters = stopWaiters.values
        stopWaiters.removeAll()
        stopTimeouts.values.forEach { $0.cancel() }
        stopTimeouts.removeAll()
        waiters.forEach { $0.resume(returning: result) }
    }

    private func beginCommand() -> UUID {
        let id = UUID()
        commands.insert(id)
        return id
    }

    private func endCommand(_ id: UUID) {
        commands.remove(id)
        guard commands.isEmpty else { return }
        let waiters = commandDrainWaiters
        commandDrainWaiters.removeAll()
        waiters.forEach { $0.resume() }
    }

    private func waitForCommands() async {
        guard !commands.isEmpty else { return }
        await withCheckedContinuation { commandDrainWaiters.append($0) }
    }

    private func receive(_ json: String, generation expectedGeneration: UInt64, owner: IOSAudioOwner?, requestJSON: String) async {
        guard generation == expectedGeneration else { return }
        guard let data = json.data(using: .utf8),
              let event = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let type = event["type"] as? String else {
            await fail("The realtime audio host returned an invalid event.", generation: expectedGeneration)
            return
        }
        switch type {
        case "session_ready":
            IOSAudioProviderService.shared.bindSelectedAccount(event, owner: owner, requestJSON: requestJSON)
            // OpenAI reports both session.created and session.updated as ready.
            // They describe one device session, including while start is pending.
            guard deviceStartTask == nil else { return }
            guard let input = event["inputFormat"] as? [String: Any], let output = event["outputFormat"] as? [String: Any],
                  input["encoding"] as? String == "pcm16", output["encoding"] as? String == "pcm16",
                  (input["channels"] as? NSNumber)?.intValue == 1, (output["channels"] as? NSNumber)?.intValue == 1,
                  let inputRate = (input["sampleRateHz"] as? NSNumber)?.uint32Value,
                  let outputRate = (output["sampleRateHz"] as? NSNumber)?.uint32Value else {
                await fail("The realtime provider did not negotiate supported PCM16 mono media.", generation: expectedGeneration)
                return
            }
            supportsTruncation = (event["capabilities"] as? [String: Any])?["audioTruncation"] as? Bool ?? false
            deviceStartTask = Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    let input = try await self.device.start(inputSampleRateHz: inputRate, outputSampleRateHz: outputRate)
                    guard self.generation == expectedGeneration, !Task.isCancelled else { return }
                    while self.session == nil, self.generation == expectedGeneration {
                        try await Task.sleep(for: .milliseconds(10))
                    }
                    guard self.generation == expectedGeneration, !Task.isCancelled else { return }
                    self.inputEnabled = !self.closeAfterCurrentTurn && !self.turnOwnership.hasPendingWork
                        && self.pendingPlaybackAcknowledgements == 0 && !self.device.hasPendingOutput
                    if self.inputEnabled {
                        self.update(.listening)
                    } else if self.device.hasPendingOutput {
                        self.update(.speaking)
                    } else {
                        self.update(.thinking)
                    }
                    self.captureTask = Task { @MainActor [weak self] in
                        do {
                            for try await pcm in input {
                                guard let self, self.generation == expectedGeneration, !Task.isCancelled else { return }
                                if self.inputEnabled, let session = self.session { try await session.sendAudio(pcm: pcm) }
                            }
                        } catch is CancellationError {
                        } catch {
                            guard let self, self.generation == expectedGeneration else { return }
                            self.scheduleFailure(error.localizedDescription, generation: expectedGeneration)
                        }
                    }
                } catch is CancellationError {
                } catch { self.scheduleFailure(error.localizedDescription, generation: expectedGeneration) }
            }
        case "turn_started":
            turnOwnership.markActive()
            inputEnabled = false
            update(.thinking)
        case "agent_response_started":
            turnOwnership.beginAgentTurn(turnID: event["turnId"] as? String)
            inputEnabled = false
            update(.thinking)
        case "audio_delta":
            guard let encoded = event["audioBase64"] as? String, encoded.utf8.count <= 1_400_000,
                  let pcm = Data(base64Encoded: encoded), event["encoding"] as? String == "pcm16",
                  (event["channels"] as? NSNumber)?.intValue == 1,
                  let rate = (event["sampleRateHz"] as? NSNumber)?.uint32Value else { await fail("Invalid realtime output audio.", generation: expectedGeneration); return }
            turnOwnership.markActive()
            inputEnabled = false
            do { try device.enqueueOutput(pcm: pcm, sampleRateHz: rate, itemID: event["itemId"] as? String); update(.speaking) }
            catch { await fail(error.localizedDescription, generation: expectedGeneration) }
        case "turn_completed":
            device.markAllOutputCompleted()
        case "agent_turn_completed":
            guard turnOwnership.completeAgentTurn(turnID: event["turnId"] as? String) else { return }
            if closeAfterCurrentTurn {
                closeAfterDrainIfReady()
            } else if pendingPlaybackAcknowledgements == 0, !device.hasPendingOutput {
                inputEnabled = true
                update(.listening)
            }
        case "transcript":
            caption = event["text"] as? String ?? caption
            // Input transcription can arrive after the Agent terminal. It is
            // presentation data and must not open a new owned turn.
            update(phase)
        case "interrupted":
            device.interruptPlayback(itemID: nil)
            if closeAfterCurrentTurn {
                closeAfterDrainIfReady()
            } else if turnOwnership.hasPendingWork {
                inputEnabled = false
                update(.thinking)
            } else {
                inputEnabled = true
                update(.listening)
            }
        case "tool_call":
            turnOwnership.beginToolWork()
            inputEnabled = false
            update(.thinking)
        case "tool_cancelled", "usage": break
        case "closed": await stop(abort: false)
        case "error": await fail((event["message"] as? String) ?? ((event["error"] as? [String: Any])?["message"] as? String) ?? "Realtime audio failed.", generation: expectedGeneration)
        default: break
        }
    }

    #if DEBUG
    func receiveForTesting(_ json: String, owner: IOSAudioOwner? = nil, requestJSON: String = "{}") async {
        await receive(json, generation: generation, owner: owner, requestJSON: requestJSON)
    }

    func requestCloseAfterDrainForTesting() {
        closeAfterCurrentTurn = true
        closeAfterDrainIfReady()
    }

    func installSessionForTesting(_ session: IosRealtimeAudioSession) {
        precondition(!isActive)
        self.session = session
        update(.listening)
    }

    var boundEngineForTesting: MobileEngineHandle? { engine }
    #endif

    private func fail(_ message: String, generation expectedGeneration: UInt64) async {
        guard generation == expectedGeneration else { return }
        let stoppedGeneration = stopFlight == nil ? generation &+ 1 : generation
        await stop(abort: true)
        guard generation == stoppedGeneration else { return }
        errorMessage = message
        update(.failed)
    }

    private func scheduleFailure(_ message: String, generation expectedGeneration: UInt64) {
        Task { @MainActor [weak self] in
            guard let self, self.generation == expectedGeneration else { return }
            await self.fail(message, generation: expectedGeneration)
        }
    }

    private func update(_ next: IOSRealtimeAudioPhase) {
        phase = next
        onStateChange?(next, caption, errorMessage)
    }
}

/// UniFFI invokes callbacks off-main. A bounded immutable stream queues work
/// on the service actor. Overflow terminates the owning conversation.
final class IOSRealtimeAudioListenerAdapter: IosRealtimeAudioListener, @unchecked Sendable {
    let events: AsyncThrowingStream<String, Error>
    private let continuation: AsyncThrowingStream<String, Error>.Continuation

    init() {
        let stream = AsyncThrowingStream<String, Error>.makeStream(bufferingPolicy: .bufferingNewest(16))
        events = stream.stream
        continuation = stream.continuation
    }

    func onEvent(eventJson: String) async {
        guard eventJson.utf8.count <= 1_500_000 else { continuation.finish(throwing: AudioServiceFailure.mediaTooLarge); return }
        if eventJson.contains("\"usage\"") {
            // Accounting is independent of UI cancellation; billed usage may
            // arrive after the presentation generation has been retired.
            await IOSAudioProviderService.shared.recordRealtimeUsage(eventJson)
        }
        if case .dropped = continuation.yield(eventJson) { continuation.finish(throwing: AudioServiceFailure.mediaTooLarge) }
    }

    func finish() { continuation.finish() }
}
