import Foundation

/// Bounded, segmented playback through an immutable Rust provider route.
/// It deliberately exposes no duplex or provider streaming capability.
@MainActor
final class IOSCloudSpeechStream: VoiceSpeechStreamingSession {
    private let cloud: IOSAudioProviderService.PinnedOperation
    private let maximumBytes: UInt64
    private let playback: any IOSPcmPlaybackDriving
    private let onTerminal: () -> Void
    private var activeOperation: IOSAudioProviderService.PinnedOperation?
    private var segments: [String] = []
    private var textBytes = 0
    private var overflow = false
    private var stopped = false
    private var paused = false
    private var finished = false

    init(cloud: IOSAudioProviderService.PinnedOperation, maximumBytes: UInt64, playback: any IOSPcmPlaybackDriving, onTerminal: @escaping () -> Void) {
        self.cloud = cloud
        self.maximumBytes = maximumBytes
        self.playback = playback
        self.onTerminal = onTerminal
    }

    func enqueue(_ text: String) {
        guard !stopped, !finished else { return }
        let next = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !next.isEmpty else { return }
        guard textBytes + next.utf8.count <= 128 * 1_024, segments.count < 256 else { overflow = true; return }
        textBytes += next.utf8.count
        segments.append(next)
    }

    func pause() { paused = true; playback.pause() }
    func resume() { paused = false; playback.resume() }

    func finish() async throws -> VoiceSpeechPlaybackOutcome {
        guard !finished else { return stopped ? .interrupted : .completed }
        defer { terminate() }
        for text in segments {
            guard !stopped else { return .interrupted }
            try Task.checkCancellation()
            if overflow { throw AudioServiceFailure.mediaTooLarge }
            while paused, !stopped { try await Task.sleep(for: .milliseconds(40)) }
            guard !stopped else { return .interrupted }
            let id = UUID().uuidString.lowercased()
            guard var request = try JSONSerialization.jsonObject(with: Data(cloud.requestJSON.utf8)) as? [String: Any] else {
                throw AudioServiceFailure.invalidRequest
            }
            request["operationId"] = id
            let operation = IOSAudioProviderService.PinnedOperation(
                id: id, host: cloud.host,
                requestJSON: String(decoding: try JSONSerialization.data(withJSONObject: request, options: .sortedKeys), as: UTF8.self),
                profileID: cloud.profileID, modelID: cloud.modelID
            )
            activeOperation = operation
            let output = try await IOSAudioProviderService.shared.synthesize(operation, text: text, maximumBytes: maximumBytes)
            activeOperation = nil
            guard !stopped else { return .interrupted }
            while paused, !stopped { try await Task.sleep(for: .milliseconds(40)) }
            guard !stopped else { return .interrupted }
            _ = try await playback.play(pcm: output.pcm, sampleRateHz: output.sampleRateHz, maximumBytes: maximumBytes)
        }
        if overflow { throw AudioServiceFailure.mediaTooLarge }
        return stopped ? .interrupted : .completed
    }

    func stop() async {
        stopped = true
        playback.stop()
        if let activeOperation { try? await activeOperation.host.cancel(operationId: activeOperation.id) }
        terminate()
    }

    private func terminate() {
        guard !finished else { return }
        finished = true
        segments.removeAll()
        onTerminal()
    }
}
