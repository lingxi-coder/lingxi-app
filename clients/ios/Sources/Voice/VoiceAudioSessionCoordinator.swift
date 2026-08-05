import AVFoundation

/// Serializes microphone, recognition and speech playback ownership so Flow
/// Mode, hold-to-talk and engine tool callbacks cannot fight over AVAudioSession.
actor VoiceAudioSessionCoordinator {
    enum Purpose: Equatable, Sendable {
        case recognition
        case recording
        case playback
        /// Flow Mode owns input monitoring and speech output simultaneously.
        /// The monitor enables voice processing on its AVAudioEngine I/O node.
        case flowDuplex
    }

    struct Lease: Equatable, Sendable {
        fileprivate let id: UUID
        let purpose: Purpose
    }

    enum CoordinationError: LocalizedError {
        case busy(Purpose)

        var errorDescription: String? {
            switch self {
            case .busy(let owner):
                return String(localized: "voice_audio_session_busy \(String(describing: owner))")
            }
        }
    }

    static let shared = VoiceAudioSessionCoordinator()
    private enum State: Equatable {
        case idle
        case active(Lease)
        case interrupted(Lease)
        case backgrounded(Lease)
    }

    private var state: State = .idle
    private nonisolated(unsafe) var observers: [NSObjectProtocol] = []

    init() {
        let center = NotificationCenter.default
        observers.append(center.addObserver(
            forName: AVAudioSession.interruptionNotification,
            object: AVAudioSession.sharedInstance(),
            queue: nil
        ) { [weak self] note in
            guard let rawType = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt else {
                return
            }
            let rawOptions = note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
            Task { await self?.handleInterruption(rawType: rawType, rawOptions: rawOptions) }
        })
        observers.append(center.addObserver(
            forName: AVAudioSession.routeChangeNotification,
            object: AVAudioSession.sharedInstance(),
            queue: nil
        ) { [weak self] note in
            guard let rawReason = note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt else {
                return
            }
            Task { await self?.handleRouteChange(rawReason: rawReason) }
        })
    }

    deinit {
        observers.forEach(NotificationCenter.default.removeObserver)
    }

    func acquire(_ requested: Purpose) throws -> Lease {
        if let currentLease {
            throw CoordinationError.busy(currentLease.purpose)
        }
        try configureAndActivate(requested)
        let lease = Lease(id: UUID(), purpose: requested)
        state = .active(lease)
        return lease
    }

    private func configureAndActivate(_ requested: Purpose) throws {
        let session = AVAudioSession.sharedInstance()
        switch requested {
        case .recognition:
            try session.setCategory(.record, mode: .measurement, options: [.duckOthers, .allowBluetoothHFP])
        case .recording:
            try session.setCategory(.playAndRecord, mode: .spokenAudio, options: [.defaultToSpeaker, .allowBluetoothHFP])
        case .playback:
            try session.setCategory(.playback, mode: .spokenAudio, options: [.duckOthers])
        case .flowDuplex:
            try session.setCategory(
                .playAndRecord,
                mode: .voiceChat,
                options: [.defaultToSpeaker, .allowBluetoothHFP]
            )
        }
        try session.setActive(true, options: .notifyOthersOnDeactivation)
    }

    func release(_ lease: Lease) {
        guard currentLease == lease else { return }
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        state = .idle
    }

    func suspendForBackground() {
        guard case .active(let lease) = state else { return }
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        state = .backgrounded(lease)
    }

    func resumeAfterForeground() {
        guard case .backgrounded(let lease) = state else { return }
        do {
            try configureAndActivate(lease.purpose)
            state = .active(lease)
        } catch {
            // The caller still owns the lease even when reactivation fails.
            // Only `release` may make the coordinator idle; otherwise a second
            // microphone or synthesizer could start over the stale owner.
            state = .backgrounded(lease)
        }
    }

    private var currentLease: Lease? {
        switch state {
        case .active(let lease), .interrupted(let lease), .backgrounded(let lease): return lease
        case .idle: return nil
        }
    }

    private func handleInterruption(rawType: UInt, rawOptions: UInt) {
        guard let type = AVAudioSession.InterruptionType(rawValue: rawType) else { return }
        switch type {
        case .began:
            if case .active(let lease) = state { state = .interrupted(lease) }
        case .ended:
            guard case .interrupted(let lease) = state else { return }
            if AVAudioSession.InterruptionOptions(rawValue: rawOptions).contains(.shouldResume) {
                do {
                    try configureAndActivate(lease.purpose)
                    state = .active(lease)
                } catch {
                    state = .interrupted(lease)
                }
            } else {
                state = .interrupted(lease)
            }
        @unknown default:
            break
        }
    }

    private func handleRouteChange(rawReason: UInt) {
        guard case .active(let lease) = state else { return }
        guard
            let reason = AVAudioSession.RouteChangeReason(rawValue: rawReason),
            reason == .oldDeviceUnavailable || reason == .newDeviceAvailable
        else { return }
        // Reapply the purpose-specific category so Bluetooth HFP and speaker
        // routes recover consistently after headsets connect or disconnect.
        do {
            try configureAndActivate(lease.purpose)
            state = .active(lease)
        } catch {
            state = .interrupted(lease)
        }
    }
}
