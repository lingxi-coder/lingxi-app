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
}

struct VoiceSpeechConfiguration: Equatable {
    let voiceIdentifier: String
    let languageIdentifier: String
    let speed: Double

    func request(text: String) -> VoiceSpeechRequest {
        VoiceSpeechRequest(
            text: text,
            voiceIdentifier: voiceIdentifier,
            languageIdentifier: languageIdentifier,
            speed: speed
        )
    }
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
    func openStream(
        configuration: VoiceSpeechConfiguration,
        managesAudioSession: Bool
    ) async throws -> any VoiceSpeechStreamingSession
}

extension VoiceSpeechPlaying {
    func openStream(
        configuration: VoiceSpeechConfiguration,
        managesAudioSession _: Bool
    ) async throws -> any VoiceSpeechStreamingSession {
        BufferedVoiceSpeechSession(player: self, configuration: configuration)
    }
}

/// Compatibility path for injected previews/tests that only implement the old
/// one-shot player. Production uses `SystemVoiceSpeechStream`, which speaks as
/// soon as each segment is enqueued.
@MainActor
private final class BufferedVoiceSpeechSession: VoiceSpeechStreamingSession {
    private weak var player: (any VoiceSpeechPlaying)?
    private let configuration: VoiceSpeechConfiguration
    private var segments: [String] = []
    private var stopped = false

    init(player: any VoiceSpeechPlaying, configuration: VoiceSpeechConfiguration) {
        self.player = player
        self.configuration = configuration
    }

    func enqueue(_ text: String) {
        guard !stopped, !text.isEmpty else { return }
        segments.append(text)
    }

    func pause() {}
    func resume() {}

    func finish() async throws -> VoiceSpeechPlaybackOutcome {
        guard !stopped, let player else { return .interrupted }
        for segment in segments {
            try Task.checkCancellation()
            let outcome = try await player.speak(configuration.request(text: segment))
            guard outcome == .completed else { return outcome }
        }
        segments = []
        return .completed
    }

    func stop() async {
        stopped = true
        segments = []
        player?.stop()
    }
}

@MainActor
final class SystemVoiceSpeechPlayer: VoiceSpeechPlaying {
    private var activeStream: (any VoiceSpeechStreamingSession)?
    private var activeStreamID: ObjectIdentifier?

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        let stream = try await openStream(
            configuration: VoiceSpeechConfiguration(
                voiceIdentifier: request.voiceIdentifier,
                languageIdentifier: request.languageIdentifier,
                speed: request.speed
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

    func openStream(
        configuration: VoiceSpeechConfiguration,
        managesAudioSession: Bool
    ) async throws -> any VoiceSpeechStreamingSession {
        if let activeStream {
            await activeStream.stop()
        }
        let lease = managesAudioSession
            ? try await VoiceAudioSessionCoordinator.shared.acquire(.playback)
            : nil
        if let selection = VoiceRuntimeResolver.parseSherpaVoice(configuration.voiceIdentifier),
           let model = GeneratedVoiceModelCatalog.byID(selection.modelID),
           let directory = VoiceModelFiles.modelRoot(for: model) {
            let stream = SherpaVoiceSpeechStream(
                configuration: configuration,
                modelID: model.id,
                modelDirectory: directory,
                speakerID: Int32(model.voices.firstIndex(where: { $0.id == selection.voiceID }) ?? 0),
                audioLease: lease
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
        let stream = SystemVoiceSpeechStream(configuration: configuration, audioLease: lease)
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
}

@MainActor
final class SystemVoiceSpeechStream: NSObject, VoiceSpeechStreamingSession, AVSpeechSynthesizerDelegate {
    private let configuration: VoiceSpeechConfiguration
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
        audioLease: VoiceAudioSessionCoordinator.Lease?
    ) {
        self.configuration = configuration
        self.audioLease = audioLease
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
        let systemVoiceIdentifier = configuration.voiceIdentifier.hasPrefix("system:")
            ? String(configuration.voiceIdentifier.dropFirst("system:".count))
            : configuration.voiceIdentifier
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
        Task { @MainActor [weak self] in
            if let lease {
                await VoiceAudioSessionCoordinator.shared.release(lease)
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
    static let shared = VoicePreviewPlayback(player: SystemVoiceSpeechPlayer())

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
        let task = activeTask
        activeID = nil
        activeTask = nil
        task?.cancel()
        player.stop()
        if let task {
            _ = await task.result
        }
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

    init(token: ConversationTurnToken, operation: UInt64) {
        self.token = token
        self.operation = operation
    }
}

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
    private let loopDelay: Duration
    private let flowSilenceInterval: Duration
    private var source: (any ConversationSource)?
    private var flowContext: FlowResponseContext?
    private var activeFlowToken: ConversationTurnToken?
    private var suppressedFlowToken: ConversationTurnToken?
    private var automaticPlaybackToken: ConversationTurnToken?
    private var dictationCompletion: ((String) -> Void)?
    private var transitionTask: Task<Void, Never>?
    private var speechTask: Task<Void, Never>?
    private var responseSetupTask: Task<Void, Never>?
    private var bargeInEventTask: Task<Void, Never>?
    private var streamOpenTask: Task<Void, Never>?
    private var audioCleanupTask: (id: UInt64, task: Task<Void, Never>)?
    private var nextAudioCleanupID: UInt64 = 1
    private var cancellationTask: Task<Void, Error>?
    private var cancellationSourceID: ObjectIdentifier?
    private var isCancellingFlowTurn = false
    private var pendingInterruptionTranscript: String?
    private var resumeAfterConfiguration = false
    private var generation: UInt64 = 0

    init() {
        voiceCapture = VoiceCapture()
        let capability = VoiceCapabilityModel()
        self.capability = capability
        speechPlayer = SystemVoiceSpeechPlayer()
        bargeInRecognizer = VoiceBargeInRecognizer()
        readinessOverride = nil
        permissionRequester = { await capability.requestPermissions() }
        loopDelay = .milliseconds(350)
        flowSilenceInterval = .milliseconds(1_200)
    }

    init(
        voiceCapture: VoiceCapture,
        capability: VoiceCapabilityModel,
        speechPlayer: any VoiceSpeechPlaying,
        bargeInRecognizer: (any VoiceBargeInRecognizing)? = nil,
        readinessOverride: (@MainActor () -> VoiceConfigurationReadiness)? = nil,
        permissionRequester: (@MainActor () async -> Void)? = nil,
        loopDelay: Duration = .milliseconds(350),
        flowSilenceInterval: Duration = .milliseconds(1_200)
    ) {
        self.voiceCapture = voiceCapture
        self.capability = capability
        self.speechPlayer = speechPlayer
        self.bargeInRecognizer = bargeInRecognizer
        self.readinessOverride = readinessOverride
        self.permissionRequester = permissionRequester ?? {
            await capability.requestPermissions()
        }
        self.loopDelay = loopDelay
        self.flowSilenceInterval = flowSilenceInterval
    }

    var isPresented: Bool { mode != nil }
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

    func startDictation(onTranscript: @escaping (String) -> Void) {
        guard mode != .flow else { return }
        guard phase != .requestingPermission else { return }
        beginOperation(mode: .dictation)
        dictationCompletion = onTranscript
        guard currentReadiness.isReadyForDictation else {
            recoverUnavailablePermissionsOrRequireConfiguration(for: .dictation)
            return
        }
        scheduleListening()
    }

    func startFlow(source: any ConversationSource) {
        guard mode != .flow else { return }
        guard mode != .dictation || voiceCapture.phase == .idle else { return }
        beginOperation(mode: .flow)
        self.source = source
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
        capability.reloadFromDefaults()
        detailOverride = nil
        guard let mode else { return }
        let ready = mode == .flow
            ? currentReadiness.isReadyForFlow
            : currentReadiness.isReadyForDictation
        guard ready else {
            recoverUnavailablePermissionsOrRequireConfiguration(for: mode)
            return
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
    func registerAutomaticPlaybackCandidate(_ token: ConversationTurnToken) {
        automaticPlaybackToken = token
        if speechTask != nil {
            nextGeneration()
            speechTask?.cancel()
            speechPlayer.stop()
        }
    }

    func handleTurnSpeechUpdate(_ update: ConversationTurnSpeechUpdate) {
        guard mode == .flow,
              let context = flowContext,
              context.token == activeFlowToken,
              update.token == context.token,
              update.sequence > context.lastSpeechSequence,
              context.token != suppressedFlowToken,
              !isCancellingFlowTurn
        else { return }

        context.lastSpeechSequence = update.sequence
        context.pendingSegments.append(contentsOf: context.segmenter.append(update.delta))
        enqueuePendingSpeechIfPossible(context)
    }

    func handleTurnCompletion(_ completion: ConversationTurnCompletion) {
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
        automaticPlaybackToken = nil
        if mode == nil {
            invalidateTasks(keepingMode: true, cancelOwnedTurn: false)
            speechPlayer.stop()
            return
        }
        let ownedSource = activeFlowToken == nil ? nil : source
        let context = flowContext
        invalidateTasks(keepingMode: true, cancelOwnedTurn: false)
        stopFlowAudioDetached(context)
        voiceCapture.cancel()
        speechPlayer.stop()
        activeFlowToken = nil
        suppressedFlowToken = nil
        flowContext = nil
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
        close()
    }

    func close() {
        let ownedSource = activeFlowToken == nil ? nil : source
        let context = flowContext
        invalidateTasks(keepingMode: false, cancelOwnedTurn: false)
        stopFlowAudioDetached(context)
        voiceCapture.cancel()
        speechPlayer.stop()
        activeFlowToken = nil
        suppressedFlowToken = nil
        flowContext = nil
        pendingInterruptionTranscript = nil
        isCancellingFlowTurn = false
        source = nil
        automaticPlaybackToken = nil
        dictationCompletion = nil
        caption = ""
        detailOverride = nil
        resumeAfterConfiguration = false
        mode = nil
        phase = .paused

        if let ownedSource {
            startDetachedCancellation(of: ownedSource)
        }
    }

    private var currentReadiness: VoiceConfigurationReadiness {
        readinessOverride?() ?? capability.configurationReadiness
    }

    private func beginOperation(mode: VoiceInteractionMode) {
        if self.mode != mode {
            let context = flowContext
            invalidateTasks(keepingMode: false, cancelOwnedTurn: false)
            stopFlowAudioDetached(context)
            voiceCapture.cancel()
            speechPlayer.stop()
            activeFlowToken = nil
            suppressedFlowToken = nil
            flowContext = nil
            pendingInterruptionTranscript = nil
            isCancellingFlowTurn = false
            source = nil
            automaticPlaybackToken = nil
            dictationCompletion = nil
        }
        self.mode = mode
        caption = ""
        detailOverride = nil
        resumeAfterConfiguration = false
    }

    private func requireConfiguration(for mode: VoiceInteractionMode) {
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
        transitionTask?.cancel()
        transitionTask = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.permissionRequester()
            guard !Task.isCancelled,
                  self.generation == operation,
                  self.mode == mode
            else { return }

            self.capability.reloadFromDefaults()
            let isReady = mode == .flow
                ? self.currentReadiness.isReadyForFlow
                : self.currentReadiness.isReadyForDictation
            self.transitionTask = nil
            if isReady {
                self.scheduleListening()
            } else {
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
        let operation = nextGeneration()
        transitionTask?.cancel()
        transitionTask = Task { @MainActor [weak self] in
            guard let self else { return }
            if let delay {
                do { try await Task.sleep(for: delay) } catch { return }
            }
            guard !Task.isCancelled, self.generation == operation else { return }
            await self.waitForFlowAudioCleanup()
            guard !Task.isCancelled, self.generation == operation else { return }
            await self.stopPlaybackAndWait()
            guard !Task.isCancelled, self.generation == operation else { return }
            self.beginCapture(operation: operation)
        }
    }

    private func beginCapture(operation: UInt64) {
        guard generation == operation, let mode else { return }
        let ready = mode == .flow
            ? currentReadiness.isReadyForFlow
            : currentReadiness.isReadyForDictation
        guard ready else {
            requireConfiguration(for: mode)
            return
        }
        if mode == .flow,
           let source,
           source.model.streaming || source.model.isCancelling || source.model.sessionTransitionPending {
            fail(String(localized: "voice_previous_turn_not_finished"))
            return
        }

        caption = ""
        detailOverride = nil
        resumeAfterConfiguration = false
        transition(to: .listening)
        voiceCapture.start(
            language: capability.language,
            automaticEndpointAfterSilence: mode == .flow ? flowSilenceInterval : nil
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
            handleEmptyCapture(mode: mode)
        case .permissionDenied:
            capability.reloadFromDefaults()
            requireConfiguration(for: mode)
        case .failed(let message):
            fail(String(localized: "voice_recognition_failed_message \(message)"))
        }
    }

    private func handleEmptyCapture(mode: VoiceInteractionMode) {
        if mode == .flow {
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
        guard let token = source.send(text) else {
            fail(String(localized: "voice_session_busy_retry"))
            return
        }
        activeFlowToken = token
        suppressedFlowToken = nil
        pendingInterruptionTranscript = nil
        let operation = nextGeneration()
        let context = FlowResponseContext(token: token, operation: operation)
        flowContext = context
        startBargeInMonitoring(context)
    }

    private var speechConfiguration: VoiceSpeechConfiguration {
        VoiceSpeechConfiguration(
            voiceIdentifier: capability.selectedVoice?.id
                ?? VoicePreferencesSnapshot.defaultVoiceSelection,
            languageIdentifier: capability.effectiveLanguageIdentifier,
            speed: capability.speed
        )
    }

    private func startBargeInMonitoring(
        _ context: FlowResponseContext,
        resumeSpeechOnSuccess: Bool = false
    ) {
        guard let bargeInRecognizer else {
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
                    language: self.capability.language,
                    prefersOnDevice: self.capability.mode == .onDevice
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
              !context.pendingSegments.isEmpty
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
                    configuration: self.speechConfiguration,
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
            self.bargeInEventTask?.cancel()
            self.bargeInEventTask = nil
            self.flowContext = nil
            self.activeFlowToken = nil
            self.suppressedFlowToken = nil
            self.caption = ""
            switch outcome {
            case .completed:
                self.detailOverride = nil
                self.scheduleListening(after: self.loopDelay)
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
        fail(message)
    }

    private func handleAutomaticTurnCompletion(
        _ completion: ConversationTurnCompletion,
        allowPlayback: Bool
    ) {
        guard completion.token == automaticPlaybackToken else { return }
        automaticPlaybackToken = nil
        guard allowPlayback,
              completion.outcome == .completed,
              capability.autoPlay,
              currentReadiness.ttsReady
        else { return }
        let text = Self.spokenText(from: completion.finalAssistantText)
        guard !text.isEmpty else { return }

        let operation = nextGeneration()
        let request = VoiceSpeechRequest(
            text: text,
            voiceIdentifier: capability.selectedVoice?.id
                ?? VoicePreferencesSnapshot.defaultVoiceSelection,
            languageIdentifier: capability.effectiveLanguageIdentifier,
            speed: capability.speed
        )
        speechTask?.cancel()
        speechTask = Task { @MainActor [weak self] in
            guard let self else { return }
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

    private func stopPlaybackAndWait() async {
        let active = speechTask
        active?.cancel()
        speechPlayer.stop()
        await active?.value
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
