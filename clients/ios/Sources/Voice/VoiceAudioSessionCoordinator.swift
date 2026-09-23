import AVFoundation

protocol VoiceAudioSessionDriving: Sendable {
    func activate(_ purpose: VoiceAudioSessionCoordinator.Purpose) throws
    func deactivate()
}

struct AVAudioSessionDriver: VoiceAudioSessionDriving {
    func activate(_ purpose: VoiceAudioSessionCoordinator.Purpose) throws {
        let session = AVAudioSession.sharedInstance()
        switch purpose {
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

    func deactivate() {
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }
}

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

    enum InvalidationReason: Sendable {
        case interruption
        case background
        case routeChange
    }

    struct Invalidation: Sendable {
        let lease: Lease
        let reason: InvalidationReason
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
        case invalidated(Lease)
    }

    private let sessionDriver: any VoiceAudioSessionDriving
    private var invalidationContinuations: [UUID: AsyncStream<Invalidation>.Continuation] = [:]
    private var state: State = .idle
    private nonisolated(unsafe) var observers: [NSObjectProtocol] = []

    init(sessionDriver: any VoiceAudioSessionDriving = AVAudioSessionDriver()) {
        self.sessionDriver = sessionDriver
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
        try sessionDriver.activate(requested)
        let lease = Lease(id: UUID(), purpose: requested)
        state = .active(lease)
        return lease
    }

    func release(_ lease: Lease) {
        guard currentLease == lease else { return }
        if case .active = state { sessionDriver.deactivate() }
        state = .idle
    }

    func suspendForBackground() {
        guard case .active(let lease) = state else { return }
        sessionDriver.deactivate()
        state = .invalidated(lease)
        publishInvalidation(.init(lease: lease, reason: .background))
    }

    func resumeAfterForeground() {}

    func invalidationEvents() -> AsyncStream<Invalidation> {
        let id = UUID()
        let (stream, continuation) = AsyncStream.makeStream(
            of: Invalidation.self,
            bufferingPolicy: .bufferingNewest(16)
        )
        invalidationContinuations[id] = continuation
        continuation.onTermination = { [weak self] _ in
            Task { await self?.removeInvalidationSubscriber(id) }
        }
        return stream
    }

    func invalidationSubscriberCount() -> Int {
        invalidationContinuations.count
    }

    private var currentLease: Lease? {
        switch state {
        case .active(let lease), .invalidated(let lease): return lease
        case .idle: return nil
        }
    }

    func owns(_ lease: Lease) -> Bool {
        currentLease == lease
    }

    func activePurpose() -> Purpose? {
        currentLease?.purpose
    }

    func leaseStateForDiagnostics() -> (purpose: Purpose?, awaitingOwnerCleanup: Bool) {
        switch state {
        case .idle: (nil, false)
        case .active(let lease): (lease.purpose, false)
        case .invalidated(let lease): (lease.purpose, true)
        }
    }

    func handleInterruption(rawType: UInt, rawOptions _: UInt) {
        guard let type = AVAudioSession.InterruptionType(rawValue: rawType) else { return }
        switch type {
        case .began:
            guard case .active(let lease) = state else { return }
            sessionDriver.deactivate()
            state = .invalidated(lease)
            publishInvalidation(.init(lease: lease, reason: .interruption))
        case .ended:
            // The interrupted operation ended when the system took the audio
            // session. It must reacquire as a new operation after the user
            // explicitly starts it again.
            break
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
        sessionDriver.deactivate()
        state = .invalidated(lease)
        publishInvalidation(.init(lease: lease, reason: .routeChange))
    }

    private func publishInvalidation(_ invalidation: Invalidation) {
        invalidationContinuations.values.forEach { $0.yield(invalidation) }
    }

    private func removeInvalidationSubscriber(_ id: UUID) {
        invalidationContinuations.removeValue(forKey: id)
    }
}
