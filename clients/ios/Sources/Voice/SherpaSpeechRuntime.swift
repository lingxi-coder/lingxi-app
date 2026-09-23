import AVFoundation
import Foundation

final class SherpaRecognitionOperation: VoiceRecognitionOperation, @unchecked Sendable {
    private let lock = NSRecursiveLock()
    private let modelID: String
    private let modelDirectory: URL
    private let automaticEndpointAfterSilence: Duration?
    private let maximumSampleCount: Int
    private let audioEngine = AVAudioEngine()
    private let clock = ContinuousClock()

    private var continuation: CheckedContinuation<String, Error>?
    private var terminalResult: Result<String, Error>?
    private var samples: [Float] = []
    private var sampleRate: Int32 = 16_000
    private var inputStarted = false
    private var inputEnded = false
    private var finishRequested = false
    private var decodeStarted = false
    private var ambientRMS: Float = 0.005
    private var speechStarted = false
    private var voicedFrames = 0
    private var lastVoiceActivity: ContinuousClock.Instant?
    private var endpointTask: Task<Void, Never>?
    private var timeoutTask: Task<Void, Never>?

    init(
        modelID: String,
        modelDirectory: URL,
        automaticEndpointAfterSilence: Duration?,
        maximumPayloadBytes: UInt64
    ) {
        self.modelID = modelID
        self.modelDirectory = modelDirectory
        self.automaticEndpointAfterSilence = automaticEndpointAfterSilence
        self.maximumSampleCount = Int(min(maximumPayloadBytes / UInt64(MemoryLayout<Int16>.size), UInt64(Int.max)))
    }

    deinit {
        lock.lock()
        endInputLocked()
        let endpointTask = endpointTask
        let timeoutTask = timeoutTask
        lock.unlock()
        endpointTask?.cancel()
        timeoutTask?.cancel()
    }

    func run() async throws -> String {
        try startAudio()
        return try await withCheckedThrowingContinuation { continuation in
            lock.lock()
            if let terminalResult {
                lock.unlock()
                continuation.resume(with: terminalResult)
                return
            }
            self.continuation = continuation
            timeoutTask = Task { [weak self] in
                try? await Task.sleep(for: .seconds(30))
                guard !Task.isCancelled else { return }
                self?.finishInput()
            }
            let shouldFinish = finishRequested
            lock.unlock()
            if shouldFinish { finishInput() }
        }
    }

    func finishInput() {
        lock.lock()
        finishRequested = true
        endInputLocked()
        let shouldDecode = !decodeStarted && terminalResult == nil
        if shouldDecode { decodeStarted = true }
        let capturedSamples = samples
        let capturedRate = sampleRate
        lock.unlock()
        guard shouldDecode else { return }
        endpointTask?.cancel()
        Task { [weak self] in
            guard let self else { return }
            guard capturedSamples.count >= Int(capturedRate / 4) else {
                complete(.failure(SpeechRecognitionError.NoSpeech))
                return
            }
            do {
                let text = try await Self.decode(
                    modelID: modelID,
                    modelDirectory: modelDirectory,
                    samples: capturedSamples,
                    sampleRate: capturedRate
                )
                let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
                complete(trimmed.isEmpty ? .failure(SpeechRecognitionError.NoSpeech) : .success(trimmed))
            } catch {
                complete(.failure(error))
            }
        }
    }

    func cancel(with error: Error) {
        complete(.failure(error))
    }

    private func startAudio() throws {
        lock.lock()
        defer { lock.unlock() }
        if let terminalResult {
            _ = try terminalResult.get()
            return
        }
        let input = audioEngine.inputNode
        let format = input.outputFormat(forBus: 0)
        sampleRate = Int32(format.sampleRate.rounded())
        input.installTap(onBus: 0, bufferSize: 1_600, format: format) { [weak self] buffer, _ in
            self?.consume(buffer)
        }
        inputStarted = true
        audioEngine.prepare()
        do {
            try audioEngine.start()
        } catch {
            endInputLocked()
            throw SpeechRecognitionError.Retriable(message: "offline mic start: \(error.localizedDescription)")
        }
        if finishRequested { endInputLocked() }
    }

    private func consume(_ buffer: AVAudioPCMBuffer) {
        let count = Int(buffer.frameLength)
        guard count > 0 else { return }
        guard count <= maximumSampleCount else {
            complete(.failure(AudioServiceFailure.mediaTooLarge))
            return
        }
        var values = [Float](repeating: 0, count: count)
        if let channel = buffer.floatChannelData?[0] {
            values.withUnsafeMutableBufferPointer { output in
                output.baseAddress?.update(from: channel, count: count)
            }
        } else if let channel = buffer.int16ChannelData?[0] {
            for index in 0 ..< count {
                values[index] = Float(channel[index]) / Float(Int16.max)
            }
        } else {
            return
        }
        var sum: Float = 0
        for value in values { sum += value * value }
        let rms = sqrt(sum / Float(count))

        lock.lock()
        guard terminalResult == nil, !inputEnded else {
            lock.unlock()
            return
        }
        guard count <= maximumSampleCount - samples.count else {
            lock.unlock()
            complete(.failure(AudioServiceFailure.mediaTooLarge))
            return
        }
        samples.append(contentsOf: values)
        let threshold = max(0.015, ambientRMS * 2.0)
        if rms >= threshold {
            voicedFrames += 1
            if voicedFrames >= 3 { speechStarted = true }
            if speechStarted { lastVoiceActivity = clock.now }
        } else if !speechStarted {
            ambientRMS = (ambientRMS * 0.95) + (rms * 0.05)
        }
        if speechStarted,
           endpointTask == nil,
           let silence = automaticEndpointAfterSilence {
            endpointTask = Task { [weak self] in
                await self?.finishAfterTrailingSilence(silence)
            }
        }
        lock.unlock()
    }

    private func finishAfterTrailingSilence(_ silence: Duration) async {
        while !Task.isCancelled {
            let (lastVoiceActivity, ended) = endpointState()
            guard !ended, let lastVoiceActivity else { return }
            let elapsed = lastVoiceActivity.duration(to: clock.now)
            if elapsed >= silence {
                finishInput()
                return
            }
            try? await Task.sleep(for: silence - elapsed)
        }
    }

    private func endpointState() -> (ContinuousClock.Instant?, Bool) {
        lock.lock()
        defer { lock.unlock() }
        return (lastVoiceActivity, inputEnded || terminalResult != nil)
    }

    private func complete(_ result: Result<String, Error>) {
        lock.lock()
        guard terminalResult == nil else {
            lock.unlock()
            return
        }
        terminalResult = result
        let continuation = continuation
        self.continuation = nil
        let endpointTask = endpointTask
        self.endpointTask = nil
        let timeoutTask = timeoutTask
        self.timeoutTask = nil
        endInputLocked()
        lock.unlock()
        endpointTask?.cancel()
        timeoutTask?.cancel()
        continuation?.resume(with: result)
    }

    private func endInputLocked() {
        guard !inputEnded else { return }
        inputEnded = true
        if inputStarted {
            audioEngine.stop()
            audioEngine.inputNode.removeTap(onBus: 0)
            inputStarted = false
        }
    }

    private static func decode(
        modelID: String,
        modelDirectory: URL,
        samples: [Float],
        sampleRate: Int32
    ) async throws -> String {
        try await Task.detached(priority: .userInitiated) {
            try Task.checkCancellation()
            return try samples.withUnsafeBufferPointer { buffer in
                guard let base = buffer.baseAddress else { throw SpeechRecognitionError.NoSpeech }
                let copied: UnsafeMutablePointer<CChar>?
                if modelID.contains("zipformer") {
                    let recognizer = modelDirectory.path.withCString(LXOnlineRecognizerCreate)
                    guard let recognizer else { throw SpeechRecognitionError.Unavailable }
                    defer { LXOnlineRecognizerDestroy(recognizer) }
                    guard let stream = LXOnlineStreamCreate(recognizer) else {
                        throw SpeechRecognitionError.Unavailable
                    }
                    defer { LXOnlineStreamDestroy(stream) }
                    LXOnlineStreamAccept(
                        recognizer,
                        stream,
                        base,
                        Int32(buffer.count),
                        sampleRate
                    )
                    LXOnlineStreamFinish(recognizer, stream)
                    copied = LXOnlineStreamCopyText(recognizer, stream)
                } else {
                    copied = modelDirectory.path.withCString { directory in
                        LXOfflineMoonshineCopyText(
                            directory,
                            base,
                            Int32(buffer.count),
                            sampleRate
                        )
                    }
                }
                guard let copied else { throw SpeechRecognitionError.Unavailable }
                defer { LXFree(copied) }
                return String(cString: copied)
            }
        }.value
    }
}

enum SherpaSpeechRenderer {
    static func render(
        text: String,
        modelID: String,
        modelDirectory: URL,
        speakerID: Int32,
        speed: Double,
        maximumBytes: UInt64,
        cancellationToken suppliedToken: SherpaAudioCancellationToken? = nil
    ) async throws -> (pcm: Data, sampleRate: UInt32) {
        guard maximumBytes >= UInt64(MemoryLayout<Int16>.size),
              maximumBytes <= UInt64(Int.max)
        else { throw AudioServiceFailure.invalidRequest }
        guard let cancellation = suppliedToken ?? SherpaAudioCancellationToken() else {
            throw AudioServiceFailure.nativeFailure("unable to create Sherpa cancellation token")
        }
        return try await withTaskCancellationHandler {
            try await Task.detached(priority: .userInitiated) {
                var pointer: UnsafeMutablePointer<Int16>?
                var count: Int32 = 0
                var sampleRate: Int32 = 0
                let result = modelDirectory.path.withCString { directory in
                    modelID.withCString { model in
                        text.withCString { text in
                            LXSherpaTtsCopyPCM16(
                                directory,
                                model,
                                text,
                                speakerID,
                                Float(min(2, max(0.5, speed))),
                                maximumBytes,
                                cancellation.pointer,
                                &pointer,
                                &count,
                                &sampleRate
                            )
                        }
                    }
                }
                switch result {
                case 1:
                    guard let pointer, count > 0, sampleRate > 0 else {
                        throw AudioServiceFailure.synthesisFailed("Sherpa returned invalid PCM", operationStarted: false)
                    }
                    defer { LXFree(pointer) }
                    let byteCount = Int(count) * MemoryLayout<Int16>.size
                    guard UInt64(byteCount) <= maximumBytes else { throw AudioServiceFailure.mediaTooLarge }
                    return (Data(bytes: pointer, count: byteCount), UInt32(sampleRate))
                case 2:
                    throw AudioServiceFailure.mediaTooLarge
                case 3:
                    throw CancellationError()
                default:
                    throw SpeechRecognitionError.Unavailable
                }
            }.value
        } onCancel: {
            cancellation.cancel()
        }
    }
}

@MainActor
final class SherpaVoiceSpeechStream: VoiceSpeechStreamingSession {
    private let configuration: VoiceSpeechConfiguration
    private let modelID: String
    private let modelDirectory: URL
    private let speakerID: Int32
    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private let coordinator: VoiceAudioSessionCoordinator
    private var audioLease: VoiceAudioSessionCoordinator.Lease?
    private var modelReference: UUID?
    private var activeRenderCancellation: SherpaAudioCancellationToken?
    private var queuedText: [String] = []
    private var playbackContinuation: CheckedContinuation<VoiceSpeechPlaybackOutcome, Never>?
    private var terminalOutcome: VoiceSpeechPlaybackOutcome?
    private var isPaused = false
    var onTerminal: (() -> Void)?

    init(
        configuration: VoiceSpeechConfiguration,
        modelID: String,
        modelDirectory: URL,
        speakerID: Int32,
        audioLease: VoiceAudioSessionCoordinator.Lease?,
        modelReference: UUID? = nil,
        coordinator: VoiceAudioSessionCoordinator? = nil
    ) {
        self.configuration = configuration
        self.modelID = modelID
        self.modelDirectory = modelDirectory
        self.speakerID = speakerID
        self.audioLease = audioLease
        self.modelReference = modelReference
        self.coordinator = coordinator ?? .shared
        engine.attach(player)
    }

    func enqueue(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard terminalOutcome == nil, !trimmed.isEmpty else { return }
        queuedText.append(trimmed)
    }

    func pause() {
        guard terminalOutcome == nil, player.isPlaying else { return }
        isPaused = true
        player.pause()
    }

    func resume() {
        guard terminalOutcome == nil, isPaused else { return }
        isPaused = false
        player.play()
    }

    func finish() async throws -> VoiceSpeechPlaybackOutcome {
        if let terminalOutcome { return terminalOutcome }
        let segments = queuedText
        queuedText.removeAll()
        for segment in segments {
            let rendered: (pcm: Data, sampleRate: UInt32)
            do {
                try Task.checkCancellation()
                guard let maximumBytes = configuration.maxPayloadBytes else {
                    throw AudioServiceFailure.invalidRequest
                }
                guard let cancellation = SherpaAudioCancellationToken() else {
                    throw AudioServiceFailure.nativeFailure("unable to create Sherpa cancellation token")
                }
                activeRenderCancellation = cancellation
                rendered = try await SherpaSpeechRenderer.render(
                    text: segment,
                    modelID: modelID,
                    modelDirectory: modelDirectory,
                    speakerID: speakerID,
                    speed: configuration.speed,
                    maximumBytes: maximumBytes,
                    cancellationToken: cancellation
                )
                activeRenderCancellation = nil
            } catch {
                activeRenderCancellation = nil
                await complete(.interrupted)
                throw error
            }
            let outcome = await play(rendered.pcm, sampleRate: rendered.sampleRate)
            guard outcome == .completed else {
                await complete(.interrupted)
                return .interrupted
            }
        }
        await complete(.completed)
        return .completed
    }

    func stop() async {
        await complete(.interrupted)
    }

    private func play(_ data: Data, sampleRate: UInt32) async -> VoiceSpeechPlaybackOutcome {
        guard terminalOutcome == nil,
              let format = AVAudioFormat(
                  commonFormat: .pcmFormatFloat32,
                  sampleRate: Double(sampleRate),
                  channels: 1,
                  interleaved: false
              )
        else { return .interrupted }
        let frameCount = data.count / MemoryLayout<Int16>.size
        guard let buffer = AVAudioPCMBuffer(
            pcmFormat: format,
            frameCapacity: AVAudioFrameCount(frameCount)
        ), let output = buffer.floatChannelData?[0]
        else { return .interrupted }
        buffer.frameLength = AVAudioFrameCount(frameCount)
        data.withUnsafeBytes { raw in
            let input = raw.bindMemory(to: Int16.self)
            for index in 0 ..< frameCount {
                output[index] = Float(input[index]) / Float(Int16.max)
            }
        }
        if engine.isRunning { engine.stop() }
        engine.disconnectNodeOutput(player)
        engine.connect(player, to: engine.mainMixerNode, format: format)
        do {
            engine.prepare()
            try engine.start()
        } catch {
            return .interrupted
        }
        return await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                playbackContinuation = continuation
                player.scheduleBuffer(buffer, completionCallbackType: .dataPlayedBack) { [weak self] _ in
                    Task { @MainActor [weak self] in
                        self?.finishScheduledPlayback()
                    }
                }
                if !isPaused { player.play() }
            }
        } onCancel: {
            Task { @MainActor [weak self] in await self?.complete(.interrupted) }
        }
    }

    private func finishScheduledPlayback() {
        let continuation = playbackContinuation
        playbackContinuation = nil
        continuation?.resume(returning: terminalOutcome ?? .completed)
    }

    private func complete(_ outcome: VoiceSpeechPlaybackOutcome) async {
        guard terminalOutcome == nil else { return }
        terminalOutcome = outcome
        activeRenderCancellation?.cancel()
        activeRenderCancellation = nil
        queuedText.removeAll()
        player.stop()
        engine.stop()
        let continuation = playbackContinuation
        playbackContinuation = nil
        continuation?.resume(returning: outcome)
        if let audioLease {
            self.audioLease = nil
            await coordinator.release(audioLease)
        }
        if let modelReference {
            self.modelReference = nil
            VoiceModelStore.shared.releaseAudioUse(modelReference)
        }
        onTerminal?()
        onTerminal = nil
    }
}

final class SherpaAudioCancellationToken: @unchecked Sendable {
    let pointer: OpaquePointer

    init?() {
        guard let pointer = LXAudioCancellationTokenCreate() else { return nil }
        self.pointer = pointer
    }

    func cancel() {
        LXAudioCancellationTokenCancel(pointer)
    }

    deinit {
        LXAudioCancellationTokenDestroy(pointer)
    }
}
