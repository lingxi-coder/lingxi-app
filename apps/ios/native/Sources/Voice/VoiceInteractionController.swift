import AVFoundation
import Observation
import OSLog
import SwiftUI

#if canImport(UIKit)
    import UIKit
#endif

enum VoiceInteractionMode: Equatable {
    case dictation
    case flow
}

enum VoiceInteractionPhase: Equatable {
    case requestingPermission
    case configurationRequired
    case listening
    case recognizing
    case thinking
    case speaking
    case interrupting
    case paused
    case failed
}

struct VoiceSpeechRequest: Equatable {
    let text: String
    let voiceIdentifier: String
    let languageIdentifier: String
    let speed: Double
    var route: AudioRouteResolution? = nil
    var maxPayloadBytes: UInt64? = nil
    var configurationRevision: UInt64? = nil
    var configurationSnapshot: AudioConfigurationSnapshot? = nil
    var audioOwner: IOSAudioOwner? = nil
}

struct VoiceSpeechConfiguration: Equatable {
    let voiceIdentifier: String
    let languageIdentifier: String
    let speed: Double
    var route: AudioRouteResolution? = nil
    var maxPayloadBytes: UInt64? = nil
    var configurationRevision: UInt64? = nil
    var configurationSnapshot: AudioConfigurationSnapshot? = nil
    var audioOwner: IOSAudioOwner? = nil


}

enum VoiceSpeechPlaybackOutcome: Equatable {
    case completed
    case interrupted
}

@MainActor
protocol VoiceSpeechStreamingSession: AnyObject {
    func enqueue(_ text: String)
    func pause()
    func resume()
    func finish() async throws -> VoiceSpeechPlaybackOutcome
    func stop() async
}

@MainActor
protocol VoiceSpeechPlaying: AnyObject {
    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome
    func stop()
    func stopAndWait() async
    /// Stop only ordinary automatic speech, with a native cleanup receipt.
    func stopAutomaticPlaybackAndWait() async -> Bool
    func openStream(
        configuration: VoiceSpeechConfiguration,
        managesAudioSession: Bool
    ) async throws -> any VoiceSpeechStreamingSession
}

extension VoiceSpeechPlaying {
    func stopAndWait() async { stop() }
    func stopAutomaticPlaybackAndWait() async -> Bool { await stopAndWait(); return true }
}

@MainActor
final class SystemVoiceSpeechPlayer: VoiceSpeechPlaying {
    private let coordinator: VoiceAudioSessionCoordinator
    private let modelRoot: (GeneratedOfflineModelEntry) -> URL?
    private var activeStream: (any VoiceSpeechStreamingSession)?
    private var activeStreamID: ObjectIdentifier?

    init(
        coordinator: VoiceAudioSessionCoordinator? = nil,
        modelRoot: @escaping (GeneratedOfflineModelEntry) -> URL? = VoiceModelFiles.modelRoot
    ) {
        self.coordinator = coordinator ?? .shared
        self.modelRoot = modelRoot
    }

    var hasActivePlayback: Bool { activeStream != nil }

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        let stream = try await openStream(
            configuration: VoiceSpeechConfiguration(
                voiceIdentifier: request.voiceIdentifier,
                languageIdentifier: request.languageIdentifier,
                speed: request.speed,
                route: request.route,
                maxPayloadBytes: request.maxPayloadBytes,
                configurationRevision: request.configurationRevision,
                configurationSnapshot: request.configurationSnapshot,
                audioOwner: request.audioOwner
            ),
            managesAudioSession: true
        )
        stream.enqueue(request.text)
        return try await stream.finish()
    }

    func stop() {
        guard let activeStream else { return }
        self.activeStream = nil
        activeStreamID = nil
        Task { await activeStream.stop() }
    }

    func stopAndWait() async {
        guard let activeStream else { return }
        self.activeStream = nil
        activeStreamID = nil
        await activeStream.stop()
    }

    func openStream(
        configuration: VoiceSpeechConfiguration,
        managesAudioSession: Bool
    ) async throws -> any VoiceSpeechStreamingSession {
        try Task.checkCancellation()
        if let activeStream {
            await activeStream.stop()
            try Task.checkCancellation()
        }
        var lease: VoiceAudioSessionCoordinator.Lease?
        let routedVoiceIdentifier: String
        if let route = configuration.route {
            guard route.status == .ready, let effective = route.effective else {
                throw AudioServiceFailure.unavailable
            }
            switch effective.source {
            case .system:
                routedVoiceIdentifier = effective.voiceId ?? ""
                lease = try await acquireLeaseIfNeeded(managesAudioSession)
            case .offline:
                guard let modelID = effective.modelId,
                      let model = GeneratedVoiceModelCatalog.byID(modelID),
                      model.kind == .tts,
                      let directory = modelRoot(model)
                else { throw AudioServiceFailure.modelMissing }
                guard let voiceID = Self.resolvedOfflineVoiceID(for: effective.voiceId, model: model) else {
                    throw AudioServiceFailure.voiceMissing
                }
                guard let voiceIndex = model.voices.firstIndex(where: { $0.id == voiceID }) else {
                    throw AudioServiceFailure.voiceMissing
                }
                lease = try await acquireLeaseIfNeeded(managesAudioSession)
                let modelReference = VoiceModelStore.shared.retainForAudioUse(modelID)
                let stream = SherpaVoiceSpeechStream(
                    configuration: VoiceSpeechConfiguration(
                        voiceIdentifier: "offline:\(modelID):\(voiceID)",
                        languageIdentifier: configuration.languageIdentifier,
                        speed: configuration.speed,
                        route: route,
                        maxPayloadBytes: configuration.maxPayloadBytes,
                        configurationRevision: configuration.configurationRevision
                    ),
                    modelID: model.id,
                    modelDirectory: directory,
                    speakerID: Int32(voiceIndex),
                    audioLease: lease,
                    modelReference: modelReference,
                    coordinator: coordinator
                )
                let streamID = ObjectIdentifier(stream)
                stream.onTerminal = { [weak self] in
                    guard self?.activeStreamID == streamID else { return }
                    self?.activeStream = nil
                    self?.activeStreamID = nil
                }
                activeStream = stream
                activeStreamID = streamID
                return stream
            default:
                throw AudioServiceFailure.unavailable
            }
        } else { throw AudioServiceFailure.unavailable }
        let systemConfiguration = VoiceSpeechConfiguration(
            voiceIdentifier: routedVoiceIdentifier,
            languageIdentifier: configuration.languageIdentifier,
            speed: configuration.speed,
            route: configuration.route,
            maxPayloadBytes: configuration.maxPayloadBytes,
            configurationRevision: configuration.configurationRevision
        )
        let stream = SystemVoiceSpeechStream(
            configuration: systemConfiguration,
            audioLease: lease,
            coordinator: coordinator
        )
        let streamID = ObjectIdentifier(stream)
        stream.onTerminal = { [weak self] in
            guard self?.activeStreamID == streamID else { return }
            self?.activeStream = nil
            self?.activeStreamID = nil
        }
        activeStream = stream
        activeStreamID = streamID
        return stream
    }

    static func resolvedOfflineVoiceID(
        for requestedVoiceID: String?,
        model: GeneratedOfflineModelEntry
    ) -> String? {
        guard model.kind == .tts else { return nil }
        let voiceID = requestedVoiceID ?? model.voices.first?.id
        guard let voiceID, model.voices.contains(where: { $0.id == voiceID }) else { return nil }
        return voiceID
    }

    private func acquireLeaseIfNeeded(_ managesAudioSession: Bool) async throws -> VoiceAudioSessionCoordinator.Lease? {
        guard managesAudioSession else { return nil }
        let lease = try await coordinator.acquire(.playback)
        do {
            try Task.checkCancellation()
            return lease
        } catch {
            await coordinator.release(lease)
            throw error
        }
    }
}

@MainActor
final class SystemVoiceSpeechStream: NSObject, VoiceSpeechStreamingSession, AVSpeechSynthesizerDelegate {
    private let configuration: VoiceSpeechConfiguration
    private let coordinator: VoiceAudioSessionCoordinator
    private let synthesizer = AVSpeechSynthesizer()
    private var audioLease: VoiceAudioSessionCoordinator.Lease?
    private var queuedText: [String] = []
    private var activeUtterance: AVSpeechUtterance?
    private var finishContinuations: [CheckedContinuation<VoiceSpeechPlaybackOutcome, Error>] = []
    private var terminalOutcome: VoiceSpeechPlaybackOutcome?
    private var isCompleting = false
    private var isFinishing = false
    private var isPaused = false
    private var isStopping = false
    // Notification tokens are only mutated on the main actor; deinit itself is
    // nonisolated under Swift 6, so expose this teardown-only storage explicitly.
    private nonisolated(unsafe) var observers: [NSObjectProtocol] = []
    var onTerminal: (() -> Void)?

    init(
        configuration: VoiceSpeechConfiguration,
        audioLease: VoiceAudioSessionCoordinator.Lease?,
        coordinator: VoiceAudioSessionCoordinator? = nil
    ) {
        self.configuration = configuration
        self.audioLease = audioLease
        self.coordinator = coordinator ?? .shared
        super.init()
        synthesizer.delegate = self
        synthesizer.usesApplicationAudioSession = true
        installInterruptionObservers()
    }

    deinit {
        observers.forEach(NotificationCenter.default.removeObserver)
    }

    func enqueue(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard terminalOutcome == nil, !isCompleting, !isStopping, !trimmed.isEmpty else { return }
        queuedText.append(trimmed)
        startNextIfPossible()
    }

    func pause() {
        guard terminalOutcome == nil, !isCompleting else { return }
        isPaused = true
        if activeUtterance != nil {
            _ = synthesizer.pauseSpeaking(at: .immediate)
        }
    }

    func resume() {
        guard terminalOutcome == nil, !isCompleting, isPaused else { return }
        isPaused = false
        if synthesizer.isPaused {
            _ = synthesizer.continueSpeaking()
        } else {
            startNextIfPossible()
        }
        // The queue can drain while paused (a `didFinish` delivered after a
        // barge-in `pause()` bails out of `completeIfDrained` on `!isPaused`),
        // so re-check here exactly as `didFinishUtterance` does. Without this a
        // `finish()` continuation parked before the pause is never resumed.
        completeIfDrained()
    }

    func finish() async throws -> VoiceSpeechPlaybackOutcome {
        if let terminalOutcome { return terminalOutcome }
        isFinishing = true
        completeIfDrained()
        if let terminalOutcome { return terminalOutcome }
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                finishContinuations.append(continuation)
                completeIfDrained()
            }
        } onCancel: {
            Task { @MainActor [weak self] in
                self?.stopImmediately()
            }
        }
    }

    func stop() async {
        stopImmediately()
        _ = try? await finish()
    }

    func stopImmediately() {
        guard terminalOutcome == nil, !isCompleting, !isStopping else { return }
        isStopping = true
        queuedText.removeAll()
        if activeUtterance != nil || synthesizer.isSpeaking || synthesizer.isPaused {
            let requestedStop = synthesizer.stopSpeaking(at: .immediate)
            if !requestedStop {
                activeUtterance = nil
                complete(.interrupted)
            }
        } else {
            complete(.interrupted)
        }
    }

    nonisolated func speechSynthesizer(
        _: AVSpeechSynthesizer,
        didFinish utterance: AVSpeechUtterance
    ) {
        let utteranceID = ObjectIdentifier(utterance)
        Task { @MainActor [weak self] in self?.didFinishUtterance(utteranceID) }
    }

    nonisolated func speechSynthesizer(
        _: AVSpeechSynthesizer,
        didCancel utterance: AVSpeechUtterance
    ) {
        let utteranceID = ObjectIdentifier(utterance)
        Task { @MainActor [weak self] in self?.didCancelUtterance(utteranceID) }
    }

    private func didFinishUtterance(_ utteranceID: ObjectIdentifier) {
        guard let activeUtterance,
              ObjectIdentifier(activeUtterance) == utteranceID
        else { return }
        self.activeUtterance = nil
        if isStopping {
            complete(.interrupted)
            return
        }
        startNextIfPossible()
        completeIfDrained()
    }

    private func didCancelUtterance(_ utteranceID: ObjectIdentifier) {
        if let activeUtterance,
           ObjectIdentifier(activeUtterance) == utteranceID {
            self.activeUtterance = nil
        } else if !isStopping {
            return
        }
        complete(.interrupted)
    }

    private func startNextIfPossible() {
        guard terminalOutcome == nil,
              !isCompleting,
              !isPaused,
              !isStopping,
              activeUtterance == nil,
              !queuedText.isEmpty
        else { return }
        let text = queuedText.removeFirst()
        let utterance = AVSpeechUtterance(string: text)
        let systemVoiceIdentifier = configuration.voiceIdentifier
        let requestedVoice = AVSpeechSynthesisVoice(identifier: systemVoiceIdentifier)
        let requestedLanguage = requestedVoice?.language
            .split(separator: "-").first.map(String.init)?.lowercased()
        let effectiveLanguage = configuration.languageIdentifier
            .split(separator: "-").first.map(String.init)?.lowercased()
        utterance.voice = requestedVoice.flatMap {
            requestedLanguage == effectiveLanguage ? $0 : nil
        } ?? AVSpeechSynthesisVoice(language: configuration.languageIdentifier)
        utterance.rate = VoiceCapabilityModel.utteranceRate(from: configuration.speed)
        activeUtterance = utterance
        synthesizer.speak(utterance)
    }

    private func completeIfDrained() {
        guard isFinishing,
              activeUtterance == nil,
              queuedText.isEmpty,
              !isPaused
        else { return }
        complete(.completed)
    }

    private func complete(_ outcome: VoiceSpeechPlaybackOutcome) {
        guard terminalOutcome == nil, !isCompleting else { return }
        isCompleting = true
        queuedText.removeAll()
        activeUtterance = nil
        observers.forEach(NotificationCenter.default.removeObserver)
        observers = []
        let lease = audioLease
        audioLease = nil
        let coordinator = self.coordinator
        Task { @MainActor [weak self] in
            if let lease {
                await coordinator.release(lease)
            }
            let continuations = self?.finishContinuations ?? []
            self?.finishContinuations.removeAll()
            self?.terminalOutcome = outcome
            self?.isCompleting = false
            continuations.forEach { $0.resume(returning: outcome) }
            self?.onTerminal?()
            self?.onTerminal = nil
        }
    }

    private func installInterruptionObservers() {
        let center = NotificationCenter.default
        observers.append(center.addObserver(
            forName: AVAudioSession.interruptionNotification,
            object: AVAudioSession.sharedInstance(),
            queue: .main
        ) { [weak self] notification in
            let raw = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt
            guard raw == AVAudioSession.InterruptionType.began.rawValue else { return }
            Task { @MainActor [weak self] in self?.stopImmediately() }
        })
        observers.append(center.addObserver(
            forName: AVAudioSession.routeChangeNotification,
            object: AVAudioSession.sharedInstance(),
            queue: .main
        ) { [weak self] notification in
            guard
                let raw = notification.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt,
                AVAudioSession.RouteChangeReason(rawValue: raw) == .oldDeviceUnavailable
            else { return }
            Task { @MainActor [weak self] in self?.stopImmediately() }
        })
    }
}

/// Process-wide owner for settings/onboarding previews. A preview can outlive
/// its SwiftUI row, so the task and player live here and can be stopped and
/// awaited before Flow Mode tries to acquire the microphone lease.
@MainActor
final class VoicePreviewPlayback {
    static let shared = VoicePreviewPlayback(player: IOSAudioService.shared.speechPlayer)

    private let player: any VoiceSpeechPlaying
    private var activeID: UUID?
    private var activeTask: Task<VoiceSpeechPlaybackOutcome, Error>?

    init(player: any VoiceSpeechPlaying) {
        self.player = player
    }

    func play(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        await stop()
        let id = UUID()
        let player = player
        let task = Task { @MainActor in
            try await player.speak(request)
        }
        activeID = id
        activeTask = task
        do {
            let result = try await task.value
            clearIfCurrent(id)
            return result
        } catch {
            clearIfCurrent(id)
            throw error
        }
    }

    func stop() async {
        guard let task = activeTask else { return }
        activeID = nil
        activeTask = nil
        task.cancel()
        _ = await player.stopAutomaticPlaybackAndWait()
        _ = await task.result
    }

    private func clearIfCurrent(_ id: UUID) {
        guard activeID == id else { return }
        activeID = nil
        activeTask = nil
    }
}

@MainActor
private final class FlowResponseContext {
    let token: ConversationTurnToken
    let operation: UInt64
    let audioConfigurationSnapshot: AudioConfigurationSnapshot
    let speechConfiguration: VoiceSpeechConfiguration
    var segmenter = StreamingSpeechSegmenter()
    var lastSpeechSequence: UInt64 = 0
    var pendingSegments: [String] = []
    var speechSession: (any VoiceSpeechStreamingSession)?
    var bargeInSession: (any VoiceBargeInSession)?
    var terminalCompletion: ConversationTurnCompletion?
    var isOpeningSpeech = false
    var isFinishingSpeech = false
    var isInterrupting = false
    var bargeInUnavailable = false

    init(token: ConversationTurnToken, operation: UInt64, audioConfigurationSnapshot: AudioConfigurationSnapshot,
         speechConfiguration: VoiceSpeechConfiguration) {
        self.token = token
        self.operation = operation
        self.audioConfigurationSnapshot = audioConfigurationSnapshot
        self.speechConfiguration = speechConfiguration
    }
}

/// Flow can be waiting for a tool while no AVAudioSession lease is active.
/// Reserve the UI audio lane for that exact owner until its response drains.
@MainActor
final class VoiceInteractionOwnership {
    static let shared = VoiceInteractionOwnership()
    private let lane = VoiceAudioOwnership<VoiceInteractionController>()

    func acquire(_ candidate: VoiceInteractionController) -> Bool {
        guard !(lane.owner === candidate && lane.kind == .automatic) else { return false }
        return lane.acquire(candidate, kind: .explicit)
    }

    func acquireAutomatic(_ candidate: VoiceInteractionController) -> Bool {
        lane.acquire(candidate, kind: .automatic)
    }
    func isAutomatic(_ candidate: VoiceInteractionController) -> Bool {
        lane.owner === candidate && lane.kind == .automatic
    }
    func reserveExplicit(_ candidate: VoiceInteractionController) -> VoiceInteractionController? {
        lane.reserveExplicit(candidate)
    }
    func finishReservation(_ candidate: VoiceInteractionController, canAcquire: Bool) -> Bool {
        let acquired = lane.finishReservation(candidate, canAcquire: canAcquire)
        if !acquired { drainAutomatic() }
        return acquired
    }
    func enqueueAutomatic(_ candidate: VoiceInteractionController) { lane.enqueueAutomatic(candidate) }
    func removeAutomatic(_ candidate: VoiceInteractionController) { lane.removeAutomatic(candidate) }
    func reconsiderAutomatic() { drainAutomatic() }
    func owns(_ candidate: VoiceInteractionController) -> Bool { lane.owner === candidate }
    func release(_ candidate: VoiceInteractionController) {
        lane.release(candidate)
        drainAutomatic()
    }
    private func drainAutomatic() {
        var remaining = lane.waitingCount
        while remaining > 0, let candidate = lane.nextAutomatic() {
            remaining -= 1
            _ = candidate.resumePendingAutomaticPlayback()
        }
    }
}

@MainActor
protocol VoiceRealtimeControlling: AnyObject {
    var isActive: Bool { get }
    var isAttached: Bool { get }
    var onStateChange: ((IOSRealtimeAudioPhase, String, String?) -> Void)? { get set }
    func reserveStart(owner: IOSAudioOwner?) -> Bool
    func restoreReservedStart(owner: IOSAudioOwner?)
    func start(snapshot: AudioConfigurationSnapshot) async throws
    func stop(abort: Bool) async -> Bool
    func commitInput()
    func interrupt() async
    func finishCurrentTurnAndClose()
}

extension IOSRealtimeAudioService: VoiceRealtimeControlling {}

/// Main-actor state machine for both chat dictation and hands-free Flow Mode.
/// A generation is captured by every async/callback edge so a closed panel or
/// switched conversation cannot be resurrected by late Speech/TTS/turn events.
@Observable
@MainActor
final class VoiceInteractionController {
    private static let log = Logger(subsystem: "com.lingxi.code", category: "flow-voice")

    private(set) var mode: VoiceInteractionMode?
    private(set) var phase: VoiceInteractionPhase = .paused
    private(set) var caption = ""
    private(set) var detailOverride: String?

    let voiceCapture: VoiceCapture
    let capability: VoiceCapabilityModel

    private let speechPlayer: any VoiceSpeechPlaying
    private let bargeInRecognizer: (any VoiceBargeInRecognizing)?
    private let readinessOverride: (@MainActor () -> VoiceConfigurationReadiness)?
    private let permissionRequester: @MainActor () async -> Void
    private let realtimeSession: any VoiceRealtimeControlling
    private let realtimeReadinessOverride: (@MainActor () -> Bool)?
    private let loopDelay: Duration
    private let flowSilenceInterval: Duration
    private var source: (any ConversationSource)?
    private var flowContext: FlowResponseContext?
    private var activeFlowToken: ConversationTurnToken?
    private var flowSubmitPending = false
    private var suppressedFlowToken: ConversationTurnToken?
    private var automaticPlaybackToken: ConversationTurnToken?
    private var automaticPlaybackOwner: IOSAudioOwner?
    private var automaticPlaybackSnapshot: AudioConfigurationSnapshot?
    private var pendingAutomaticCompletion: ConversationTurnCompletion?
    private var automaticPlaybackCleanup: Task<Bool, Never>?
    private var automaticStopUnconfirmed = false
    private var dictationCompletion: ((String) -> Void)?
    private var rejectedFlowTranscript: ((String) -> Void)?
    private var transitionTask: Task<Void, Never>?
    private var captureStartGeneration: UInt64?
    private var speechTask: Task<Void, Never>?
    private var responseSetupTask: Task<Void, Never>?
    private var bargeInEventTask: Task<Void, Never>?
    private var streamOpenTask: Task<Void, Never>?
    private var audioCleanupTask: (id: UInt64, task: Task<Void, Never>)?
    private var nextAudioCleanupID: UInt64 = 1
    private var cancellationTask: Task<Void, Error>?
    private var cancellationSourceID: ObjectIdentifier?
    private var isCancellingFlowTurn = false
    private var flowPausedInBackground = false
    private var pendingInterruptionTranscript: String?
    private var resumeAfterConfiguration = false
    private var generation: UInt64 = 0
    private let controlsRealtimeSession: Bool
    private let ownership: VoiceInteractionOwnership
    private var ownsRealtimeSession = false
    private var realtimeStartPending = false
    private var finishFlowAfterNavigation = false
    private var finishRealtimeAfterNavigation = false
    private var finishDictationAfterNavigation = false
    private var activeAudioConfigurationSnapshot: AudioConfigurationSnapshot?
    private var activeRecognitionRoute: AudioRouteResolution?
    private var activeSpeechRoute: AudioRouteResolution?
    private var activeVoiceAudioOwner: IOSAudioOwner?

    init() {
        controlsRealtimeSession = true
        ownership = .shared
        voiceCapture = VoiceCapture()
        let capability = VoiceCapabilityModel()
        self.capability = capability
        speechPlayer = IOSAudioService.shared.speechPlayer
        bargeInRecognizer = IOSAudioService.shared.bargeInRecognizer
        readinessOverride = nil
        permissionRequester = { await capability.requestPermissions() }
        realtimeSession = IOSRealtimeAudioService.shared
        realtimeReadinessOverride = nil
        loopDelay = .milliseconds(350)
        flowSilenceInterval = .milliseconds(1_200)
    }

    init(
        voiceCapture: VoiceCapture,
        capability: VoiceCapabilityModel,
        speechPlayer: any VoiceSpeechPlaying,
        bargeInRecognizer: (any VoiceBargeInRecognizing)? = nil,
        controlsRealtimeSession: Bool = true,
        ownership: VoiceInteractionOwnership? = nil,
        readinessOverride: (@MainActor () -> VoiceConfigurationReadiness)? = nil,
        permissionRequester: (@MainActor () async -> Void)? = nil,
        realtimeSession: (any VoiceRealtimeControlling)? = nil,
        realtimeReadinessOverride: (@MainActor () -> Bool)? = nil,
        loopDelay: Duration = .milliseconds(350),
        flowSilenceInterval: Duration = .milliseconds(1_200)
    ) {
        self.voiceCapture = voiceCapture
        self.capability = capability
        self.speechPlayer = speechPlayer
        self.bargeInRecognizer = bargeInRecognizer
        self.controlsRealtimeSession = controlsRealtimeSession
        self.ownership = ownership ?? .shared
        self.readinessOverride = readinessOverride
        self.permissionRequester = permissionRequester ?? {
            await capability.requestPermissions()
        }
        self.realtimeSession = realtimeSession ?? IOSRealtimeAudioService.shared
        self.realtimeReadinessOverride = realtimeReadinessOverride
        self.loopDelay = loopDelay
        self.flowSilenceInterval = flowSilenceInterval
    }

    var isPresented: Bool { mode != nil }
    var hasOwnedAudio: Bool { ownership.owns(self) }
    var capturePhase: VoiceCapturePhase { voiceCapture.phase }

    var orbPhase: OrbPhase {
        switch phase {
        case .listening, .recognizing: return .listening
        case .thinking: return .thinking
        case .speaking: return .speaking
        case .interrupting: return .listening
        case .requestingPermission, .configurationRequired, .paused, .failed: return .idle
        }
    }

    var statusTitle: String {
        switch phase {
        case .requestingPermission: return String(localized: "voice_waiting_permission")
        case .configurationRequired: return String(localized: "voice_status_config_required")
        case .listening: return String(localized: "voice_status_listening")
        case .recognizing: return String(localized: "voice_status_recognizing")
        case .thinking: return String(localized: "voice_status_thinking")
        case .speaking: return String(localized: "voice_status_speaking")
        case .interrupting: return String(localized: "voice_status_interrupting")
        case .paused: return String(localized: "settings_status_paused")
        case .failed: return String(localized: "voice_status_failed")
        }
    }

    var statusDetail: String {
        if let detailOverride, !detailOverride.isEmpty { return detailOverride }
        switch phase {
        case .requestingPermission: return String(localized: "voice_needs_permission_setup")
        case .configurationRequired:
            guard let mode else { return String(localized: "voice_detail_config_required_default") }
            return configurationMessage(for: mode)
        case .listening:
            return mode == .flow ? String(localized: "voice_detail_listening_flow") : String(localized: "voice_detail_listening_dictation")
        case .recognizing: return String(localized: "voice_detail_recognizing")
        case .thinking: return String(localized: "voice_detail_thinking")
        case .speaking: return String(localized: "voice_detail_speaking")
        case .interrupting: return String(localized: "voice_detail_interrupting")
        case .paused: return String(localized: "voice_detail_paused")
        case .failed: return String(localized: "voice_detail_failed")
        }
    }

    var statusColor: Color {
        switch phase {
        case .listening, .recognizing, .interrupting: return Color(okl: 0.72, 0.18, 150)
        case .speaking: return Color(okl: 0.75, 0.19, 300)
        case .configurationRequired, .failed: return Color(okl: 0.75, 0.18, 50)
        case .requestingPermission, .thinking, .paused: return Color(okl: 0.70, 0.16, 260)
        }
    }

    var orbAccessibilityLabel: String { statusTitle }

    var orbAccessibilityHint: String {
        switch phase {
        case .listening:
            return mode == .flow ? String(localized: "voice_hint_listening_flow") : String(localized: "voice_hint_listening_dictation")
        case .thinking: return String(localized: "voice_hint_thinking")
        case .speaking: return String(localized: "voice_hint_speaking")
        case .interrupting: return String(localized: "voice_hint_interrupting")
        case .paused, .failed: return String(localized: "voice_hint_paused_failed")
        default: return ""
        }
    }

    func startDictation(audioOwner: IOSAudioOwner? = nil, onTranscript: @escaping (String) -> Void) {
        guard mode != .flow else { return }
        guard phase != .requestingPermission else { return }
        guard ownership.acquire(self) else {
            if deferExplicitStart(for: .dictation, resume: { [weak self] in
                self?.startDictation(audioOwner: audioOwner, onTranscript: onTranscript)
            }) { return }
            mode = .dictation
            fail(String(localized: "voice_session_busy_retry"))
            return
        }
        beginOperation(mode: .dictation)
        activeVoiceAudioOwner = audioOwner
        activeAudioConfigurationSnapshot = capability.configurationSnapshot
        activeRecognitionRoute = AudioConfigurationRuntime.route(kind: .recognition, snapshot: capability.configurationSnapshot)
        activeSpeechRoute = AudioConfigurationRuntime.route(kind: .speech, snapshot: capability.configurationSnapshot)
        dictationCompletion = onTranscript
        guard currentReadiness.isReadyForDictation else {
            recoverUnavailablePermissionsOrRequireConfiguration(for: .dictation)
            return
        }
        scheduleListening()
    }

    func startFlow(source: any ConversationSource, onRejectedTranscript: @escaping (String) -> Void = { _ in }) {
        guard controlsRealtimeSession, !source.isTemporaryConversation, source.isAvailableForPrompts else { return }
        guard mode != .flow else { return }
        guard mode != .dictation || voiceCapture.phase == .idle else { return }
        guard ownership.acquire(self) else {
            mode = .flow
            self.source = source
            rejectedFlowTranscript = onRejectedTranscript
            if deferExplicitStart(for: .flow, resume: { [weak self] in
                self?.startFlow(source: source, onRejectedTranscript: onRejectedTranscript)
            }) { return }
            fail(String(localized: "voice_session_busy_retry"))
            return
        }
        beginOperation(mode: .flow)
        self.source = source
        activeVoiceAudioOwner = source.voiceAudioOwner
        activeAudioConfigurationSnapshot = capability.configurationSnapshot
        activeRecognitionRoute = AudioConfigurationRuntime.route(kind: .recognition, snapshot: capability.configurationSnapshot)
        activeSpeechRoute = AudioConfigurationRuntime.route(kind: .speech, snapshot: capability.configurationSnapshot)
        rejectedFlowTranscript = onRejectedTranscript
        if capability.configurationSnapshot.configuration.conversation.mode == "realtime" {
            startRealtimeConversation()
            return
        }
        guard currentReadiness.isReadyForFlow else {
            recoverUnavailablePermissionsOrRequireConfiguration(for: .flow)
            return
        }
        guard !source.model.streaming,
              !source.model.isCancelling,
              !source.model.sessionTransitionPending
        else {
            fail(String(localized: "voice_flow_busy_wait_to_open"))
            return
        }
        scheduleListening()
    }

    func finishListening() {
        if mode == .flow, controlsRealtimeSession, ownsRealtimeSession,
           realtimeSession.isActive {
            realtimeSession.commitInput()
            return
        }
        if phase == .requestingPermission, mode == .dictation {
            close()
            return
        }
        guard phase == .listening else { return }
        transition(to: .recognizing)
        voiceCapture.finish()
    }

    func cancelDictation() {
        guard mode == .dictation else { return }
        close()
    }

    func handleOrbTap() {
        if mode == .flow, controlsRealtimeSession, ownsRealtimeSession,
           realtimeSession.isActive, phase == .thinking || phase == .speaking {
            let realtimeSession = realtimeSession
            Task { await realtimeSession.interrupt() }
            return
        }
        switch phase {
        case .listening:
            finishListening()
        case .thinking, .speaking:
            cancelFlowTurnAndRelisten()
        case .paused, .failed:
            retry()
        case .requestingPermission, .configurationRequired, .recognizing, .interrupting:
            break
        }
    }

    func retry() {
        guard let requestedMode = mode else { return }
        guard ownership.acquire(self) else {
            if deferExplicitStart(for: requestedMode, resetMode: false, resume: { [weak self] in self?.retry() }) { return }
            fail(String(localized: "voice_session_busy_retry"))
            return
        }
        capability.reloadFromDefaults()
        activeAudioConfigurationSnapshot = capability.configurationSnapshot
        activeRecognitionRoute = AudioConfigurationRuntime.route(kind: .recognition, snapshot: capability.configurationSnapshot)
        activeSpeechRoute = AudioConfigurationRuntime.route(kind: .speech, snapshot: capability.configurationSnapshot)
        detailOverride = nil
        if mode == .flow, capability.configurationSnapshot.configuration.conversation.mode == "realtime" {
            startRealtimeConversation()
            return
        }
        guard let mode else { return }
        let ready = mode == .flow
            ? currentReadiness.isReadyForFlow
            : currentReadiness.isReadyForDictation
        guard ready else {
            recoverUnavailablePermissionsOrRequireConfiguration(for: mode)
            return
        }
        if mode == .flow, flowPausedInBackground {
            let stillRunning = source?.model.streaming == true
                || source?.model.isCancelling == true
                || source?.model.sessionTransitionPending == true
                || activeFlowToken != nil
            if stillRunning {
                detailOverride = String(localized: "voice_stopped_in_background")
                transition(to: .paused)
                return
            }
            flowPausedInBackground = false
        }
        if mode == .flow,
           let pendingInterruptionTranscript,
           let context = flowContext {
            isCancellingFlowTurn = false
            context.isInterrupting = true
            commitBargeInTranscript(pendingInterruptionTranscript, context: context)
            return
        }
        if mode == .flow, activeFlowToken != nil {
            cancelFlowTurnAndRelisten()
            return
        }
        if mode == .flow,
           let source,
           source.model.streaming || source.model.isCancelling || source.model.sessionTransitionPending {
            fail(String(localized: "voice_flow_busy_wait_to_retry"))
            return
        }
        invalidateTasks(keepingMode: true, cancelOwnedTurn: false)
        scheduleListening()
    }

    func reloadConfigurationAndResumeIfPossible() {
        capability.reloadFromDefaults()
        guard mode != nil, resumeAfterConfiguration else { return }
        retry()
    }

    /// Register the exact ordinary-chat turn that may be played when the
    /// user's auto-play preference is enabled. Flow turns bypass this path and
    /// always speak through their owned token.
    func registerAutomaticPlaybackCandidate(_ token: ConversationTurnToken, audioOwner: IOSAudioOwner? = nil) {
        ownership.removeAutomatic(self)
        pendingAutomaticCompletion = nil
        automaticPlaybackToken = token
        automaticPlaybackOwner = audioOwner
        automaticPlaybackSnapshot = capability.configurationSnapshot
        if ownership.isAutomatic(self), speechTask != nil || automaticPlaybackCleanup != nil {
            _ = beginAutomaticPlaybackCleanup()
        }
    }

    func handleTurnSpeechUpdate(_ update: ConversationTurnSpeechUpdate) {
        guard mode == .flow,
              let context = flowContext,
              context.token == activeFlowToken,
              update.token == context.token,
              update.sequence > context.lastSpeechSequence,
              context.token != suppressedFlowToken,
              !isCancellingFlowTurn,
              !flowPausedInBackground
        else { return }

        context.lastSpeechSequence = update.sequence
        context.pendingSegments.append(contentsOf: context.segmenter.append(update.delta))
        enqueuePendingSpeechIfPossible(context)
    }

    func handleTurnCompletion(_ completion: ConversationTurnCompletion) {
        if completion.token == automaticPlaybackToken {
            handleAutomaticTurnCompletion(completion, allowPlayback: mode == nil)
            return
        }
        guard mode == .flow else {
            handleAutomaticTurnCompletion(completion, allowPlayback: mode == nil)
            return
        }
        guard let context = flowContext,
              completion.token == context.token,
              completion.token == activeFlowToken,
              completion.token != suppressedFlowToken,
              !isCancellingFlowTurn
        else { return }

        context.terminalCompletion = completion
        if flowPausedInBackground {
            if completion.outcome == .completed {
                context.pendingSegments.append(contentsOf: context.segmenter.finish(
                    finalText: completion.finalAssistantText
                ))
                if context.segmenter.detectedTerminalRewrite {
                    Self.log.notice(
                        "background assistant text rewrote streamed prefix turn=\(completion.token.clientTurnId, privacy: .public) epoch=\(completion.token.sessionEpoch, privacy: .public) sequence=\(context.lastSpeechSequence, privacy: .public)"
                    )
                }
            }
            finishBackgroundPausedFlowTurn(context, completion: completion)
            return
        }
        if completion.outcome == .completed {
            context.pendingSegments.append(contentsOf: context.segmenter.finish(
                finalText: completion.finalAssistantText
            ))
            if context.segmenter.detectedTerminalRewrite {
                Self.log.notice(
                    "terminal assistant text rewrote streamed prefix turn=\(completion.token.clientTurnId, privacy: .public) epoch=\(completion.token.sessionEpoch, privacy: .public) sequence=\(context.lastSpeechSequence, privacy: .public)"
                )
            }
            enqueuePendingSpeechIfPossible(context)
            guard !context.isInterrupting else { return }
            finishFlowResponseIfReady(context)
            return
        }

        guard !context.isInterrupting else { return }
        stopFlowResponse(context)
        if finishFlowAfterNavigation {
            close(cancelOwnedTurn: false)
            return
        }
        switch completion.outcome {
        case .completed:
            break
        case .cancelled:
            fail(String(localized: "voice_turn_cancelled_retry"))
        case .maxTurns:
            fail(String(localized: "voice_turn_max_turns"))
        case .failed:
            fail(String(localized: "voice_turn_failed"))
        }
    }

    func handleBackground() {
        if !ownership.owns(self) {
            ownership.removeAutomatic(self)
            pendingAutomaticCompletion = nil
            automaticPlaybackToken = nil
            automaticPlaybackOwner = nil
            automaticPlaybackSnapshot = nil
            return
        }
        if controlsRealtimeSession && ownsRealtimeSession {
            let realtimeSession = realtimeSession
            Task { @MainActor [weak self] in
                if await realtimeSession.stop(abort: true) {
                    self?.ownsRealtimeSession = false
                }
            }
        }
        automaticPlaybackToken = nil
        automaticPlaybackOwner = nil
        automaticPlaybackSnapshot = nil
        ownership.removeAutomatic(self)
        pendingAutomaticCompletion = nil
        if mode == nil {
            close(cancelOwnedTurn: false)
            return
        }
        let preservesSubmittedFlowTurn = mode == .flow && activeFlowToken != nil && flowContext != nil
        let ownedSource = preservesSubmittedFlowTurn ? nil : (activeFlowToken == nil ? nil : source)
        let context = flowContext
        invalidateTasks(keepingMode: true, cancelOwnedTurn: false)
        stopFlowAudioDetached(context)
        voiceCapture.cancel()
        speechPlayer.stop()
        flowPausedInBackground = preservesSubmittedFlowTurn
        if !preservesSubmittedFlowTurn {
            activeFlowToken = nil
            flowContext = nil
        }
        suppressedFlowToken = nil
        pendingInterruptionTranscript = nil
        isCancellingFlowTurn = false
        caption = ""
        detailOverride = String(localized: "voice_stopped_in_background")
        transition(to: .paused)
        if let ownedSource {
            startDetachedCancellation(of: ownedSource)
        }
    }

    func handleContextChange() {
        // An ordinary turn's optional reply playback belongs to this owner,
        // even if its view disappears before the turn or playback finishes.
        if mode == nil, automaticPlaybackToken != nil || speechTask != nil || automaticPlaybackCleanup != nil || automaticStopUnconfirmed { return }
        if mode == .flow, activeFlowToken != nil, flowContext != nil {
            finishFlowAfterNavigation = true
            return
        }
        if mode == .flow, flowSubmitPending, ownership.owns(self) {
            finishFlowAfterNavigation = true
            return
        }
        // Permission and prior-audio cleanup may suspend after this owner has
        // already acquired the explicit lane, before capture leaves idle.
        if captureStartGeneration != nil, ownership.owns(self) {
            if mode == .dictation { finishDictationAfterNavigation = true; return }
            if mode == .flow { finishFlowAfterNavigation = true; return }
        }
        // A capture may already contain speech even before a turn exists.
        // Keep its callback bound to the old draft/owner until it settles.
        if mode == .dictation, capturePhase != .idle {
            finishDictationAfterNavigation = true
            return
        }
        if mode == .flow, capturePhase != .idle {
            finishFlowAfterNavigation = true
            return
        }
        if controlsRealtimeSession && ownsRealtimeSession {
            finishRealtimeAfterNavigation = true
            if !realtimeStartPending { realtimeSession.finishCurrentTurnAndClose() }
            return
        }
        close(cancelOwnedTurn: false)
    }

    /// Explicit capture may interrupt optional auto-play, but never an active
    /// Flow or dictation owner. The reservation prevents queued speech from
    /// reacquiring the lane until native playback has actually drained.
    private func deferExplicitStart(for requestedMode: VoiceInteractionMode, resetMode: Bool = true,
                                    resume: @escaping @MainActor () -> Void) -> Bool {
        guard let previous = ownership.reserveExplicit(self) else { return false }
        let operation = nextGeneration()
        mode = requestedMode
        transition(to: .requestingPermission)
        transitionTask?.cancel()
        transitionTask = Task { @MainActor [self, previous] in
            let stopped = await previous.stopAutomaticPlaybackForPreemption()
            let current = !Task.isCancelled && generation == operation && mode == requestedMode
            let acquired = ownership.finishReservation(self, canAcquire: stopped && current)
            guard current else {
                if acquired { ownership.release(self) }
                return
            }
            guard acquired else {
                transitionTask = nil
                fail(String(localized: "voice_session_busy_retry"))
                return
            }
            transitionTask = nil
            if resetMode { mode = nil }
            phase = .paused
            resume()
        }
        return true
    }

    private func stopAutomaticPlaybackForPreemption() async -> Bool {
        guard let cleanup = beginAutomaticPlaybackCleanup() else { return false }
        return await cleanup.value
    }

    private func beginAutomaticPlaybackCleanup() -> Task<Bool, Never>? {
        if let automaticPlaybackCleanup { return automaticPlaybackCleanup }
        guard ownership.isAutomatic(self) else { return nil }
        let active = speechTask
        speechTask = nil
        automaticStopUnconfirmed = true
        active?.cancel()
        let cleanup = Task { @MainActor [self] in
            let stopped = await speechPlayer.stopAutomaticPlaybackAndWait()
            let drained: Bool
            if let active, stopped {
                drained = await VoiceAudioStopDeadline.completed(active, within: .seconds(5))
            } else {
                drained = stopped
            }
            if drained {
                automaticPlaybackCleanup = nil
                automaticStopUnconfirmed = false
                ownership.release(self)
            } else {
                // Keep the lane quarantined until a later retry confirms native
                // cleanup; a finished caller is not a device stop receipt.
                automaticPlaybackCleanup = nil
                speechTask = active
            }
            return drained
        }
        automaticPlaybackCleanup = cleanup
        return cleanup
    }

    fileprivate func resumePendingAutomaticPlayback() -> Bool {
        guard let completion = pendingAutomaticCompletion else { return true }
        if mode != nil {
            ownership.enqueueAutomatic(self)
            return false
        }
        pendingAutomaticCompletion = nil
        handleAutomaticTurnCompletion(completion, allowPlayback: mode == nil)
        return pendingAutomaticCompletion == nil
    }

    func handleContextRestored() {
        finishFlowAfterNavigation = false
        finishDictationAfterNavigation = false
        if realtimeStartPending {
            finishRealtimeAfterNavigation = false
            realtimeSession.restoreReservedStart(owner: source?.voiceAudioOwner)
        }
    }

    func close() {
        close(cancelOwnedTurn: true)
    }

    private func close(cancelOwnedTurn: Bool) {
        let ownedAudio = ownership.owns(self)
        let automaticStop = ownership.isAutomatic(self) ? beginAutomaticPlaybackCleanup() : nil
        var realtimeStop: Task<Bool, Never>?
        if controlsRealtimeSession && ownsRealtimeSession {
            let realtimeSession = realtimeSession
            if cancelOwnedTurn {
                realtimeStop = Task { await realtimeSession.stop(abort: true) }
            } else {
                realtimeSession.finishCurrentTurnAndClose()
            }
        }
        let ownedSource = activeFlowToken == nil && !flowSubmitPending ? nil : source
        let context = flowContext
        let activeSpeech = speechTask
        let hasOwnedPlayback = activeSpeech != nil || context?.speechSession != nil
        invalidateTasks(keepingMode: false, cancelOwnedTurn: false)
        stopFlowAudioDetached(context)
        let capture = voiceCapture.cancelForDrain()
        if ownedAudio, automaticStop == nil, hasOwnedPlayback { speechPlayer.stop() }
        activeFlowToken = nil
        flowSubmitPending = false
        suppressedFlowToken = nil
        flowContext = nil
        pendingInterruptionTranscript = nil
        isCancellingFlowTurn = false
        flowPausedInBackground = false
        source = nil
        dictationCompletion = nil
        rejectedFlowTranscript = nil
        caption = ""
        detailOverride = nil
        resumeAfterConfiguration = false
        mode = nil
        phase = .paused
        finishFlowAfterNavigation = false
        finishRealtimeAfterNavigation = false
        finishDictationAfterNavigation = false
        if !ownedAudio { ownership.reconsiderAutomatic() }

        if ownedAudio, automaticStop == nil {
            let closedGeneration = generation
            let cleanup = audioCleanupTask?.task
            Task { @MainActor [weak self] in
                await capture?.value
                await activeSpeech?.value
                await cleanup?.value
                guard let self, self.generation == closedGeneration, self.mode == nil else { return }
                if let realtimeStop, !(await realtimeStop.value) {
                    self.mode = .flow
                    self.ownsRealtimeSession = true
                    self.fail(String(localized: "voice_stop_failed_message \(String(localized: "voice_session_busy_retry"))"))
                    return
                }
                self.ownsRealtimeSession = false
                if hasOwnedPlayback { await self.speechPlayer.stopAndWait() }
                guard self.generation == closedGeneration, self.mode == nil else { return }
                self.ownership.release(self)
            }
        }

        if cancelOwnedTurn, let ownedSource {
            startDetachedCancellation(of: ownedSource)
        }
    }

    private func startRealtimeConversation() {
        guard let source, source.isAvailableForPrompts, !source.model.streaming, !source.model.isCancelling,
              !source.model.sessionTransitionPending else {
            fail(String(localized: "voice_flow_busy_wait_to_open"))
            return
        }
        let realtime = realtimeSession
        guard !realtime.isActive else {
            fail(String(localized: "voice_session_busy_retry"))
            return
        }
        let supported = realtimeReadinessOverride?() ?? (capability.cloudService.realtimeCapability?.supported == true)
        guard realtime.isAttached, supported else {
            detailOverride = capability.cloudService.realtimeCapability?.reason ?? String(localized: "audio_realtime_unavailable")
            transition(to: .configurationRequired)
            resumeAfterConfiguration = true
            return
        }
        let snapshot = activeAudioConfigurationSnapshot ?? capability.configurationSnapshot
        guard realtime.reserveStart(owner: source.voiceAudioOwner) else {
            fail(String(localized: "voice_session_busy_retry"))
            return
        }
        let operation = nextGeneration()
        ownsRealtimeSession = true
        realtimeStartPending = true
        realtime.onStateChange = { [weak self] state, caption, error in
            guard let self, self.mode == .flow, self.generation == operation else { return }
            self.caption = caption
            if let error { self.detailOverride = error }
            switch state {
            case .idle:
                if self.finishRealtimeAfterNavigation {
                    self.ownsRealtimeSession = false
                    self.close(cancelOwnedTurn: false)
                } else {
                    self.transition(to: .paused)
                }
            case .interrupted: self.transition(to: .paused)
            case .connecting: self.transition(to: .requestingPermission)
            case .listening: self.transition(to: .listening)
            case .thinking: self.transition(to: .thinking)
            case .speaking: self.transition(to: .speaking)
            case .failed: self.transition(to: .failed)
            }
        }
        transitionTask?.cancel()
        transitionTask = Task { @MainActor [weak self] in
            guard let self, !Task.isCancelled, self.generation == operation else { return }
            do {
                try await realtime.start(snapshot: snapshot)
                guard !Task.isCancelled, self.generation == operation else { return }
                self.realtimeStartPending = false
                self.transitionTask = nil
                if self.finishRealtimeAfterNavigation { realtime.finishCurrentTurnAndClose() }
            }
            catch is CancellationError {
                guard !Task.isCancelled, self.generation == operation else { return }
                let stopped = await realtime.stop(abort: true)
                guard self.generation == operation else { return }
                self.realtimeStartPending = false
                self.ownsRealtimeSession = !stopped
                if self.finishRealtimeAfterNavigation { self.close(cancelOwnedTurn: false) }
                else { self.fail(String(localized: "voice_turn_cancelled_retry")) }
            }
            catch {
                guard self.generation == operation else { return }
                let stopped = await realtime.stop(abort: true)
                guard self.generation == operation else { return }
                self.realtimeStartPending = false
                self.ownsRealtimeSession = !stopped
                if self.finishRealtimeAfterNavigation { self.close(cancelOwnedTurn: false) }
                else { self.fail(error.localizedDescription) }
            }
        }
    }

    private var currentReadiness: VoiceConfigurationReadiness {
        readinessOverride?() ?? capability.configurationReadiness
    }

    private func admittedCaptureIsReady(for mode: VoiceInteractionMode) -> Bool {
        if readinessOverride != nil {
            return mode == .flow ? currentReadiness.isReadyForFlow : currentReadiness.isReadyForDictation
        }
        guard let recognition = activeRecognitionRoute, recognition.effective != nil,
              capability.microphonePermissionStatus == .granted else { return false }
        let systemRecognition = recognition.effective?.source == .system
        guard !systemRecognition || capability.speechAuthorization == .authorized else { return false }
        let recognitionReady = recognition.status == .ready ||
            (systemRecognition && recognition.status == .permissionRequired)
        return recognitionReady && (mode != .flow || activeSpeechRoute?.status == .ready)
    }

    private func beginOperation(mode: VoiceInteractionMode) {
        if self.mode != mode {
            let context = flowContext
            let hadSpeech = speechTask != nil || context?.speechSession != nil
            invalidateTasks(keepingMode: false, cancelOwnedTurn: false)
            stopFlowAudioDetached(context)
            voiceCapture.cancel()
            if hadSpeech { speechPlayer.stop() }
            activeFlowToken = nil
            suppressedFlowToken = nil
            flowContext = nil
            pendingInterruptionTranscript = nil
            isCancellingFlowTurn = false
            flowPausedInBackground = false
            source = nil
            dictationCompletion = nil
            rejectedFlowTranscript = nil
        }
        self.mode = mode
        caption = ""
        detailOverride = nil
        flowPausedInBackground = false
        resumeAfterConfiguration = false
    }

    private func requireConfiguration(for mode: VoiceInteractionMode) {
        if (mode == .dictation && finishDictationAfterNavigation) || (mode == .flow && finishFlowAfterNavigation) {
            close(cancelOwnedTurn: false)
            return
        }
        resumeAfterConfiguration = true
        detailOverride = configurationMessage(for: mode)
        transition(to: .configurationRequired)
    }

    private func recoverUnavailablePermissionsOrRequireConfiguration(
        for mode: VoiceInteractionMode
    ) {
        let hasUndeterminedPermission = currentReadiness.issues.contains { issue in
            issue.kind == .permissionUndetermined
                && (issue.component == .speech || issue.component == .microphone)
        }
        guard hasUndeterminedPermission else {
            requireConfiguration(for: mode)
            return
        }

        detailOverride = String(localized: "voice_needs_permission_setup")
        transition(to: .requestingPermission)
        let operation = nextGeneration()
        captureStartGeneration = operation
        transitionTask?.cancel()
        transitionTask = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.permissionRequester()
            guard !Task.isCancelled,
                  self.generation == operation,
                  self.mode == mode
            else { return }

            self.capability.reloadFromDefaults()
            let isReady = self.admittedCaptureIsReady(for: mode)
            self.transitionTask = nil
            if isReady {
                self.scheduleListening()
            } else {
                self.captureStartGeneration = nil
                self.requireConfiguration(for: mode)
            }
        }
    }

    private func configurationMessage(for mode: VoiceInteractionMode) -> String {
        let readiness = currentReadiness
        let relevantIssues = readiness.issues.filter { issue in
            mode == .flow || issue.component != .tts
        }
        let messages = relevantIssues
            .sorted { lhs, rhs in
                let lhsPriority = lhs.kind == .unconfigured ? 0 : 1
                let rhsPriority = rhs.kind == .unconfigured ? 0 : 1
                return lhsPriority < rhsPriority
            }
            .prefix(4)
            .map(Self.compactConfigurationIssue)
        return messages.isEmpty ? String(localized: "voice_setup_required_default") : messages.joined(separator: "；")
    }

    private static func compactConfigurationIssue(_ issue: VoiceConfigurationIssue) -> String {
        switch (issue.component, issue.kind) {
        case (.speech, .unconfigured) where issue.message == String(localized: "voice_issue_save_language_mode"):
            return String(localized: "voice_issue_speech_config_unsaved")
        case (.tts, .unconfigured) where issue.message == String(localized: "voice_issue_save_voice"):
            return String(localized: "voice_issue_voice_not_selected")
        case (.speech, .permissionUndetermined): return String(localized: "voice_issue_need_speech_permission_short")
        case (.microphone, .permissionUndetermined): return String(localized: "voice_mic_permission_required")
        default: return issue.message
        }
    }

    private func scheduleListening(after delay: Duration? = nil) {
        if delay != nil {
            activeAudioConfigurationSnapshot = capability.configurationSnapshot
            activeRecognitionRoute = AudioConfigurationRuntime.route(kind: .recognition, snapshot: capability.configurationSnapshot)
            activeSpeechRoute = AudioConfigurationRuntime.route(kind: .speech, snapshot: capability.configurationSnapshot)
        }
        let operation = nextGeneration()
        // Automatic relistening is idle work; only an explicitly requested
        // capture is retained when its owner leaves the screen.
        captureStartGeneration = delay == nil ? operation : nil
        transitionTask?.cancel()
        transitionTask = Task { @MainActor [weak self] in
            guard let self else { return }
            if let delay {
                do { try await Task.sleep(for: delay) } catch { return }
            }
            guard !Task.isCancelled, self.generation == operation else { return }
            await self.waitForFlowAudioCleanup()
            guard !Task.isCancelled, self.generation == operation else { return }
            await VoicePreviewPlayback.shared.stop()
            guard !Task.isCancelled, self.generation == operation else { return }
            await self.stopPlaybackAndWait()
            guard !Task.isCancelled, self.generation == operation else { return }
            self.captureStartGeneration = nil
            self.transitionTask = nil
            self.beginCapture(operation: operation)
        }
    }

    private func beginCapture(operation: UInt64) {
        guard generation == operation, let mode else { return }
        let ready = admittedCaptureIsReady(for: mode)
        guard ready else {
            requireConfiguration(for: mode)
            return
        }
        if mode == .flow,
           let source,
           source.model.streaming || source.model.isCancelling || source.model.sessionTransitionPending {
            if finishFlowAfterNavigation { close(cancelOwnedTurn: false); return }
            fail(String(localized: "voice_previous_turn_not_finished"))
            return
        }

        caption = ""
        detailOverride = nil
        resumeAfterConfiguration = false
        let audioConfigurationSnapshot = activeAudioConfigurationSnapshot ?? capability.configurationSnapshot
        transition(to: .listening)
        voiceCapture.start(
            language: audioConfigurationSnapshot.configuration.language,
            automaticEndpointAfterSilence: mode == .flow ? flowSilenceInterval : nil,
            configurationSnapshot: audioConfigurationSnapshot,
            audioOwner: activeVoiceAudioOwner
        ) { [weak self] result in
            guard let self, self.generation == operation else { return }
            self.handleCaptureResult(result, operation: operation)
        }
    }

    private func handleCaptureResult(_ result: VoiceCaptureResult, operation: UInt64) {
        guard generation == operation, let mode else { return }
        switch result {
        case .transcript(let text):
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else {
                handleEmptyCapture(mode: mode)
                return
            }
            if mode == .dictation {
                let completion = dictationCompletion
                completion?(trimmed)
                close()
            } else {
                submitFlowTranscript(trimmed)
            }
        case .empty:
            if mode == .dictation, finishDictationAfterNavigation { close(cancelOwnedTurn: false); return }
            handleEmptyCapture(mode: mode)
        case .permissionDenied:
            if mode == .dictation, finishDictationAfterNavigation { close(cancelOwnedTurn: false); return }
            if mode == .flow, finishFlowAfterNavigation { close(cancelOwnedTurn: false); return }
            capability.reloadFromDefaults()
            requireConfiguration(for: mode)
        case .failed(let message):
            if mode == .dictation, finishDictationAfterNavigation { close(cancelOwnedTurn: false); return }
            if mode == .flow, finishFlowAfterNavigation { close(cancelOwnedTurn: false); return }
            fail(String(localized: "voice_recognition_failed_message \(message)"))
        }
    }

    private func handleEmptyCapture(mode: VoiceInteractionMode) {
        if mode == .flow {
            if finishFlowAfterNavigation {
                close(cancelOwnedTurn: false)
                return
            }
            detailOverride = String(localized: "voice_relistening_unclear")
            scheduleListening(after: loopDelay)
        } else {
            fail(String(localized: "voice_nothing_recognized_retry"))
        }
    }

    private func submitFlowTranscript(_ text: String) {
        guard let source else {
            fail(String(localized: "voice_session_unavailable"))
            return
        }
        caption = text
        transition(to: .thinking)
        let operation = nextGeneration()
        let snapshot = activeAudioConfigurationSnapshot ?? capability.configurationSnapshot
        let speech = speechConfiguration(snapshot: snapshot, owner: activeVoiceAudioOwner, route: activeSpeechRoute)
        flowSubmitPending = true
        transitionTask?.cancel()
        transitionTask = Task { @MainActor [weak self, source] in
            let token = await source.send(text)
            guard let self, self.generation == operation, self.mode == .flow, self.source === source else {
                if token != nil { try? await source.cancelAndWait() }
                return
            }
            self.flowSubmitPending = false
            self.transitionTask = nil
            guard let token else {
                self.rejectedFlowTranscript?(text)
                if self.finishFlowAfterNavigation { self.close(cancelOwnedTurn: false); return }
                self.fail(String(localized: "voice_session_busy_retry"))
                return
            }
            self.activeFlowToken = token
            self.suppressedFlowToken = nil
            self.flowPausedInBackground = false
            self.pendingInterruptionTranscript = nil
            let context = FlowResponseContext(token: token, operation: operation,
                audioConfigurationSnapshot: snapshot, speechConfiguration: speech)
            self.flowContext = context
            self.startBargeInMonitoring(context)
        }
    }

    private var speechConfiguration: VoiceSpeechConfiguration {
        speechConfiguration(snapshot: capability.configurationSnapshot)
    }

    private func speechConfiguration(snapshot: AudioConfigurationSnapshot,
                                     owner: IOSAudioOwner? = nil,
                                     route capturedRoute: AudioRouteResolution? = nil) -> VoiceSpeechConfiguration {
        let route = capturedRoute ?? AudioConfigurationRuntime.route(kind: .speech, snapshot: snapshot)
        let voiceIdentifier: String
        if let effective = route.effective {
            switch effective.source {
            case .system:
                voiceIdentifier = effective.voiceId ?? ""
            case .offline:
                voiceIdentifier = "offline:\(effective.modelId ?? "unknown"):\(effective.voiceId ?? "unknown")"
            default:
                voiceIdentifier = ""
            }
        } else {
            voiceIdentifier = ""
        }
        return VoiceSpeechConfiguration(
            voiceIdentifier: voiceIdentifier,
            languageIdentifier: resolveAudioLanguageForNativeDevice(
                configured: snapshot.configuration.language,
                deviceLocale: Locale.autoupdatingCurrent.identifier
            ),
            speed: snapshot.configuration.rate,
            route: route,
            maxPayloadBytes: IOSAudioService.shared.maximumPayloadBytes,
            configurationRevision: snapshot.revision,
            configurationSnapshot: snapshot,
            audioOwner: owner
        )
    }

    private func startBargeInMonitoring(
        _ context: FlowResponseContext,
        resumeSpeechOnSuccess: Bool = false
    ) {
        guard let bargeInRecognizer,
              context.audioConfigurationSnapshot.configuration.recognition.source != .provider,
              context.audioConfigurationSnapshot.configuration.speech.source != .provider else {
            context.bargeInUnavailable = true
            detailOverride = String(localized: "voice_bargein_unavailable_tap_orb")
            context.speechSession?.resume()
            enqueuePendingSpeechIfPossible(context)
            finishFlowResponseIfReady(context)
            return
        }

        responseSetupTask?.cancel()
        responseSetupTask = Task { @MainActor [weak self, weak context] in
            guard let self, let context else { return }
            do {
                let session = try await bargeInRecognizer.start(
                    language: context.audioConfigurationSnapshot.configuration.language,
                    prefersOnDevice: context.audioConfigurationSnapshot.configuration.recognition.source == .offline,
                    configurationSnapshot: context.audioConfigurationSnapshot
                )
                guard !Task.isCancelled,
                      self.generation == context.operation,
                      self.flowContext === context,
                      self.mode == .flow
                else {
                    await session.stop()
                    return
                }
                context.bargeInSession = session
                context.bargeInUnavailable = false
                self.responseSetupTask = nil
                if resumeSpeechOnSuccess {
                    context.isInterrupting = false
                    context.speechSession?.resume()
                    self.detailOverride = nil
                    self.transition(to: context.speechSession == nil ? .thinking : .speaking)
                }
                self.consumeBargeInEvents(session, context: context)
                self.enqueuePendingSpeechIfPossible(context)
                self.finishFlowResponseIfReady(context)
            } catch is CancellationError {
                return
            } catch {
                guard self.generation == context.operation,
                      self.flowContext === context
                else { return }
                self.responseSetupTask = nil
                context.bargeInSession = nil
                context.bargeInUnavailable = true
                if resumeSpeechOnSuccess {
                    context.isInterrupting = false
                    context.speechSession?.resume()
                    self.transition(to: context.speechSession == nil ? .thinking : .speaking)
                }
                self.detailOverride = String(localized: "voice_bargein_unavailable_tap_orb")
                self.enqueuePendingSpeechIfPossible(context)
                self.finishFlowResponseIfReady(context)
            }
        }
    }

    private func consumeBargeInEvents(
        _ session: any VoiceBargeInSession,
        context: FlowResponseContext
    ) {
        bargeInEventTask?.cancel()
        bargeInEventTask = Task { @MainActor [weak self, weak context] in
            for await event in session.events {
                guard !Task.isCancelled,
                      let self,
                      let context,
                      self.generation == context.operation,
                      self.flowContext === context,
                      context.bargeInSession === session
                else { return }
                self.handleBargeInEvent(event, context: context)
            }
        }
    }

    private func handleBargeInEvent(
        _ event: VoiceBargeInEvent,
        context: FlowResponseContext
    ) {
        switch event {
        case .speechStarted:
            guard !context.isInterrupting else { return }
            context.isInterrupting = true
            context.speechSession?.pause()
            caption = ""
            detailOverride = String(localized: "voice_detecting_speech")
            transition(to: .interrupting)

        case let .partial(text):
            guard context.isInterrupting else { return }
            caption = text

        case let .transcript(text):
            guard context.isInterrupting else { return }
            commitBargeInTranscript(text, context: context)

        case .empty:
            recoverFromFalseBargeIn(context)

        case let .failed(message):
            if context.isInterrupting {
                let operation = context.operation
                Task { @MainActor [weak self, weak context] in
                    guard let context else { return }
                    await context.speechSession?.stop()
                    guard let self,
                          self.generation == operation,
                          self.flowContext === context,
                          self.mode == .flow
                    else { return }
                    self.fail(String(localized: "voice_bargein_failed_message \(message)"))
                }
            } else {
                context.bargeInSession = nil
                context.bargeInUnavailable = true
                detailOverride = String(localized: "voice_bargein_unavailable_tap_orb")
                enqueuePendingSpeechIfPossible(context)
            }
        }
    }

    private func recoverFromFalseBargeIn(_ context: FlowResponseContext) {
        guard context.isInterrupting else { return }
        context.bargeInSession = nil
        caption = ""
        if let completion = context.terminalCompletion,
           completion.outcome != .completed {
            context.isInterrupting = false
            handleTurnCompletion(completion)
            return
        }
        detailOverride = String(localized: "voice_resuming_no_new_question")
        startBargeInMonitoring(context, resumeSpeechOnSuccess: true)
    }

    private func commitBargeInTranscript(
        _ rawText: String,
        context: FlowResponseContext
    ) {
        let text = rawText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let source else {
            recoverFromFalseBargeIn(context)
            return
        }

        pendingInterruptionTranscript = text
        suppressedFlowToken = context.token
        isCancellingFlowTurn = true
        caption = text
        detailOverride = String(localized: "voice_stopping_previous_sending_new")
        transition(to: .paused)
        let operation = nextGeneration()
        responseSetupTask?.cancel()
        responseSetupTask = nil
        bargeInEventTask?.cancel()
        bargeInEventTask = nil
        streamOpenTask?.cancel()
        streamOpenTask = nil
        speechTask?.cancel()

        transitionTask?.cancel()
        transitionTask = Task { @MainActor [weak self] in
            guard let self else { return }
            await context.speechSession?.stop()
            await context.bargeInSession?.stop()
            do {
                if source.model.streaming || source.model.isCancelling {
                    try await self.awaitCancellation(of: source)
                }
            } catch {
                guard self.generation == operation, self.mode == .flow else { return }
                self.isCancellingFlowTurn = false
                self.fail(String(localized: "voice_stop_previous_failed \(error.localizedDescription)"), preservingCaption: true)
                return
            }
            guard self.generation == operation, self.mode == .flow else { return }
            self.isCancellingFlowTurn = false
            self.activeFlowToken = nil
            self.suppressedFlowToken = nil
            self.flowContext = nil
            self.pendingInterruptionTranscript = nil
            self.submitFlowTranscript(text)
        }
    }

    private func enqueuePendingSpeechIfPossible(_ context: FlowResponseContext) {
        guard flowContext === context,
              generation == context.operation,
              !context.isInterrupting,
              !context.pendingSegments.isEmpty,
              !flowPausedInBackground
        else { return }

        if let session = context.speechSession {
            let segments = context.pendingSegments
            context.pendingSegments.removeAll()
            segments.forEach(session.enqueue)
            caption = segments.last ?? caption
            transition(to: .speaking)
            return
        }

        guard !context.isOpeningSpeech else { return }
        if bargeInRecognizer != nil,
           context.bargeInSession == nil,
           !context.bargeInUnavailable {
            return
        }

        context.isOpeningSpeech = true
        streamOpenTask?.cancel()
        streamOpenTask = Task { @MainActor [weak self, weak context] in
            guard let self, let context else { return }
            do {
                let session = try await self.speechPlayer.openStream(
                    configuration: context.speechConfiguration,
                    managesAudioSession: context.bargeInSession == nil
                )
                guard !Task.isCancelled,
                      self.generation == context.operation,
                      self.flowContext === context,
                      !context.isInterrupting
                else {
                    // A barge-in that later resolves `.empty` keeps this very
                    // context alive, so the flag must be released before the
                    // stop or `enqueuePendingSpeechIfPossible` and
                    // `finishFlowResponseIfReady` stay wedged on it forever.
                    context.isOpeningSpeech = false
                    await session.stop()
                    return
                }
                context.isOpeningSpeech = false
                context.speechSession = session
                self.streamOpenTask = nil
                self.enqueuePendingSpeechIfPossible(context)
                self.finishFlowResponseIfReady(context)
            } catch is CancellationError {
                context.isOpeningSpeech = false
                return
            } catch {
                guard self.generation == context.operation,
                      self.flowContext === context
                else { return }
                context.isOpeningSpeech = false
                self.pauseFlowResponseForFailure(
                    context,
                    message: String(localized: "voice_playback_failed_message \(error.localizedDescription)")
                )
            }
        }
    }

    private func finishFlowResponseIfReady(_ context: FlowResponseContext) {
        guard flowContext === context,
              generation == context.operation,
              context.terminalCompletion?.outcome == .completed,
              !context.isInterrupting,
              !context.isOpeningSpeech,
              context.pendingSegments.isEmpty,
              !context.isFinishingSpeech
        else { return }

        context.isFinishingSpeech = true
        speechTask?.cancel()
        speechTask = Task { @MainActor [weak self, weak context] in
            guard let self, let context else { return }
            let outcome: VoiceSpeechPlaybackOutcome
            do {
                if let session = context.speechSession {
                    outcome = try await session.finish()
                } else {
                    outcome = .completed
                }
            } catch is CancellationError {
                return
            } catch {
                guard self.generation == context.operation,
                      self.flowContext === context
                else { return }
                self.pauseFlowResponseForFailure(
                    context,
                    message: String(localized: "voice_playback_failed_message \(error.localizedDescription)")
                )
                return
            }

            guard self.generation == context.operation,
                  self.flowContext === context,
                  self.mode == .flow
            else { return }
            await context.bargeInSession?.stop()
            guard !Task.isCancelled,
                  self.generation == context.operation,
                  self.flowContext === context,
                  self.mode == .flow
            else { return }
            self.bargeInEventTask?.cancel()
            self.bargeInEventTask = nil
            self.flowContext = nil
            self.activeFlowToken = nil
            self.suppressedFlowToken = nil
            self.flowPausedInBackground = false
            self.caption = ""
            switch outcome {
            case .completed:
                self.detailOverride = nil
                if self.finishFlowAfterNavigation {
                    self.close(cancelOwnedTurn: false)
                } else {
                    self.scheduleListening(after: self.loopDelay)
                }
            case .interrupted:
                self.detailOverride = String(localized: "voice_playback_interrupted_retry")
                self.transition(to: .paused)
            }
        }
    }

    private func stopFlowResponse(_ context: FlowResponseContext) {
        responseSetupTask?.cancel()
        responseSetupTask = nil
        bargeInEventTask?.cancel()
        bargeInEventTask = nil
        streamOpenTask?.cancel()
        streamOpenTask = nil
        speechTask?.cancel()
        speechTask = nil
        stopFlowAudioDetached(context)
        if flowContext === context { flowContext = nil }
        if activeFlowToken == context.token { activeFlowToken = nil }
    }

    private func pauseFlowResponseForFailure(
        _ context: FlowResponseContext,
        message: String
    ) {
        responseSetupTask?.cancel()
        responseSetupTask = nil
        bargeInEventTask?.cancel()
        bargeInEventTask = nil
        streamOpenTask = nil
        speechTask = nil
        stopFlowAudioDetached(context)
        if finishFlowAfterNavigation, context.terminalCompletion != nil {
            close(cancelOwnedTurn: false)
            return
        }
        fail(message)
    }

    private func handleAutomaticTurnCompletion(
        _ completion: ConversationTurnCompletion,
        allowPlayback: Bool
    ) {
        guard completion.token == automaticPlaybackToken else { return }
        guard completion.outcome == .completed,
              capability.autoPlay,
              currentReadiness.ttsReady
        else {
            automaticPlaybackToken = nil
            automaticPlaybackOwner = nil
            automaticPlaybackSnapshot = nil
            return
        }
        let text = Self.spokenText(from: completion.finalAssistantText)
        guard !text.isEmpty else {
            automaticPlaybackToken = nil
            automaticPlaybackOwner = nil
            automaticPlaybackSnapshot = nil
            return
        }
        guard allowPlayback, mode == nil, automaticPlaybackCleanup == nil,
              ownership.acquireAutomatic(self) else {
            pendingAutomaticCompletion = completion
            ownership.enqueueAutomatic(self)
            return
        }
        let snapshot = automaticPlaybackSnapshot ?? capability.configurationSnapshot
        let audioOwner = automaticPlaybackOwner
        automaticPlaybackToken = nil
        automaticPlaybackOwner = nil
        automaticPlaybackSnapshot = nil
        pendingAutomaticCompletion = nil

        let operation = nextGeneration()
        let route = AudioConfigurationRuntime.route(kind: .speech, snapshot: snapshot)
        let request = VoiceSpeechRequest(
            text: text,
            voiceIdentifier: route.effective?.voiceId ?? VoiceOptionIdentity.systemDefault,
            languageIdentifier: resolveAudioLanguageForNativeDevice(
                configured: snapshot.configuration.language,
                deviceLocale: Locale.autoupdatingCurrent.identifier
            ),
            speed: snapshot.configuration.rate,
            route: route,
            maxPayloadBytes: IOSAudioService.shared.maximumPayloadBytes,
            configurationRevision: snapshot.revision,
            configurationSnapshot: snapshot,
            audioOwner: audioOwner
        )
        speechTask?.cancel()
        speechTask = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.generation == operation, self.mode == nil, !self.automaticStopUnconfirmed {
                    self.speechTask = nil
                    self.ownership.release(self)
                }
            }
            do {
                _ = try await self.speechPlayer.speak(request)
                try Task.checkCancellation()
                guard self.generation == operation, self.mode == nil else { return }
            } catch {
                // Ordinary auto-play is optional and has no inline voice panel;
                // the next explicit voice action remains available after failure.
            }
        }
    }

    private func cancelFlowTurnAndRelisten() {
        guard mode == .flow,
              activeFlowToken != nil,
              !isCancellingFlowTurn,
              let source
        else { return }
        let operation = nextGeneration()
        suppressedFlowToken = activeFlowToken
        isCancellingFlowTurn = true
        detailOverride = String(localized: "voice_stopping_current_reply")
        transition(to: .paused)
        responseSetupTask?.cancel()
        responseSetupTask = nil
        bargeInEventTask?.cancel()
        bargeInEventTask = nil
        streamOpenTask?.cancel()
        streamOpenTask = nil
        speechTask?.cancel()
        let context = flowContext
        transitionTask?.cancel()
        transitionTask = Task { @MainActor [weak self] in
            await context?.speechSession?.stop()
            await context?.bargeInSession?.stop()
            do {
                if source.model.streaming || source.model.isCancelling {
                    try await self?.awaitCancellation(of: source)
                }
            } catch {
                guard let self, self.generation == operation else { return }
                self.isCancellingFlowTurn = false
                self.fail(String(localized: "voice_stop_failed_message \(error.localizedDescription)"))
                return
            }
            guard let self, self.generation == operation, self.mode == .flow else { return }
            self.activeFlowToken = nil
            self.suppressedFlowToken = nil
            self.isCancellingFlowTurn = false
            self.flowPausedInBackground = false
            self.flowContext = nil
            self.pendingInterruptionTranscript = nil
            self.scheduleListening()
        }
    }

    private func awaitCancellation(of source: any ConversationSource) async throws {
        let sourceID = ObjectIdentifier(source)
        let task: Task<Void, Error>
        if let cancellationTask, cancellationSourceID == sourceID {
            task = cancellationTask
        } else {
            task = Task { @MainActor in
                try await source.cancelAndWait()
            }
            cancellationTask = task
            cancellationSourceID = sourceID
        }

        do {
            try await task.value
            if cancellationSourceID == sourceID {
                cancellationTask = nil
                cancellationSourceID = nil
            }
        } catch {
            if cancellationSourceID == sourceID {
                cancellationTask = nil
                cancellationSourceID = nil
            }
            throw error
        }
    }

    private func startDetachedCancellation(of source: any ConversationSource) {
        Task { @MainActor [weak self] in
            try? await self?.awaitCancellation(of: source)
        }
    }

    private func finishBackgroundPausedFlowTurn(
        _ context: FlowResponseContext,
        completion: ConversationTurnCompletion
    ) {
        if flowContext === context { flowContext = nil }
        if activeFlowToken == context.token { activeFlowToken = nil }
        suppressedFlowToken = nil
        pendingInterruptionTranscript = nil
        isCancellingFlowTurn = false
        flowPausedInBackground = false
        caption = ""
        if finishFlowAfterNavigation {
            close(cancelOwnedTurn: false)
            return
        }
        switch completion.outcome {
        case .completed:
            detailOverride = String(localized: "voice_stopped_in_background")
        case .cancelled:
            detailOverride = String(localized: "voice_turn_cancelled_retry")
        case .maxTurns:
            detailOverride = String(localized: "voice_turn_max_turns")
        case .failed:
            detailOverride = String(localized: "voice_turn_failed")
        }
        transition(to: .paused)
    }

    private func stopPlaybackAndWait() async {
        guard let active = speechTask else { return }
        active.cancel()
        speechPlayer.stop()
        await active.value
        speechTask = nil
    }

    private func fail(_ message: String, preservingCaption: Bool = false) {
        voiceCapture.cancel()
        if !preservingCaption { caption = "" }
        detailOverride = message
        resumeAfterConfiguration = false
        transition(to: .failed)
    }

    @discardableResult
    private func nextGeneration() -> UInt64 {
        generation &+= 1
        return generation
    }

    private func invalidateTasks(keepingMode: Bool, cancelOwnedTurn: Bool) {
        nextGeneration()
        captureStartGeneration = nil
        realtimeStartPending = false
        transitionTask?.cancel()
        transitionTask = nil
        speechTask?.cancel()
        speechTask = nil
        responseSetupTask?.cancel()
        responseSetupTask = nil
        bargeInEventTask?.cancel()
        bargeInEventTask = nil
        streamOpenTask?.cancel()
        streamOpenTask = nil
        if cancelOwnedTurn, activeFlowToken != nil {
            source?.cancel()
        }
        if !keepingMode { activeFlowToken = nil }
    }

    private func stopFlowAudioDetached(_ context: FlowResponseContext?) {
        guard let context else { return }
        let previousCleanup = audioCleanupTask?.task
        let cleanupID = nextAudioCleanupID
        nextAudioCleanupID &+= 1
        let task = Task { @MainActor in
            if let previousCleanup { await previousCleanup.value }
            await context.speechSession?.stop()
            await context.bargeInSession?.stop()
        }
        audioCleanupTask = (cleanupID, task)
    }

    private func waitForFlowAudioCleanup() async {
        while let cleanup = audioCleanupTask {
            await cleanup.task.value
            if audioCleanupTask?.id == cleanup.id {
                audioCleanupTask = nil
            }
        }
    }

    private func transition(to newPhase: VoiceInteractionPhase) {
        guard phase != newPhase else { return }
        phase = newPhase
        #if canImport(UIKit)
            UIAccessibility.post(notification: .announcement, argument: statusTitle)
        #endif
    }

    private static func spokenText(from markdown: String) -> String {
        markdown
            .replacingOccurrences(of: "```", with: "")
            .replacingOccurrences(of: "`", with: "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
