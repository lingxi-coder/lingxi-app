import Foundation
import Observation

enum IOSRealtimeAudioPhase: Equatable { case idle, connecting, listening, thinking, speaking, interrupted, failed }

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
    private var engine: MobileEngineHandle?
    private var engineOwnerID: String?
    private var session: IosRealtimeAudioSession?
    @ObservationIgnored private var eventTask: Task<Void, Never>?
    private var listener: IOSRealtimeAudioListenerAdapter?
    private var generation: UInt64 = 0
    @ObservationIgnored private var cleanupTask: Task<Void, Never>?
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
        observeInvalidation: Bool = true
    ) {
        self.device = device ?? IOSRealtimeAudioDevice()
        self.stopTimeout = stopTimeout
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
                    self.inputEnabled = true
                    self.update(.listening)
                }
            }
            self.playbackAcknowledgements[acknowledgementID] = task
        }
    }

    deinit { eventTask?.cancel(); invalidationTask?.cancel(); captureTask?.cancel(); deviceStartTask?.cancel() }

    var isAttached: Bool { engine != nil }
    var isActive: Bool { session != nil || phase == .connecting || stopUnconfirmed || stopFlight != nil }

    func attach(engine: MobileEngineHandle, ownerID: String) {
        self.engine = engine
        engineOwnerID = ownerID
    }

    func detach(ownerID: String) {
        guard engineOwnerID == ownerID else { return }
        engine = nil
        engineOwnerID = nil
        cleanupTask = Task { @MainActor [weak self] in _ = await self?.stop(abort: true) }
    }

    func start(snapshot: AudioConfigurationSnapshot) async throws {
        if let cleanupTask { await cleanupTask.value; self.cleanupTask = nil }
        guard let engine else { throw AudioServiceFailure.nativeFailure(String(localized: "audio_realtime_unavailable")) }
        guard !isActive else { throw AudioServiceFailure.busy }
        guard IOSAudioProviderService.shared.sessionContext != nil else { throw AudioServiceFailure.unavailable }
        generation &+= 1
        let operation = generation
        guard snapshot.configuration.conversation.interaction == "turn_based" else { throw AudioServiceFailure.unsupported }
        caption = ""
        errorMessage = nil
        update(.connecting)
        let open = Task { @MainActor [self] in
            let pinned = try await IOSAudioProviderService.shared.pinRealtime(snapshot: snapshot)
            try Task.checkCancellation()
            guard generation == operation, self.engine === engine else { throw CancellationError() }
            let callback = IOSRealtimeAudioListenerAdapter()
            listener = callback
            eventTask = Task { @MainActor [weak self] in
                do {
                    for try await event in callback.events {
                        guard let self, self.generation == operation, !Task.isCancelled else { return }
                        await self.receive(event, generation: operation)
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
            if generation == operation { openingTask = nil }
        } catch {
            if generation == operation { openingTask = nil }
            if generation == operation { await fail(error.localizedDescription, generation: operation) }
            throw error
        }
    }

    func commitInput() async {
        guard let session, phase == .listening else { return }
        let operation = generation
        let command = beginCommand()
        defer { endCommand(command) }
        inputEnabled = false
        update(.thinking)
        do { try await session.commitInput() }
        catch { scheduleFailure(error.localizedDescription, generation: operation) }
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
            inputEnabled = true
            update(.listening)
        } catch { scheduleFailure(error.localizedDescription, generation: operation) }
    }

    @discardableResult
    func stop(abort: Bool) async -> Bool {
        if let flight = stopFlight { return await waitForStop(flight.id) }
        generation &+= 1
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

    private func receive(_ json: String, generation expectedGeneration: UInt64) async {
        guard generation == expectedGeneration else { return }
        guard let data = json.data(using: .utf8),
              let event = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let type = event["type"] as? String else {
            await fail("The realtime audio host returned an invalid event.", generation: expectedGeneration)
            return
        }
        switch type {
        case "session_ready":
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
                    self.inputEnabled = true
                    self.update(.listening)
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
        case "audio_delta":
            guard let encoded = event["audioBase64"] as? String, encoded.utf8.count <= 1_400_000,
                  let pcm = Data(base64Encoded: encoded), event["encoding"] as? String == "pcm16",
                  (event["channels"] as? NSNumber)?.intValue == 1,
                  let rate = (event["sampleRateHz"] as? NSNumber)?.uint32Value else { await fail("Invalid realtime output audio.", generation: expectedGeneration); return }
            inputEnabled = false
            do { try device.enqueueOutput(pcm: pcm, sampleRateHz: rate, itemID: event["itemId"] as? String); update(.speaking) }
            catch { await fail(error.localizedDescription, generation: expectedGeneration) }
        case "turn_completed":
            device.markAllOutputCompleted()
        case "transcript":
            caption = event["text"] as? String ?? caption
            if event["role"] as? String == "user", event["final"] as? Bool == true {
                inputEnabled = false
                update(.thinking)
            } else { update(phase) }
        case "interrupted":
            device.interruptPlayback(itemID: nil)
            inputEnabled = true
            update(.listening)
        case "tool_call": update(.thinking)
        case "tool_cancelled", "usage": break
        case "closed": await stop(abort: false)
        case "error": await fail((event["message"] as? String) ?? ((event["error"] as? [String: Any])?["message"] as? String) ?? "Realtime audio failed.", generation: expectedGeneration)
        default: break
        }
    }

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
