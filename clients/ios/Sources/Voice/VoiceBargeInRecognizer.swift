import AVFoundation
import Foundation
import Speech

enum VoiceBargeInEvent: Equatable, Sendable {
    case speechStarted
    case partial(String)
    case transcript(String)
    case empty
    case failed(String)
}

@MainActor
protocol VoiceBargeInSession: AnyObject {
    var events: AsyncStream<VoiceBargeInEvent> { get }
    func stop() async
}

@MainActor
protocol VoiceBargeInRecognizing: AnyObject {
    func start(language: String?, prefersOnDevice: Bool) async throws -> any VoiceBargeInSession
    func start(
        language: String?,
        prefersOnDevice: Bool,
        configurationSnapshot: AudioConfigurationSnapshot
    ) async throws -> any VoiceBargeInSession
}

extension VoiceBargeInRecognizing {
    func start(
        language: String?,
        prefersOnDevice: Bool,
        configurationSnapshot _: AudioConfigurationSnapshot
    ) async throws -> any VoiceBargeInSession {
        try await start(language: language, prefersOnDevice: prefersOnDevice)
    }
}

enum VoiceBargeInError: LocalizedError {
    case permissionDenied
    case unavailable
    case voiceProcessingUnavailable(String)

    var errorDescription: String? {
        switch self {
        case .permissionDenied:
            return String(localized: "voice_bargein_permission_unavailable")
        case .unavailable:
            return String(localized: "voice_bargein_language_unavailable")
        case let .voiceProcessingUnavailable(message):
            return String(localized: "voice_bargein_route_unsupported \(message)")
        }
    }
}

/// Creates one response-scoped duplex listener. Recognition is intentionally
/// started only after sustained voice activity; until then the input tap keeps a
/// short pre-roll and does not submit synthesized app speech to Speech.framework.
@MainActor
final class VoiceBargeInRecognizer: VoiceBargeInRecognizing {
    private var activeSessions: [UUID: any VoiceBargeInSession] = [:]
    private let coordinator: VoiceAudioSessionCoordinator
    var onSessionsChanged: (@MainActor () -> Void)?

    var activeSessionCount: Int { activeSessions.count }

    init(coordinator: VoiceAudioSessionCoordinator? = nil) {
        self.coordinator = coordinator ?? .shared
    }

    func start(language: String?, prefersOnDevice: Bool) async throws -> any VoiceBargeInSession {
        let snapshot = AudioConfigurationRuntime.snapshot()
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
        let identifier = resolveAudioLanguageForNativeDevice(
            configured: language ?? configurationSnapshot.configuration.language,
            deviceLocale: Locale.autoupdatingCurrent.identifier
        )
        let recognizer = SFSpeechRecognizer(locale: Locale(identifier: identifier))
        let route = AudioConfigurationRuntime.route(
            kind: .recognition,
            snapshot: configurationSnapshot,
            languageOverride: language
        )
        guard route.status == .ready || route.status == .permissionRequired,
              let effective = route.effective else { throw VoiceBargeInError.unavailable }
        if effective.source == .offline {
            guard AVAudioApplication.shared.recordPermission == .granted else {
                throw VoiceBargeInError.permissionDenied
            }
            guard let modelID = effective.modelId,
                  let model = GeneratedVoiceModelCatalog.byID(modelID),
                  model.kind == .stt,
                  VoiceModelFiles.modelRoot(for: model) != nil
            else { throw VoiceBargeInError.unavailable }
            return track(SherpaVoiceBargeInSession(
                language: language,
                configurationSnapshot: configurationSnapshot,
                route: route
            ))
        }
        guard effective.source == .system, let recognizer, recognizer.isAvailable else {
            throw VoiceBargeInError.unavailable
        }
        guard SFSpeechRecognizer.authorizationStatus() == .authorized,
              AVAudioApplication.shared.recordPermission == .granted
        else { throw VoiceBargeInError.permissionDenied }

        let lease = try await coordinator.acquire(.flowDuplex)
        do {
            let operation = try VoiceBargeInOperation(
                recognizer: recognizer,
                prefersOnDevice: prefersOnDevice,
                lease: lease,
                coordinator: coordinator
            )
            return track(SystemVoiceBargeInSession(operation: operation))
        } catch {
            await coordinator.release(lease)
            throw error
        }
    }

    func stopAll() async {
        let sessions = Array(activeSessions.values)
        for session in sessions { await session.stop() }
        activeSessions.removeAll()
        onSessionsChanged?()
    }

    private func track(_ session: any VoiceBargeInSession) -> any VoiceBargeInSession {
        let id = UUID()
        activeSessions[id] = session
        onSessionsChanged?()
        return TrackedVoiceBargeInSession(session: session) { [weak self] in
            self?.activeSessions.removeValue(forKey: id)
            self?.onSessionsChanged?()
        }
    }
}

@MainActor
private final class TrackedVoiceBargeInSession: VoiceBargeInSession {
    let events: AsyncStream<VoiceBargeInEvent>
    private let session: any VoiceBargeInSession
    private let onStop: @MainActor () -> Void
    private var isStopped = false

    init(session: any VoiceBargeInSession, onStop: @escaping @MainActor () -> Void) {
        self.session = session
        self.events = session.events
        self.onStop = onStop
    }

    func stop() async {
        guard !isStopped else { return }
        isStopped = true
        await session.stop()
        onStop()
    }
}

@MainActor
private final class SherpaVoiceBargeInSession: VoiceBargeInSession {
    let events: AsyncStream<VoiceBargeInEvent>
    private let language: String?
    private let configurationSnapshot: AudioConfigurationSnapshot
    private let route: AudioRouteResolution
    private var continuation: AsyncStream<VoiceBargeInEvent>.Continuation?
    private var task: Task<Void, Never>?

    init(
        language: String?,
        configurationSnapshot: AudioConfigurationSnapshot,
        route: AudioRouteResolution
    ) {
        self.language = language
        self.configurationSnapshot = configurationSnapshot
        self.route = route
        var captured: AsyncStream<VoiceBargeInEvent>.Continuation?
        events = AsyncStream { captured = $0 }
        continuation = captured
        task = Task { [weak self] in
            guard let self else { return }
            do {
                let rawTranscript = try await IOSAudioService.shared.transcribeForBargeIn(
                    language: self.language,
                    configurationSnapshot: self.configurationSnapshot,
                    route: self.route,
                    automaticEndpointAfterSilence: .milliseconds(800)
                )
                let transcript = rawTranscript.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !Task.isCancelled else { return }
                if transcript.isEmpty {
                    continuation?.yield(.empty)
                } else {
                    continuation?.yield(.speechStarted)
                    continuation?.yield(.transcript(transcript))
                }
            } catch is CancellationError {
                return
            } catch {
                continuation?.yield(.failed(error.localizedDescription))
            }
            continuation?.finish()
        }
        continuation?.onTermination = { [weak self] _ in
            Task { @MainActor [weak self] in await self?.stop() }
        }
    }

    func stop() async {
        await IOSAudioService.shared.cancelTranscription(owner: .ui(instanceID: "flow-barge-in"))
        task?.cancel()
        task = nil
        continuation?.finish()
        continuation = nil
    }
}

@MainActor
private final class SystemVoiceBargeInSession: VoiceBargeInSession {
    let events: AsyncStream<VoiceBargeInEvent>
    private let operation: VoiceBargeInOperation

    init(operation: VoiceBargeInOperation) {
        self.operation = operation
        events = operation.events
    }

    func stop() async {
        await operation.stop()
    }
}

/// Audio callbacks and Speech callbacks can arrive on unrelated queues. The
/// recursive lock protects every mutable field; the AsyncStream is single-owner
/// and emits only low-frequency state/transcript events, never PCM buffers.
private final class VoiceBargeInOperation: @unchecked Sendable {
    let events: AsyncStream<VoiceBargeInEvent>

    private let lock = NSRecursiveLock()
    private let recognizer: SFSpeechRecognizer
    private let prefersOnDevice: Bool
    private let coordinator: VoiceAudioSessionCoordinator
    private let audioEngine = AVAudioEngine()
    private let clock = ContinuousClock()
    private var continuation: AsyncStream<VoiceBargeInEvent>.Continuation?
    private var lease: VoiceAudioSessionCoordinator.Lease?
    private var recognitionRequest: SFSpeechAudioBufferRecognitionRequest?
    private var recognitionTask: SFSpeechRecognitionTask?
    private var endpointTask: Task<Void, Never>?
    private var recognitionTimeoutTask: Task<Void, Never>?
    private var observers: [NSObjectProtocol] = []
    private var preRoll: [AVAudioPCMBuffer] = []
    private var preRollFrames: AVAudioFramePosition = 0
    private var ambientRMS: Float = 0.005
    private var voiceCandidateStarted: ContinuousClock.Instant?
    private var lastVoiceActivity: ContinuousClock.Instant?
    private var latestTranscript = ""
    private var cleanupFinished = false
    private var cleanupWaiters: [CheckedContinuation<Void, Never>] = []
    private var recognitionStarting = false
    private var recognitionStarted = false
    private var inputStarted = false
    private var terminal = false

    init(
        recognizer: SFSpeechRecognizer,
        prefersOnDevice: Bool,
        lease: VoiceAudioSessionCoordinator.Lease,
        coordinator: VoiceAudioSessionCoordinator
    ) throws {
        self.recognizer = recognizer
        self.prefersOnDevice = prefersOnDevice
        self.coordinator = coordinator
        self.lease = lease
        var captured: AsyncStream<VoiceBargeInEvent>.Continuation?
        events = AsyncStream { captured = $0 }
        continuation = captured
        continuation?.onTermination = { @Sendable [weak self] _ in
            Task { await self?.stop() }
        }
        try startAudio()
        installLifecycleObservers()
    }

    deinit {
        let endpointTask: Task<Void, Never>?
        let timeoutTask: Task<Void, Never>?
        let lease: VoiceAudioSessionCoordinator.Lease?
        lock.lock()
        endpointTask = self.endpointTask
        timeoutTask = recognitionTimeoutTask
        lease = self.lease
        self.endpointTask = nil
        recognitionTimeoutTask = nil
        self.lease = nil
        stopInputLocked()
        recognitionTask?.cancel()
        recognitionTask = nil
        recognitionRequest = nil
        removeObserversLocked()
        lock.unlock()
        endpointTask?.cancel()
        timeoutTask?.cancel()
        if let lease {
            let coordinator = self.coordinator
            Task { await coordinator.release(lease) }
        }
    }

    func stop() async {
        let cleanup = beginTerminal()
        guard cleanup.didBegin else {
            await waitForCleanup()
            return
        }
        cleanup.endpoint?.cancel()
        cleanup.timeout?.cancel()
        cleanup.recognition?.cancel()
        if let lease = cleanup.lease {
            await coordinator.release(lease)
        }
        cleanup.continuation?.finish()
        markCleanupFinished()
    }

    private func startAudio() throws {
        let input = audioEngine.inputNode
        do {
            try input.setVoiceProcessingEnabled(true)
        } catch {
            throw VoiceBargeInError.voiceProcessingUnavailable(error.localizedDescription)
        }
        let format = input.outputFormat(forBus: 0)
        input.installTap(onBus: 0, bufferSize: 512, format: format) { [weak self] buffer, _ in
            self?.consume(buffer)
        }
        inputStarted = true
        audioEngine.prepare()
        do {
            try audioEngine.start()
        } catch {
            stopInputLocked()
            throw VoiceBargeInError.voiceProcessingUnavailable(error.localizedDescription)
        }
    }

    private func consume(_ buffer: AVAudioPCMBuffer) {
        let rms = Self.rms(of: buffer)
        let now = clock.now
        var shouldStartRecognition = false

        lock.lock()
        guard !terminal else {
            lock.unlock()
            return
        }

        if recognitionStarted, let request = recognitionRequest {
            request.append(buffer)
        } else if let copied = Self.copy(buffer) {
            preRoll.append(copied)
            preRollFrames += AVAudioFramePosition(copied.frameLength)
            let maximumFrames = AVAudioFramePosition(copied.format.sampleRate * 0.5)
            while preRollFrames > maximumFrames, preRoll.count > 1 {
                preRollFrames -= AVAudioFramePosition(preRoll.removeFirst().frameLength)
            }
        }

        let threshold = max(0.015, ambientRMS * 2.2)
        if rms >= threshold {
            lastVoiceActivity = now
            if recognitionStarted || recognitionStarting {
                voiceCandidateStarted = nil
            } else if let started = voiceCandidateStarted {
                if started.duration(to: now) >= .milliseconds(250) {
                    recognitionStarting = true
                    voiceCandidateStarted = nil
                    shouldStartRecognition = true
                }
            } else {
                voiceCandidateStarted = now
            }
        } else {
            voiceCandidateStarted = nil
            if !recognitionStarted, !recognitionStarting {
                ambientRMS = (ambientRMS * 0.95) + (rms * 0.05)
            }
        }
        lock.unlock()

        if shouldStartRecognition {
            yieldIfActive(.speechStarted)
            DispatchQueue.main.async { [weak self] in self?.startRecognition() }
        }
    }

    private func startRecognition() {
        lock.lock()
        guard !terminal, recognitionStarting, !recognitionStarted else {
            lock.unlock()
            return
        }

        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        request.taskHint = .dictation
        request.addsPunctuation = true
        request.requiresOnDeviceRecognition = prefersOnDevice && recognizer.supportsOnDeviceRecognition
        recognitionRequest = request
        recognitionStarted = true
        recognitionStarting = false
        let buffered = preRoll
        preRoll.removeAll()
        preRollFrames = 0
        buffered.forEach(request.append)
        lock.unlock()

        let task = recognizer.recognitionTask(with: request) { [weak self] result, error in
            self?.handleRecognition(result: result, error: error)
        }

        lock.lock()
        guard !terminal else {
            lock.unlock()
            task.cancel()
            return
        }
        recognitionTask = task
        recognitionTimeoutTask = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(30)) } catch { return }
            self?.finishRecognition(forceFailure: String(localized: "voice_bargein_timeout"))
        }
        lock.unlock()
    }

    private func handleRecognition(result: SFSpeechRecognitionResult?, error: Error?) {
        if let result {
            let text = result.bestTranscription.formattedString
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if !text.isEmpty {
                lock.lock()
                guard !terminal else {
                    lock.unlock()
                    return
                }
                let changed = text != latestTranscript
                latestTranscript = text
                if endpointTask == nil {
                    endpointTask = Task { [weak self] in await self?.finishAfterSilence() }
                }
                lock.unlock()
                if changed { yieldIfActive(.partial(text)) }
            }
            if result.isFinal {
                finishRecognition(forceFailure: nil)
                return
            }
        }

        if let error {
            let nsError = error as NSError
            if nsError.domain == "kAFAssistantErrorDomain", nsError.code == 1110 {
                // Speech can report NoSpeech after already delivering a usable
                // partial. Preserve that transcript instead of treating a valid
                // interruption as a false VAD trigger.
                finishRecognition(forceFailure: nil)
            } else {
                finishRecognition(forceFailure: error.localizedDescription)
            }
        }
    }

    private func finishAfterSilence() async {
        let silence: Duration = .milliseconds(1_200)
        while !Task.isCancelled {
            guard let elapsed = silenceElapsed() else { return }
            if elapsed >= silence {
                endRecognitionAudio()
                return
            }
            do { try await Task.sleep(for: silence - elapsed) } catch { return }
        }
    }

    private func silenceElapsed() -> Duration? {
        lock.lock()
        defer { lock.unlock() }
        guard !terminal, let lastVoiceActivity else { return nil }
        return lastVoiceActivity.duration(to: clock.now)
    }

    private func endRecognitionAudio() {
        lock.lock()
        recognitionRequest?.endAudio()
        lock.unlock()
    }

    private func finishRecognition(forceFailure: String?) {
        lock.lock()
        let transcript = latestTranscript.trimmingCharacters(in: .whitespacesAndNewlines)
        lock.unlock()
        if !transcript.isEmpty {
            finish(.transcript(transcript))
        } else if let forceFailure {
            finish(.failed(forceFailure))
        } else {
            finish(.empty)
        }
    }

    private func finish(_ event: VoiceBargeInEvent) {
        let cleanup = beginTerminal()
        guard cleanup.didBegin else { return }
        cleanup.endpoint?.cancel()
        cleanup.timeout?.cancel()
        cleanup.recognition?.cancel()
        Task { [cleanup] in
            if let lease = cleanup.lease {
                await coordinator.release(lease)
            }
            cleanup.continuation?.yield(event)
            cleanup.continuation?.finish()
            self.markCleanupFinished()
        }
    }

    private func yieldIfActive(_ event: VoiceBargeInEvent) {
        lock.lock()
        let activeContinuation = terminal ? nil : continuation
        lock.unlock()
        activeContinuation?.yield(event)
    }

    private func waitForCleanup() async {
        await withCheckedContinuation { continuation in
            lock.lock()
            if cleanupFinished {
                lock.unlock()
                continuation.resume()
            } else {
                cleanupWaiters.append(continuation)
                lock.unlock()
            }
        }
    }

    private func markCleanupFinished() {
        lock.lock()
        guard !cleanupFinished else {
            lock.unlock()
            return
        }
        cleanupFinished = true
        let waiters = cleanupWaiters
        cleanupWaiters.removeAll()
        lock.unlock()
        waiters.forEach { $0.resume() }
    }

    private typealias Cleanup = (
        didBegin: Bool,
        lease: VoiceAudioSessionCoordinator.Lease?,
        recognition: SFSpeechRecognitionTask?,
        endpoint: Task<Void, Never>?,
        timeout: Task<Void, Never>?,
        continuation: AsyncStream<VoiceBargeInEvent>.Continuation?
    )

    private func beginTerminal() -> Cleanup {
        lock.lock()
        defer { lock.unlock() }
        guard !terminal else { return (false, nil, nil, nil, nil, nil) }
        terminal = true
        let cleanup: Cleanup = (
            true,
            lease,
            recognitionTask,
            endpointTask,
            recognitionTimeoutTask,
            continuation
        )
        lease = nil
        recognitionTask = nil
        recognitionRequest = nil
        endpointTask = nil
        recognitionTimeoutTask = nil
        continuation = nil
        stopInputLocked()
        removeObserversLocked()
        return cleanup
    }

    private func stopInputLocked() {
        guard inputStarted else { return }
        audioEngine.stop()
        audioEngine.inputNode.removeTap(onBus: 0)
        inputStarted = false
    }

    private func installLifecycleObservers() {
        let center = NotificationCenter.default
        observers.append(center.addObserver(
            forName: AVAudioSession.interruptionNotification,
            object: AVAudioSession.sharedInstance(),
            queue: nil
        ) { [weak self] notification in
            let raw = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt
            guard raw == AVAudioSession.InterruptionType.began.rawValue else { return }
            self?.finish(.failed(String(localized: "voice_audio_session_interrupted")))
        })
        // Route changes are reapplied by VoiceAudioSessionCoordinator while the
        // same duplex lease remains active. Ending this stream here would leave
        // an externally-managed TTS queue without an audio-session owner.
    }

    private func removeObserversLocked() {
        observers.forEach(NotificationCenter.default.removeObserver)
        observers.removeAll()
    }

    private static func copy(_ buffer: AVAudioPCMBuffer) -> AVAudioPCMBuffer? {
        guard let source = buffer.floatChannelData,
              let copy = AVAudioPCMBuffer(
                  pcmFormat: buffer.format,
                  frameCapacity: buffer.frameLength
              ),
              let destination = copy.floatChannelData
        else { return nil }
        copy.frameLength = buffer.frameLength
        let bytes = Int(buffer.frameLength) * MemoryLayout<Float>.size
        for channel in 0..<Int(buffer.format.channelCount) {
            memcpy(destination[channel], source[channel], bytes)
        }
        return copy
    }

    private static func rms(of buffer: AVAudioPCMBuffer) -> Float {
        let frameCount = Int(buffer.frameLength)
        guard frameCount > 0, let samples = buffer.floatChannelData?[0] else { return 0 }
        var sum: Float = 0
        for index in 0..<frameCount {
            let sample = samples[index]
            sum += sample * sample
        }
        return sqrt(sum / Float(frameCount))
    }
}
