import AVFoundation

/// Serializes microphone, recognition and speech playback ownership so Flow
/// Mode, hold-to-talk and engine tool callbacks cannot fight over AVAudioSession.
actor VoiceAudioSessionCoordinator {
    enum Purpose: Equatable {
        case recognition
        case recording
        case playback
    }

    enum CoordinationError: LocalizedError {
        case busy(Purpose)

        var errorDescription: String? {
            switch self {
            case .busy(let owner):
                return "音频会话正由 \(owner) 使用"
            }
        }
    }

    static let shared = VoiceAudioSessionCoordinator()
    private enum State: Equatable {
        case idle
        case active(Purpose)
        case interrupted(Purpose)
        case backgrounded(Purpose)
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
            Task { await self?.handleInterruption(note) }
        })
        observers.append(center.addObserver(
            forName: AVAudioSession.routeChangeNotification,
            object: AVAudioSession.sharedInstance(),
            queue: nil
        ) { [weak self] note in
            Task { await self?.handleRouteChange(note) }
        })
    }

    deinit {
        observers.forEach(NotificationCenter.default.removeObserver)
    }

    func activate(_ requested: Purpose) throws {
        if let currentPurpose {
            throw CoordinationError.busy(currentPurpose)
        }
        try configureAndActivate(requested)
        state = .active(requested)
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
        }
        try session.setActive(true, options: .notifyOthersOnDeactivation)
    }

    func deactivate(_ owner: Purpose) {
        guard currentPurpose == owner else { return }
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        state = .idle
    }

    func suspendForBackground() {
        guard case .active(let owner) = state else { return }
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        state = .backgrounded(owner)
    }

    func resumeAfterForeground() {
        guard case .backgrounded(let owner) = state else { return }
        do {
            try configureAndActivate(owner)
            state = .active(owner)
        } catch {
            state = .idle
        }
    }

    private var currentPurpose: Purpose? {
        switch state {
        case .active(let owner), .interrupted(let owner), .backgrounded(let owner): return owner
        case .idle: return nil
        }
    }

    private func handleInterruption(_ notification: Notification) {
        guard
            let raw = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
            let type = AVAudioSession.InterruptionType(rawValue: raw)
        else { return }
        switch type {
        case .began:
            if case .active(let owner) = state { state = .interrupted(owner) }
        case .ended:
            guard case .interrupted(let owner) = state else { return }
            let rawOptions = notification.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
            if AVAudioSession.InterruptionOptions(rawValue: rawOptions).contains(.shouldResume) {
                do {
                    try configureAndActivate(owner)
                    state = .active(owner)
                } catch {
                    state = .idle
                }
            } else {
                state = .idle
            }
        @unknown default:
            state = .idle
        }
    }

    private func handleRouteChange(_ notification: Notification) {
        guard case .active(let owner) = state else { return }
        guard
            let raw = notification.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt,
            let reason = AVAudioSession.RouteChangeReason(rawValue: raw),
            reason == .oldDeviceUnavailable || reason == .newDeviceAvailable
        else { return }
        // Reapply the purpose-specific category so Bluetooth HFP and speaker
        // routes recover consistently after headsets connect or disconnect.
        do {
            try configureAndActivate(owner)
            state = .active(owner)
        } catch {
            state = .idle
        }
    }
}
