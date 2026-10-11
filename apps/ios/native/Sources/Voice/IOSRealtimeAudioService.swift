import Foundation
import Observation

enum IOSRealtimeAudioPhase: Equatable { case idle, connecting, listening, thinking, speaking, interrupted, failed }

/// Connects device media to the current Harness Agent driver. This service
/// never creates a second chat engine, executes a tool, or handles credentials.
@Observable
@MainActor
final class IOSRealtimeAudioService {
    static let shared = IOSRealtimeAudioService()
    private let device = IOSRealtimeAudioDevice()
    private var engine: MobileEngineHandle?
    private var engineOwnerID: String?
    private var session: IosRealtimeAudioSession?
    @ObservationIgnored private var eventTask: Task<Void, Never>?
    private var listener: IOSRealtimeAudioListenerAdapter?
    private var generation: UInt64 = 0
    @ObservationIgnored private var cleanupTask: Task<Void, Never>?
    @ObservationIgnored private var captureTask: Task<Void, Never>?
    @ObservationIgnored private var deviceStartTask: Task<Void, Never>?
    private var inputEnabled = false
    private var supportsTruncation = false
    private var pendingPlaybackAcknowledgements = 0
    @ObservationIgnored private var invalidationTask: Task<Void, Never>?
    private(set) var phase: IOSRealtimeAudioPhase = .idle
    private(set) var caption = ""
    private(set) var errorMessage: String?
    var onStateChange: ((IOSRealtimeAudioPhase, String, String?) -> Void)?

    init() {
        invalidationTask = Task { @MainActor [weak self] in
            let events = await VoiceAudioSessionCoordinator.shared.invalidationEvents()
            for await event in events {
                guard let self else { return }
                if await self.device.owns(event) { await self.stop(abort: true) }
            }
        }
        device.onPlaybackCompleted = { [weak self] itemID in
            guard let self, let session = self.session else { return }
            let generation = self.generation
            self.pendingPlaybackAcknowledgements += 1
            Task { @MainActor [weak self] in
                do { try await session.playbackCompleted(itemId: itemID) }
                catch {
                    guard let self, generation == self.generation else { return }
                    await self.fail(error.localizedDescription)
                    return
                }
                guard let self, generation == self.generation else { return }
                self.pendingPlaybackAcknowledgements -= 1
                if self.pendingPlaybackAcknowledgements == 0, !self.device.hasPendingOutput {
                    self.inputEnabled = true
                    self.update(.listening)
                }
            }
        }
    }

    deinit { eventTask?.cancel(); invalidationTask?.cancel(); captureTask?.cancel(); deviceStartTask?.cancel() }

    var isAttached: Bool { engine != nil }
    var isActive: Bool { session != nil || phase == .connecting }

    func attach(engine: MobileEngineHandle, ownerID: String) {
        self.engine = engine
        engineOwnerID = ownerID
    }

    func detach(ownerID: String) {
        guard engineOwnerID == ownerID else { return }
        engine = nil
        engineOwnerID = nil
        cleanupTask = Task { @MainActor [weak self] in await self?.stop(abort: true) }
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
        do {
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
                    await self.fail(error.localizedDescription)
                }
            }
            let opened = try await startIosRealtimeAudio(engine: engine, providerHost: pinned.host, requestJson: pinned.requestJSON, listener: callback)
            guard generation == operation, self.engine === engine else {
                await opened.abort()
                throw CancellationError()
            }
            session = opened
        } catch {
            if generation == operation { await fail(error.localizedDescription) }
            throw error
        }
    }

    func commitInput() async {
        guard let session, phase == .listening else { return }
        inputEnabled = false
        update(.thinking)
        do { try await session.commitInput() }
        catch { await fail(error.localizedDescription) }
    }

    func interrupt() async {
        guard let session else { return }
        guard supportsTruncation else {
            // The Agent must never keep audio history claiming unplayed media.
            await stop(abort: true)
            return
        }
        let itemID = device.currentPlaybackItemID
        let position = device.interruptPlayback(itemID: itemID)
        inputEnabled = false
        update(.interrupted)
        do {
            try await session.interrupt(itemId: itemID, audioEndMs: itemID == nil ? nil : UInt32(clamping: position))
            inputEnabled = true
            update(.listening)
        } catch { await fail(error.localizedDescription) }
    }

    func stop(abort: Bool) async {
        generation &+= 1
        let old = session
        session = nil
        listener?.finish()
        listener = nil
        eventTask?.cancel(); eventTask = nil
        captureTask?.cancel(); captureTask = nil
        deviceStartTask?.cancel(); deviceStartTask = nil
        inputEnabled = false
        pendingPlaybackAcknowledgements = 0
        await device.stop()
        if let old {
            if abort { await old.abort() }
            else { try? await old.close() }
        }
        update(.idle)
    }

    private func receive(_ json: String, generation expectedGeneration: UInt64) async {
        guard let data = json.data(using: .utf8),
              let event = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let type = event["type"] as? String else {
            await fail("The realtime audio host returned an invalid event.")
            return
        }
        switch type {
        case "session_ready":
            guard let input = event["inputFormat"] as? [String: Any], let output = event["outputFormat"] as? [String: Any],
                  input["encoding"] as? String == "pcm16", output["encoding"] as? String == "pcm16",
                  (input["channels"] as? NSNumber)?.intValue == 1, (output["channels"] as? NSNumber)?.intValue == 1,
                  let inputRate = (input["sampleRateHz"] as? NSNumber)?.uint32Value,
                  let outputRate = (output["sampleRateHz"] as? NSNumber)?.uint32Value else {
                await fail("The realtime provider did not negotiate supported PCM16 mono media.")
                return
            }
            supportsTruncation = (event["capabilities"] as? [String: Any])?["audioTruncation"] as? Bool ?? false
            deviceStartTask = Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    let input = try await self.device.start(inputSampleRateHz: inputRate, outputSampleRateHz: outputRate)
                    guard self.generation == expectedGeneration, !Task.isCancelled else { await self.device.stop(); return }
                    while self.session == nil, self.generation == expectedGeneration {
                        try await Task.sleep(for: .milliseconds(10))
                    }
                    guard self.generation == expectedGeneration, !Task.isCancelled else { await self.device.stop(); return }
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
                            await self.fail(error.localizedDescription)
                        }
                    }
                } catch is CancellationError {
                } catch { if self.generation == expectedGeneration { await self.fail(error.localizedDescription) } }
            }
        case "audio_delta":
            guard let encoded = event["audioBase64"] as? String, encoded.utf8.count <= 1_400_000,
                  let pcm = Data(base64Encoded: encoded), event["encoding"] as? String == "pcm16",
                  (event["channels"] as? NSNumber)?.intValue == 1,
                  let rate = (event["sampleRateHz"] as? NSNumber)?.uint32Value else { await fail("Invalid realtime output audio."); return }
            inputEnabled = false
            do { try device.enqueueOutput(pcm: pcm, sampleRateHz: rate, itemID: event["itemId"] as? String); update(.speaking) }
            catch { await fail(error.localizedDescription) }
        case "turn_completed":
            device.markAllOutputCompleted()
        case "transcript":
            caption = event["text"] as? String ?? caption
            if event["role"] as? String == "user", event["final"] as? Bool == true {
                inputEnabled = false
                update(.thinking)
            } else { update(phase) }
        case "interrupted":
            device.interruptPlayback()
            inputEnabled = true
            update(.listening)
        case "tool_call": update(.thinking)
        case "tool_cancelled", "usage": break
        case "closed": await stop(abort: false)
        case "error": await fail((event["message"] as? String) ?? ((event["error"] as? [String: Any])?["message"] as? String) ?? "Realtime audio failed.")
        default: break
        }
    }

    private func fail(_ message: String) async {
        await stop(abort: true)
        errorMessage = message
        update(.failed)
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
