import AVFoundation
import Foundation

@MainActor
protocol IOSPcmPlaybackDriving: AnyObject {
    var isPlaying: Bool { get }
    var positionMs: UInt64 { get }
    func play(pcm: Data, sampleRateHz: UInt32, maximumBytes: UInt64) async throws -> UInt64
    func stop()
    func pause()
    func resume()
}

/// Device playback only. Provider protocols and credentials remain in Rust.
@MainActor
final class IOSPcmPlayback: NSObject, AVAudioPlayerDelegate, IOSPcmPlaybackDriving {
    private let coordinator: VoiceAudioSessionCoordinator
    private var player: AVAudioPlayer?
    private var continuation: CheckedContinuation<UInt64, Error>?
    private var lease: VoiceAudioSessionCoordinator.Lease?
    private var playbackID: UUID?

    init(coordinator: VoiceAudioSessionCoordinator) {
        self.coordinator = coordinator
    }

    var isPlaying: Bool { player != nil }
    var positionMs: UInt64 { UInt64(max(0, (player?.currentTime ?? 0) * 1_000)) }

    static func wave(pcm: Data, sampleRateHz: UInt32, maximumBytes: UInt64) throws -> Data {
        guard !pcm.isEmpty, pcm.count.isMultiple(of: 2),
              [8_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000].contains(sampleRateHz)
        else { throw AudioServiceFailure.invalidRequest }
        guard UInt64(pcm.count) <= maximumBytes, pcm.count <= Int(UInt32.max) - 36 else {
            throw AudioServiceFailure.mediaTooLarge
        }
        var wav = Data()
        func uint16(_ value: UInt16) { var value = value.littleEndian; withUnsafeBytes(of: &value) { wav.append(contentsOf: $0) } }
        func uint32(_ value: UInt32) { var value = value.littleEndian; withUnsafeBytes(of: &value) { wav.append(contentsOf: $0) } }
        wav.append(Data("RIFF".utf8)); uint32(UInt32(pcm.count) + 36)
        wav.append(Data("WAVEfmt ".utf8)); uint32(16); uint16(1); uint16(1)
        uint32(sampleRateHz); uint32(sampleRateHz * 2); uint16(2); uint16(16)
        wav.append(Data("data".utf8)); uint32(UInt32(pcm.count)); wav.append(pcm)
        return wav
    }

    func play(pcm: Data, sampleRateHz: UInt32, maximumBytes: UInt64) async throws -> UInt64 {
        guard player == nil, playbackID == nil else { throw AudioServiceFailure.busy }
        let wav = try Self.wave(pcm: pcm, sampleRateHz: sampleRateHz, maximumBytes: maximumBytes)
        let identity = UUID()
        playbackID = identity
        do {
            let acquired = try await coordinator.acquire(.playback)
            guard playbackID == identity else {
                await coordinator.release(acquired)
                throw CancellationError()
            }
            lease = acquired
            try Task.checkCancellation()
            let audio = try AVAudioPlayer(data: wav, fileTypeHint: AVFileType.wav.rawValue)
            audio.delegate = self
            player = audio
            return try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { waiter in
                    continuation = waiter
                    guard audio.play() else {
                        finish(.failure(AudioServiceFailure.nativeFailure("PCM playback could not start.")))
                        return
                    }
                }
            } onCancel: {
                Task { @MainActor [weak self] in
                    guard self?.playbackID == identity else { return }
                    self?.stop()
                }
            }
        } catch {
            let failure: Error = error is VoiceAudioSessionCoordinator.CoordinationError ? AudioServiceFailure.busy : error
            if playbackID == identity { finish(.failure(failure)) }
            throw failure
        }
    }

    func pause() { player?.pause() }
    func resume() { player?.play() }

    func stop() { finish(.failure(CancellationError())) }

    nonisolated func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        Task { @MainActor [weak self] in
            guard let self, self.player === player else { return }
            let duration = UInt64(max(0, player.duration * 1_000))
            self.finish(flag ? .success(duration) : .failure(AudioServiceFailure.nativeFailure("PCM playback was interrupted.")))
        }
    }

    nonisolated func audioPlayerDecodeErrorDidOccur(_ player: AVAudioPlayer, error: Error?) {
        Task { @MainActor [weak self] in
            guard let self, self.player === player else { return }
            self.finish(.failure(AudioServiceFailure.nativeFailure(error?.localizedDescription ?? "PCM decoding failed.")))
        }
    }

    private func finish(_ result: Result<UInt64, Error>) {
        let waiter = continuation
        continuation = nil
        player?.stop()
        player = nil
        playbackID = nil
        let oldLease = lease
        lease = nil
        // Lease cleanup completes before releasing the operation's caller.
        Task { @MainActor [coordinator] in
            if let oldLease { await coordinator.release(oldLease) }
            waiter?.resume(with: result)
        }
    }
}
