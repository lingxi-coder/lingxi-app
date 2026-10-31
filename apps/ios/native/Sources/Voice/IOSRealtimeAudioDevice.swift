import AVFoundation
import Foundation

func isSupportedMediaPcmSampleRate(_ sampleRateHz: UInt32) -> Bool {
    (8_000 ... 192_000).contains(sampleRateHz)
}

struct IOSMediaPlaybackReceiptLedger {
    private struct GenerationReceipt {
        var acceptedSampleFrames: UInt64 = 0
        var playedSampleFrames: UInt64 = 0
        var renderedAtUnixMs: UInt64?
    }

    private var receipts: [UInt64: GenerationReceipt] = [:]
    private(set) var lastClaimedGeneration: UInt64?

    mutating func accept(_ sampleFrames: UInt32, generation: UInt64) {
        let current = receipts[generation]?.acceptedSampleFrames ?? 0
        var receipt = receipts[generation] ?? GenerationReceipt()
        receipt.acceptedSampleFrames = current.addingReportingOverflow(UInt64(sampleFrames)).overflow
            ? UInt64.max
            : current + UInt64(sampleFrames)
        receipts[generation] = receipt
    }

    mutating func recordPlayed(_ sampleFrames: UInt64, generation: UInt64, renderedAtUnixMs: UInt64?) {
        var receipt = receipts[generation] ?? GenerationReceipt()
        receipt.playedSampleFrames = receipt.playedSampleFrames.addingReportingOverflow(sampleFrames).overflow
            ? UInt64.max
            : receipt.playedSampleFrames + sampleFrames
        if let renderedAtUnixMs { receipt.renderedAtUnixMs = renderedAtUnixMs }
        receipts[generation] = receipt
    }

    mutating func take(generation: UInt64) -> MediaRenderReceiptDto {
        if let lastClaimedGeneration, generation <= lastClaimedGeneration {
            return MediaRenderReceiptDto(acceptedSampleFrames: 0, playedSampleFrames: 0, renderedAtUnixMs: nil)
        }
        lastClaimedGeneration = generation
        let generationReceipt = receipts.removeValue(forKey: generation) ?? GenerationReceipt()
        let receipt = MediaRenderReceiptDto(
            acceptedSampleFrames: 0,
            playedSampleFrames: UInt32(clamping: min(generationReceipt.playedSampleFrames, generationReceipt.acceptedSampleFrames)),
            renderedAtUnixMs: generationReceipt.playedSampleFrames > 0 ? generationReceipt.renderedAtUnixMs : nil
        )
        return receipt
    }
}

struct IOSRealtimeAudioInputFrame: Sendable {
    let pcm16le: Data
    let capturedAtUnixMs: UInt64
}

/// The coordinator rejected this attempt before invoking native activation.
struct IOSRealtimeAudioNotAdmitted: LocalizedError {
    let purpose: VoiceAudioSessionCoordinator.Purpose
    var errorDescription: String? {
        VoiceAudioSessionCoordinator.CoordinationError.busy(purpose).errorDescription
    }
}

/// Tracks native tap callbacks that have already entered when Stop begins.
final class IOSRealtimeAudioCallbackFence: @unchecked Sendable {
    private let lock = NSLock()
    private var closing = false
    private var active = 0
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func enter() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard !closing else { return false }
        active += 1
        return true
    }

    func leave() {
        lock.lock()
        active -= 1
        let ready = closing && active == 0 ? waiters : []
        if !ready.isEmpty { waiters.removeAll() }
        lock.unlock()
        ready.forEach { $0.resume() }
    }

    func close() {
        lock.lock()
        closing = true
        lock.unlock()
    }

    func wait() async {
        await withCheckedContinuation { continuation in
            lock.lock()
            if active == 0 {
                lock.unlock()
                continuation.resume()
            } else {
                waiters.append(continuation)
                lock.unlock()
            }
        }
    }
}

/// A single voice-processing engine owns microphone and speaker under the
/// same device lease. Its bounded input stream fails on overflow rather than
/// silently losing audio that the provider could turn into another transcript.
@MainActor
final class IOSRealtimeAudioDevice {
    private struct OutputItem {
        var pendingBuffers = 0
        var ended = false
        var durationMs: UInt64 = 0
        let itemID: String?
    }

    private let coordinator: VoiceAudioSessionCoordinator
    private var engine: AVAudioEngine?
    private var player: AVAudioPlayerNode?
    private var lease: VoiceAudioSessionCoordinator.Lease?
    private var inputFence: IOSRealtimeAudioCallbackFence?
    private var starting = false
    private var startWaiters: [CheckedContinuation<Void, Never>] = []
    private var stopRequested = false
    private var stopConfirmed = false
    private var stopTask: Task<Bool, Never>?
    private var inputContinuation: AsyncThrowingStream<IOSRealtimeAudioInputFrame, Error>.Continuation?
    private var queuedOutputBytes = 0
    private var scheduledOutputMs: UInt64 = 0
    private var itemStartPositionMs: [String: UInt64] = [:]
    private var outputItems: [String: OutputItem] = [:]
    private var playbackGeneration: UInt64 = 0
    private var outputSampleRateHz: UInt32 = 24_000
    private struct MediaWrite {
        let acceptedSampleFrames: UInt32
        let outputStartFrame: UInt64
        let outputFrameCount: UInt32
        let generation: UInt64
    }
    private var mediaWrites: [UUID: MediaWrite] = [:]
    private var mediaWriteOrder: [UUID] = []
    private var scheduledMediaOutputFrames: UInt64 = 0
    private var mediaReceiptLedger = IOSMediaPlaybackReceiptLedger()
    private var activeFlushGeneration: UInt64?
    private var playbackEnabled = true
    var onPlaybackCompleted: ((String?) -> Void)?

    init(coordinator: VoiceAudioSessionCoordinator = .shared) { self.coordinator = coordinator }

    func owns(_ event: VoiceAudioSessionCoordinator.Invalidation) async -> Bool {
        guard lease == event.lease else { return false }
        return await coordinator.owns(event.lease)
    }

    var playbackPositionMs: UInt64 {
        guard let player, let nodeTime = player.lastRenderTime, let time = player.playerTime(forNodeTime: nodeTime), time.sampleRate > 0 else { return 0 }
        return UInt64(max(0, Double(time.sampleTime) / time.sampleRate * 1_000))
    }

    var currentPlaybackItemID: String? {
        let position = playbackPositionMs
        let key = itemStartPositionMs.filter { $0.value <= position }.max { $0.value < $1.value }?.key
        return key.flatMap { outputItems[$0]?.itemID }
    }

    func start(inputSampleRateHz: UInt32 = 24_000, outputSampleRateHz: UInt32 = 24_000) async throws -> AsyncThrowingStream<Data, Error> {
        let frames = try await startTimestamped(inputSampleRateHz: inputSampleRateHz, outputSampleRateHz: outputSampleRateHz)
        return AsyncThrowingStream(bufferingPolicy: .bufferingNewest(32)) { continuation in
            let relay = Task {
                do {
                    for try await frame in frames {
                        if case .dropped = continuation.yield(frame.pcm16le) {
                            continuation.finish(throwing: AudioServiceFailure.mediaTooLarge)
                            return
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in relay.cancel() }
        }
    }

    func startTimestamped(
        inputSampleRateHz: UInt32 = 24_000,
        outputSampleRateHz: UInt32 = 24_000,
        audioSessionPurpose: VoiceAudioSessionCoordinator.Purpose = .flowDuplex
    ) async throws -> AsyncThrowingStream<IOSRealtimeAudioInputFrame, Error> {
        guard audioSessionPurpose == .flowDuplex || audioSessionPurpose == .recognition else {
            throw AudioServiceFailure.unsupported
        }
        let playbackEnabled = audioSessionPurpose == .flowDuplex
        guard engine == nil, lease == nil, !starting else { throw AudioServiceFailure.busy }
        guard [16_000, 24_000, 48_000].contains(inputSampleRateHz),
              !playbackEnabled || [16_000, 24_000, 48_000].contains(outputSampleRateHz) else {
            throw AudioServiceFailure.unsupported
        }
        if stopRequested {
            guard stopConfirmed else { throw AudioServiceFailure.busy }
            stopRequested = false
            stopConfirmed = false
            stopTask = nil
        }
        starting = true
        defer { finishStarting() }
        guard await AVAudioApplication.requestRecordPermission() else { throw AudioServiceFailure.permissionDenied }
        try Task.checkCancellation()
        guard !stopRequested else { throw AudioServiceFailure.cancelled }
        switch await coordinator.acquireForMedia(audioSessionPurpose) {
        case let .acquired(acquired):
            lease = acquired
        case let .notAdmitted(purpose):
            throw IOSRealtimeAudioNotAdmitted(purpose: purpose)
        case let .activationFailed(attempted, error):
            lease = attempted
            _ = await stopResources()
            throw error
        }
        do {
            try Task.checkCancellation()
            guard !stopRequested else { throw AudioServiceFailure.cancelled }
            let audio = AVAudioEngine()
            let input = audio.inputNode
            if playbackEnabled { try input.setVoiceProcessingEnabled(true) }
            let inputFormat = input.outputFormat(forBus: 0)
            guard inputFormat.sampleRate > 0,
                  let target = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: Double(inputSampleRateHz), channels: 1, interleaved: true),
                  let converter = AVAudioConverter(from: inputFormat, to: target)
            else { throw AudioServiceFailure.unavailable }
            let outputFormat: AVAudioFormat?
            if playbackEnabled {
                guard let format = AVAudioFormat(
                    commonFormat: .pcmFormatFloat32,
                    sampleRate: Double(outputSampleRateHz),
                    channels: 1,
                    interleaved: false
                ) else { throw AudioServiceFailure.unavailable }
                outputFormat = format
            } else {
                outputFormat = nil
            }
            let pair = AsyncThrowingStream<IOSRealtimeAudioInputFrame, Error>.makeStream(bufferingPolicy: .bufferingNewest(32))
            let continuation = pair.continuation
            inputContinuation = continuation
            let unixEpochAtHostClockZero = Date().timeIntervalSince1970 - ProcessInfo.processInfo.systemUptime
            let callbackFence = IOSRealtimeAudioCallbackFence()
            inputFence = callbackFence
            input.installTap(onBus: 0, bufferSize: 1_024, format: inputFormat) { buffer, when in
                guard callbackFence.enter() else { return }
                defer { callbackFence.leave() }
                // AVAudioEngine serializes this node's tap. The converter is
                // confined to that callback; only immutable Data crosses out.
                let capacity = AVAudioFrameCount(ceil(Double(buffer.frameLength) * target.sampleRate / inputFormat.sampleRate) + 32)
                guard let converted = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: capacity) else {
                    continuation.finish(throwing: AudioServiceFailure.nativeFailure("Realtime audio conversion allocation failed."))
                    return
                }
                var provided = false
                var error: NSError?
                let status = converter.convert(to: converted, error: &error) { _, state in
                    if provided { state.pointee = .noDataNow; return nil }
                    provided = true
                    state.pointee = .haveData
                    return buffer
                }
                guard status != .error, error == nil, let samples = converted.int16ChannelData?[0] else {
                    continuation.finish(throwing: AudioServiceFailure.nativeFailure(error?.localizedDescription ?? "Realtime audio conversion failed."))
                    return
                }
                guard converted.frameLength > 0 else { return }
                let bytes = Data(bytes: samples, count: Int(converted.frameLength) * 2)
                let captureTime = when.isHostTimeValid
                    ? AVAudioTime.seconds(forHostTime: when.hostTime) + unixEpochAtHostClockZero
                    : Date().timeIntervalSince1970
                let capturedAtUnixMs = UInt64(max(0, captureTime * 1_000))
                if case .dropped = continuation.yield(IOSRealtimeAudioInputFrame(
                    pcm16le: bytes,
                    capturedAtUnixMs: capturedAtUnixMs
                )) {
                    continuation.finish(throwing: AudioServiceFailure.mediaTooLarge)
                }
            }
            let speaker: AVAudioPlayerNode?
            if let outputFormat {
                let playerNode = AVAudioPlayerNode()
                audio.attach(playerNode)
                audio.connect(playerNode, to: audio.mainMixerNode, format: outputFormat)
                speaker = playerNode
            } else {
                speaker = nil
            }
            self.outputSampleRateHz = outputSampleRateHz
            self.playbackEnabled = playbackEnabled
            engine = audio
            player = speaker
            audio.prepare()
            try audio.start()
            speaker?.play()
            return pair.stream
        } catch {
            _ = await stopResources()
            throw error
        }
    }

    func enqueueOutput(pcm: Data, sampleRateHz: UInt32, itemID: String?) throws {
        guard playbackEnabled, let player, let engine, engine.isRunning else { throw AudioServiceFailure.unavailable }
        guard sampleRateHz == outputSampleRateHz, !pcm.isEmpty, pcm.count.isMultiple(of: 2) else { throw AudioServiceFailure.unsupported }
        guard pcm.count <= 1_024 * 1_024, queuedOutputBytes + pcm.count <= 4 * 1_024 * 1_024 else { throw AudioServiceFailure.mediaTooLarge }
        let frameCount = pcm.count / 2
        guard let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Double(sampleRateHz), channels: 1, interleaved: false),
              let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frameCount)),
              let samples = buffer.floatChannelData?[0] else { throw AudioServiceFailure.unavailable }
        buffer.frameLength = AVAudioFrameCount(frameCount)
        pcm.withUnsafeBytes { bytes in
            for index in 0 ..< frameCount {
                let value = bytes.loadUnaligned(fromByteOffset: index * 2, as: Int16.self)
                samples[index] = Float(Int16(littleEndian: value)) / 32_768
            }
        }
        let key = itemID ?? "__output__"
        guard outputItems[key] != nil || outputItems.count < 64,
              outputItems.values.reduce(0, { $0 + $1.pendingBuffers }) < 256 else { throw AudioServiceFailure.mediaTooLarge }
        let scheduledStart = max(playbackPositionMs, scheduledOutputMs)
        if itemStartPositionMs[key] == nil { itemStartPositionMs[key] = scheduledStart }
        scheduledOutputMs = scheduledStart + UInt64(frameCount) * 1_000 / UInt64(sampleRateHz)
        var item = outputItems[key] ?? OutputItem(itemID: itemID)
        guard !item.ended else { throw AudioServiceFailure.invalidRequest }
        item.pendingBuffers += 1
        item.durationMs += UInt64(frameCount) * 1_000 / UInt64(sampleRateHz)
        outputItems[key] = item
        queuedOutputBytes += pcm.count
        let generation = playbackGeneration
        player.scheduleBuffer(buffer, completionCallbackType: .dataPlayedBack) { [weak self] _ in
            Task { @MainActor [weak self] in
                guard let self, self.playbackGeneration == generation else { return }
                self.queuedOutputBytes -= pcm.count
                guard var item = self.outputItems[key] else { return }
                item.pendingBuffers -= 1
                self.outputItems[key] = item
                self.completeIfPlayed(key)
            }
        }
    }

    /// Queue one PCM16LE frame for a native media session. The immediate
    /// receipt reports acceptance only; `flushMediaOutput` or
    /// `interruptMediaOutput` reports frames that reached AVAudioPlayerNode's
    /// render timeline.
    func enqueueMediaOutput(pcm16le: Data, sampleRateHz: UInt32, channels: UInt16) throws -> MediaRenderReceiptDto {
        guard activeFlushGeneration != playbackGeneration else { throw AudioServiceFailure.busy }
        guard playbackEnabled, let player, let engine, engine.isRunning else { throw AudioServiceFailure.unavailable }
        guard isSupportedMediaPcmSampleRate(sampleRateHz), (1 ... 2).contains(channels),
              !pcm16le.isEmpty, pcm16le.count.isMultiple(of: Int(channels) * 2) else {
            throw AudioServiceFailure.unsupported
        }
        guard pcm16le.count <= 1_024 * 1_024, queuedOutputBytes + pcm16le.count <= 4 * 1_024 * 1_024 else {
            throw AudioServiceFailure.mediaTooLarge
        }

        let inputFrames = pcm16le.count / (Int(channels) * 2)
        let outputFrames = max(1, Int(ceil(Double(inputFrames) * Double(outputSampleRateHz) / Double(sampleRateHz))))
        guard outputFrames <= 2_097_152,
              let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Double(outputSampleRateHz), channels: 1, interleaved: false),
              let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(outputFrames)),
              let samples = buffer.floatChannelData?[0] else { throw AudioServiceFailure.unavailable }
        buffer.frameLength = AVAudioFrameCount(outputFrames)

        // Convert packed PCM16LE to the mono float format used by the shared
        // voice-processing engine. Linear resampling keeps this bounded and
        // preserves the exact accepted frame count for the receipt.
        pcm16le.withUnsafeBytes { rawBytes in
            for outputIndex in 0 ..< outputFrames {
                let sourcePosition = Double(outputIndex) * Double(sampleRateHz) / Double(outputSampleRateHz)
                let lowerIndex = min(Int(sourcePosition), inputFrames - 1)
                let upperIndex = min(lowerIndex + 1, inputFrames - 1)
                let fraction = Float(sourcePosition - Double(lowerIndex))
                func monoSample(at frame: Int) -> Float {
                    var total: Float = 0
                    for channel in 0 ..< Int(channels) {
                        let offset = (frame * Int(channels) + channel) * 2
                        let encoded = rawBytes.loadUnaligned(fromByteOffset: offset, as: Int16.self)
                        total += Float(Int16(littleEndian: encoded)) / 32_768
                    }
                    return total / Float(channels)
                }
                let lower = monoSample(at: lowerIndex)
                let upper = monoSample(at: upperIndex)
                samples[outputIndex] = lower + (upper - lower) * fraction
            }
        }

        guard mediaWrites.count < 128 else { throw AudioServiceFailure.mediaTooLarge }
        let identifier = UUID()
        let generation = playbackGeneration
        let outputStartFrame = scheduledMediaOutputFrames
        let outputFrameCount = UInt32(outputFrames)
        scheduledMediaOutputFrames += UInt64(outputFrameCount)
        let acceptedFrames = UInt32(clamping: inputFrames)
        mediaWrites[identifier] = MediaWrite(
            acceptedSampleFrames: acceptedFrames,
            outputStartFrame: outputStartFrame,
            outputFrameCount: outputFrameCount,
            generation: generation
        )
        mediaWriteOrder.append(identifier)
        mediaReceiptLedger.accept(acceptedFrames, generation: generation)
        queuedOutputBytes += pcm16le.count
        player.scheduleBuffer(buffer, completionCallbackType: .dataPlayedBack) { [weak self] _ in
            Task { @MainActor [weak self] in
                guard let self, self.playbackGeneration == generation,
                      let write = self.mediaWrites.removeValue(forKey: identifier) else { return }
                self.mediaWriteOrder.removeAll { $0 == identifier }
                self.queuedOutputBytes = max(0, self.queuedOutputBytes - pcm16le.count)
                self.mediaReceiptLedger.recordPlayed(
                    UInt64(write.acceptedSampleFrames),
                    generation: write.generation,
                    renderedAtUnixMs: Self.unixMillisecondsNow()
                )
            }
        }
        return MediaRenderReceiptDto(
            acceptedSampleFrames: acceptedFrames,
            playedSampleFrames: 0,
            renderedAtUnixMs: nil
        )
    }

    /// Wait until all queued media audio has actually completed in the native
    /// player, then return its measured completion receipt.
    func flushMediaOutput() async throws -> MediaRenderReceiptDto {
        guard playbackEnabled, player != nil else { throw AudioServiceFailure.unsupported }
        guard activeFlushGeneration == nil else { throw AudioServiceFailure.busy }
        let generation = playbackGeneration
        let targetWrites = Set(mediaWriteOrder.filter { mediaWrites[$0]?.generation == generation })
        activeFlushGeneration = generation
        defer {
            if activeFlushGeneration == generation { activeFlushGeneration = nil }
        }
        try Task.checkCancellation()
        while targetWrites.contains(where: { mediaWrites[$0] != nil }) {
            try Task.checkCancellation()
            try await Task.sleep(for: .milliseconds(5))
        }
        let receipt = takeMediaReceipt(generation: generation)
        if playbackGeneration == generation { playbackGeneration &+= 1 }
        return receipt
    }

    /// Interrupt queued media audio and report the portion already consumed
    /// according to the player's render time. No wall-clock duration is used.
    /// Interrupt synchronously on the main actor so it can preempt a flush
    /// that is suspended waiting for scheduled buffers to finish.
    func interruptMediaOutput() -> MediaRenderReceiptDto {
        let generation = playbackGeneration
        let interruptedWrites = mediaWriteOrder.filter { mediaWrites[$0]?.generation == generation }
        let renderedFrame = currentOutputSampleFrame
        playbackGeneration &+= 1
        player?.stop()
        player?.play()

        for identifier in interruptedWrites {
            guard let write = mediaWrites[identifier] else { continue }
            let playedOutputFrames: UInt64
            if renderedFrame <= write.outputStartFrame {
                playedOutputFrames = 0
            } else {
                playedOutputFrames = min(
                    renderedFrame - write.outputStartFrame,
                    UInt64(write.outputFrameCount)
                )
            }
            let ratio = Double(playedOutputFrames) / Double(max(1, write.outputFrameCount))
            mediaReceiptLedger.recordPlayed(
                UInt64((Double(write.acceptedSampleFrames) * ratio).rounded(.down)),
                generation: write.generation,
                renderedAtUnixMs: Self.unixMillisecondsNow()
            )
            mediaWrites.removeValue(forKey: identifier)
        }
        mediaWriteOrder.removeAll { interruptedWrites.contains($0) }
        queuedOutputBytes = 0
        scheduledMediaOutputFrames = 0
        outputItems.removeAll()
        itemStartPositionMs.removeAll()
        scheduledOutputMs = 0
        return takeMediaReceipt(generation: generation)
    }

    func setMediaPlaybackGain(_ gain: Float) throws {
        guard playbackEnabled, gain.isFinite, (0 ... 1).contains(gain), let engine else { throw AudioServiceFailure.invalidRequest }
        engine.mainMixerNode.outputVolume = gain
    }

    var hasPendingOutput: Bool { queuedOutputBytes > 0 || !outputItems.isEmpty }

    func markAllOutputCompleted() {
        guard !outputItems.isEmpty else { onPlaybackCompleted?(nil); return }
        let keys = Array(outputItems.keys)
        for key in keys {
            outputItems[key]?.ended = true
            completeIfPlayed(key)
        }
    }

    @discardableResult
    func interruptPlayback(itemID: String? = nil) -> UInt64 {
        let current = playbackPositionMs
        let start = itemStartPositionMs[itemID ?? "__output__"] ?? 0
        let duration = outputItems[itemID ?? "__output__"]?.durationMs ?? 0
        let position = min(current > start ? current - start : 0, duration)
        if !mediaWrites.isEmpty {
            _ = interruptMediaOutput()
            return position
        }
        playbackGeneration &+= 1
        player?.stop()
        player?.play()
        queuedOutputBytes = 0
        scheduledOutputMs = 0
        outputItems.removeAll()
        itemStartPositionMs.removeAll()
        return position
    }

    @discardableResult
    func stop() async -> Bool {
        stopRequested = true
        if let stopTask { return await stopTask.value }
        let task = Task { @MainActor [self] in
            await waitForStart()
            return await stopResources()
        }
        stopTask = task
        let confirmed = await task.value
        stopConfirmed = confirmed
        if !confirmed { stopTask = nil }
        return confirmed
    }

    private func finishStarting() {
        starting = false
        let waiters = startWaiters
        startWaiters.removeAll()
        waiters.forEach { $0.resume() }
    }

    private func waitForStart() async {
        guard starting else { return }
        await withCheckedContinuation { startWaiters.append($0) }
    }

    private func stopResources() async -> Bool {
        if mediaWrites.isEmpty {
            _ = takeMediaReceipt(generation: playbackGeneration)
        } else {
            _ = interruptMediaOutput()
        }
        playbackGeneration &+= 1
        inputContinuation?.finish()
        inputContinuation = nil
        let callbackFence = inputFence
        callbackFence?.close()
        engine?.inputNode.removeTap(onBus: 0)
        player?.stop()
        engine?.stop()
        engine = nil
        player = nil
        queuedOutputBytes = 0
        scheduledOutputMs = 0
        outputItems.removeAll()
        itemStartPositionMs.removeAll()
        scheduledMediaOutputFrames = 0
        if let callbackFence { await callbackFence.wait() }
        inputFence = nil
        if let lease {
            guard await coordinator.releaseWithReceipt(lease) else { return false }
            self.lease = nil
        }
        return true
    }

    private var currentOutputSampleFrame: UInt64 {
        guard let player, let nodeTime = player.lastRenderTime,
              let playerTime = player.playerTime(forNodeTime: nodeTime),
              playerTime.sampleTime >= 0 else { return 0 }
        return UInt64(playerTime.sampleTime)
    }

    private func takeMediaReceipt(generation: UInt64) -> MediaRenderReceiptDto {
        mediaReceiptLedger.take(generation: generation)
    }

    private static func unixMillisecondsNow() -> UInt64 {
        UInt64(max(0, Date().timeIntervalSince1970 * 1_000))
    }

    private func completeIfPlayed(_ key: String) {
        guard let item = outputItems[key], item.ended, item.pendingBuffers == 0 else { return }
        outputItems.removeValue(forKey: key)
        onPlaybackCompleted?(item.itemID)
    }
}
