import AVFoundation
import Foundation

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
    private var inputContinuation: AsyncThrowingStream<Data, Error>.Continuation?
    private var queuedOutputBytes = 0
    private var scheduledOutputMs: UInt64 = 0
    private var itemStartPositionMs: [String: UInt64] = [:]
    private var outputItems: [String: OutputItem] = [:]
    private var playbackGeneration: UInt64 = 0
    private var outputSampleRateHz: UInt32 = 24_000
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
        guard engine == nil, lease == nil else { throw AudioServiceFailure.busy }
        guard [16_000, 24_000, 48_000].contains(inputSampleRateHz), [16_000, 24_000, 48_000].contains(outputSampleRateHz) else {
            throw AudioServiceFailure.unsupported
        }
        guard await AVAudioApplication.requestRecordPermission() else { throw AudioServiceFailure.permissionDenied }
        try Task.checkCancellation()
        let acquired = try await coordinator.acquire(.flowDuplex)
        lease = acquired
        do {
            try Task.checkCancellation()
            let audio = AVAudioEngine()
            let input = audio.inputNode
            try input.setVoiceProcessingEnabled(true)
            let inputFormat = input.outputFormat(forBus: 0)
            guard inputFormat.sampleRate > 0,
                  let target = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: Double(inputSampleRateHz), channels: 1, interleaved: true),
                  let converter = AVAudioConverter(from: inputFormat, to: target),
                  let outputFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Double(outputSampleRateHz), channels: 1, interleaved: false)
            else { throw AudioServiceFailure.unavailable }
            let pair = AsyncThrowingStream<Data, Error>.makeStream(bufferingPolicy: .bufferingNewest(32))
            let continuation = pair.continuation
            inputContinuation = continuation
            input.installTap(onBus: 0, bufferSize: 1_024, format: inputFormat) { buffer, _ in
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
                if case .dropped = continuation.yield(bytes) {
                    continuation.finish(throwing: AudioServiceFailure.mediaTooLarge)
                }
            }
            let speaker = AVAudioPlayerNode()
            audio.attach(speaker)
            audio.connect(speaker, to: audio.mainMixerNode, format: outputFormat)
            self.outputSampleRateHz = outputSampleRateHz
            engine = audio
            player = speaker
            audio.prepare()
            try audio.start()
            speaker.play()
            return pair.stream
        } catch {
            await stop()
            throw error
        }
    }

    func enqueueOutput(pcm: Data, sampleRateHz: UInt32, itemID: String?) throws {
        guard let player, let engine, engine.isRunning else { throw AudioServiceFailure.unavailable }
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
        playbackGeneration &+= 1
        player?.stop()
        player?.play()
        queuedOutputBytes = 0
        scheduledOutputMs = 0
        outputItems.removeAll()
        itemStartPositionMs.removeAll()
        return position
    }

    func stop() async {
        playbackGeneration &+= 1
        inputContinuation?.finish()
        inputContinuation = nil
        engine?.inputNode.removeTap(onBus: 0)
        player?.stop()
        engine?.stop()
        engine = nil
        player = nil
        queuedOutputBytes = 0
        scheduledOutputMs = 0
        outputItems.removeAll()
        itemStartPositionMs.removeAll()
        if let lease { await coordinator.release(lease) }
        lease = nil
    }

    private func completeIfPlayed(_ key: String) {
        guard let item = outputItems[key], item.ended, item.pendingBuffers == 0 else { return }
        outputItems.removeValue(forKey: key)
        onPlaybackCompleted?(item.itemID)
    }
}
