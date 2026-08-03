import AVFoundation
import Observation
import SwiftUI

#if canImport(UIKit)
    import UIKit
#endif

enum VoiceInteractionMode: Equatable {
    case dictation
    case flow
}

enum VoiceInteractionPhase: Equatable {
    case configurationRequired
    case listening
    case recognizing
    case thinking
    case speaking
    case paused
    case failed
}

struct VoiceSpeechRequest: Equatable {
    let text: String
    let voiceIdentifier: String
    let languageIdentifier: String
    let speed: Double
}

enum VoiceSpeechPlaybackOutcome: Equatable {
    case completed
    case interrupted
}

@MainActor
protocol VoiceSpeechPlaying: AnyObject {
    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome
    func stop()
}

@MainActor
final class SystemVoiceSpeechPlayer: VoiceSpeechPlaying {
    private let synthesizer = AVSpeechSynthesizer()
    private var activePlaybackID: UUID?
    private var playbackInterrupted = false

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        let lease = try await VoiceAudioSessionCoordinator.shared.acquire(.playback)
        let playbackID = UUID()
        activePlaybackID = playbackID
        playbackInterrupted = false
        let observers = installInterruptionObservers(playbackID: playbackID)
        defer {
            observers.forEach(NotificationCenter.default.removeObserver)
            if activePlaybackID == playbackID {
                activePlaybackID = nil
                playbackInterrupted = false
            }
        }
        do {
            try Task.checkCancellation()
            let utterance = AVSpeechUtterance(string: request.text)
            utterance.voice = AVSpeechSynthesisVoice(identifier: request.voiceIdentifier)
                ?? AVSpeechSynthesisVoice(language: request.languageIdentifier)
            utterance.rate = VoiceCapabilityModel.utteranceRate(from: request.speed)
            synthesizer.speak(utterance)
            while synthesizer.isSpeaking, !playbackInterrupted {
                try await Task.sleep(for: .milliseconds(50))
            }
            try Task.checkCancellation()
            let outcome: VoiceSpeechPlaybackOutcome = playbackInterrupted ? .interrupted : .completed
            if outcome == .interrupted {
                synthesizer.stopSpeaking(at: .immediate)
            }
            await VoiceAudioSessionCoordinator.shared.release(lease)
            return outcome
        } catch {
            synthesizer.stopSpeaking(at: .immediate)
            await VoiceAudioSessionCoordinator.shared.release(lease)
            throw error
        }
    }

    func stop() {
        synthesizer.stopSpeaking(at: .immediate)
    }

    private func installInterruptionObservers(playbackID: UUID) -> [NSObjectProtocol] {
        let center = NotificationCenter.default
        let interruption = center.addObserver(
            forName: AVAudioSession.interruptionNotification,
            object: AVAudioSession.sharedInstance(),
            queue: .main
        ) { [weak self] notification in
            let raw = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt
            guard raw == AVAudioSession.InterruptionType.began.rawValue else { return }
            Task { @MainActor [weak self] in
                self?.markInterrupted(playbackID: playbackID)
            }
        }
        let routeChange = center.addObserver(
            forName: AVAudioSession.routeChangeNotification,
            object: AVAudioSession.sharedInstance(),
            queue: .main
        ) { [weak self] notification in
            guard
                let raw = notification.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt,
                AVAudioSession.RouteChangeReason(rawValue: raw) == .oldDeviceUnavailable
            else { return }
            Task { @MainActor [weak self] in
                self?.markInterrupted(playbackID: playbackID)
            }
        }
        return [interruption, routeChange]
    }

    private func markInterrupted(playbackID: UUID) {
        guard activePlaybackID == playbackID else { return }
        playbackInterrupted = true
        synthesizer.stopSpeaking(at: .immediate)
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

/// Main-actor state machine for both chat dictation and hands-free Flow Mode.
/// A generation is captured by every async/callback edge so a closed panel or
/// switched conversation cannot be resurrected by late Speech/TTS/turn events.
@Observable
@MainActor
final class VoiceInteractionController {
    private(set) var mode: VoiceInteractionMode?
    private(set) var phase: VoiceInteractionPhase = .paused
    private(set) var caption = ""
    private(set) var detailOverride: String?

    let voiceCapture: VoiceCapture
    let capability: VoiceCapabilityModel

    private let speechPlayer: any VoiceSpeechPlaying
    private let readinessOverride: (@MainActor () -> VoiceConfigurationReadiness)?
    private let loopDelay: Duration
    private var source: (any ConversationSource)?
    private var activeFlowToken: ConversationTurnToken?
    private var suppressedFlowToken: ConversationTurnToken?
    private var automaticPlaybackToken: ConversationTurnToken?
    private var dictationCompletion: ((String) -> Void)?
    private var transitionTask: Task<Void, Never>?
    private var speechTask: Task<Void, Never>?
    private var cancellationTask: Task<Void, Error>?
    private var cancellationSourceID: ObjectIdentifier?
    private var isCancellingFlowTurn = false
    private var resumeAfterConfiguration = false
    private var generation: UInt64 = 0

    init() {
        voiceCapture = VoiceCapture()
        capability = VoiceCapabilityModel()
        speechPlayer = SystemVoiceSpeechPlayer()
        readinessOverride = nil
        loopDelay = .milliseconds(350)
    }

    init(
        voiceCapture: VoiceCapture,
        capability: VoiceCapabilityModel,
        speechPlayer: any VoiceSpeechPlaying,
        readinessOverride: (@MainActor () -> VoiceConfigurationReadiness)? = nil,
        loopDelay: Duration = .milliseconds(350)
    ) {
        self.voiceCapture = voiceCapture
        self.capability = capability
        self.speechPlayer = speechPlayer
        self.readinessOverride = readinessOverride
        self.loopDelay = loopDelay
    }

    var isPresented: Bool { mode != nil }
    var capturePhase: VoiceCapturePhase { voiceCapture.phase }

    var orbPhase: OrbPhase {
        switch phase {
        case .listening, .recognizing: return .listening
        case .thinking: return .thinking
        case .speaking: return .speaking
        case .configurationRequired, .paused, .failed: return .idle
        }
    }

    var statusTitle: String {
        switch phase {
        case .configurationRequired: return "等待配置"
        case .listening: return "正在聆听"
        case .recognizing: return "正在识别"
        case .thinking: return "Agent 正在思考"
        case .speaking: return "正在播报"
        case .paused: return "已暂停"
        case .failed: return "语音暂不可用"
        }
    }

    var statusDetail: String {
        if let detailOverride, !detailOverride.isEmpty { return detailOverride }
        switch phase {
        case .configurationRequired:
            guard let mode else { return "请先完成语音识别与系统声音设置" }
            return configurationMessage(for: mode)
        case .listening:
            return mode == .flow ? "说完后轻点光球收音" : "说完后松开或再次点击"
        case .recognizing: return "正在整理刚才的语音…"
        case .thinking: return "回复完成后会自动播报"
        case .speaking: return "轻点光球可打断并继续说话"
        case .paused: return "轻点重试恢复语音"
        case .failed: return "请检查设置后重试"
        }
    }

    var statusColor: Color {
        switch phase {
        case .listening, .recognizing: return Color(okl: 0.72, 0.18, 150)
        case .speaking: return Color(okl: 0.75, 0.19, 300)
        case .configurationRequired, .failed: return Color(okl: 0.75, 0.18, 50)
        case .thinking, .paused: return Color(okl: 0.70, 0.16, 260)
        }
    }

    var orbAccessibilityLabel: String { statusTitle }

    var orbAccessibilityHint: String {
        switch phase {
        case .listening: return "轻点结束收音"
        case .thinking: return "轻点取消当前回复并重新监听"
        case .speaking: return "轻点停止播报并重新监听"
        case .paused, .failed: return "轻点重试"
        default: return ""
        }
    }

    func startDictation(onTranscript: @escaping (String) -> Void) {
        guard mode != .flow else { return }
        beginOperation(mode: .dictation)
        dictationCompletion = onTranscript
        guard currentReadiness.isReadyForDictation else {
            requireConfiguration(for: .dictation)
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
            requireConfiguration(for: .flow)
            return
        }
        guard !source.model.streaming,
              !source.model.isCancelling,
              !source.model.sessionTransitionPending
        else {
            fail("请等待当前回复或会话切换完成后再开启心流模式")
            return
        }
        scheduleListening()
    }

    func finishListening() {
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
        case .thinking:
            cancelFlowTurnAndRelisten()
        case .speaking:
            stopPlaybackAndRelisten()
        case .paused, .failed:
            retry()
        case .configurationRequired, .recognizing:
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
            requireConfiguration(for: mode)
            return
        }
        if mode == .flow, activeFlowToken != nil {
            cancelFlowTurnAndRelisten()
            return
        }
        if mode == .flow,
           let source,
           source.model.streaming || source.model.isCancelling || source.model.sessionTransitionPending {
            fail("请等待当前回复或会话切换完成后重试")
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

    func handleTurnCompletion(_ completion: ConversationTurnCompletion) {
        guard mode == .flow else {
            handleAutomaticTurnCompletion(completion, allowPlayback: mode == nil)
            return
        }
        guard completion.token == activeFlowToken,
              completion.token != suppressedFlowToken,
              !isCancellingFlowTurn
        else { return }

        activeFlowToken = nil
        suppressedFlowToken = nil
        switch completion.outcome {
        case .completed:
            let text = Self.spokenText(from: completion.finalAssistantText)
            guard !text.isEmpty else {
                caption = ""
                scheduleListening(after: loopDelay)
                return
            }
            caption = text
            transition(to: .speaking)
            speakAndContinue(text)
        case .cancelled:
            fail("本轮已取消，轻点重试继续心流")
        case .maxTurns:
            fail("Agent 已达到最大轮数，请轻点重试")
        case .failed:
            fail("Agent 回复失败，请检查网络或 Provider 后重试")
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
        invalidateTasks(keepingMode: true, cancelOwnedTurn: false)
        voiceCapture.cancel()
        speechPlayer.stop()
        activeFlowToken = nil
        suppressedFlowToken = nil
        isCancellingFlowTurn = false
        caption = ""
        detailOverride = "已在后台停止收音，轻点重试恢复"
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
        invalidateTasks(keepingMode: false, cancelOwnedTurn: false)
        voiceCapture.cancel()
        speechPlayer.stop()
        activeFlowToken = nil
        suppressedFlowToken = nil
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
            invalidateTasks(keepingMode: false, cancelOwnedTurn: false)
            voiceCapture.cancel()
            speechPlayer.stop()
            activeFlowToken = nil
            suppressedFlowToken = nil
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
        return messages.isEmpty ? "请先完成语音设置" : messages.joined(separator: "；")
    }

    private static func compactConfigurationIssue(_ issue: VoiceConfigurationIssue) -> String {
        switch (issue.component, issue.kind) {
        case (.speech, .unconfigured) where issue.message == "请先保存语音识别语言和识别方式":
            return "未保存语音识别配置"
        case (.tts, .unconfigured) where issue.message == "请先选择并保存系统播报声音":
            return "未选择系统播报声音"
        case (.speech, .permissionUndetermined): return "需要语音识别权限"
        case (.microphone, .permissionUndetermined): return "需要麦克风权限"
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
            fail("上一轮尚未结束，请稍后重试")
            return
        }

        caption = ""
        detailOverride = nil
        resumeAfterConfiguration = false
        transition(to: .listening)
        voiceCapture.start(language: capability.language) { [weak self] result in
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
            fail("语音识别失败：\(message)")
        }
    }

    private func handleEmptyCapture(mode: VoiceInteractionMode) {
        if mode == .flow {
            detailOverride = "未听清，正在重新监听…"
            scheduleListening(after: loopDelay)
        } else {
            fail("未识别到语音，请轻点重试")
        }
    }

    private func submitFlowTranscript(_ text: String) {
        guard let source else {
            fail("当前会话不可用")
            return
        }
        caption = text
        transition(to: .thinking)
        guard let token = source.send(text) else {
            fail("当前会话正忙，请稍后重试")
            return
        }
        activeFlowToken = token
        suppressedFlowToken = nil
    }

    private func speakAndContinue(_ text: String) {
        let operation = nextGeneration()
        let request = VoiceSpeechRequest(
            text: text,
            voiceIdentifier: capability.voiceIdentifier,
            languageIdentifier: capability.effectiveLanguageIdentifier,
            speed: capability.speed
        )
        speechTask?.cancel()
        speechTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let outcome = try await self.speechPlayer.speak(request)
                try Task.checkCancellation()
                guard self.generation == operation, self.mode == .flow else { return }
                switch outcome {
                case .completed:
                    self.caption = ""
                    self.scheduleListening(after: self.loopDelay)
                case .interrupted:
                    self.caption = ""
                    self.detailOverride = "语音播报被中断，轻点重试继续心流"
                    self.transition(to: .paused)
                }
            } catch is CancellationError {
                return
            } catch {
                guard self.generation == operation else { return }
                self.fail("语音播报失败：\(error.localizedDescription)")
            }
        }
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
            voiceIdentifier: capability.voiceIdentifier,
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
        detailOverride = "正在停止当前回复…"
        transition(to: .paused)
        transitionTask?.cancel()
        transitionTask = Task { @MainActor [weak self] in
            do {
                try await self?.awaitCancellation(of: source)
            } catch {
                guard let self, self.generation == operation else { return }
                self.isCancellingFlowTurn = false
                self.fail("停止失败：\(error.localizedDescription)")
                return
            }
            guard let self, self.generation == operation, self.mode == .flow else { return }
            self.activeFlowToken = nil
            self.suppressedFlowToken = nil
            self.isCancellingFlowTurn = false
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

    private func stopPlaybackAndRelisten() {
        guard mode == .flow else { return }
        let operation = nextGeneration()
        transitionTask?.cancel()
        transitionTask = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.stopPlaybackAndWait()
            guard self.generation == operation, self.mode == .flow else { return }
            self.scheduleListening()
        }
    }

    private func stopPlaybackAndWait() async {
        let active = speechTask
        active?.cancel()
        speechPlayer.stop()
        await active?.value
        speechTask = nil
    }

    private func fail(_ message: String) {
        voiceCapture.cancel()
        caption = ""
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
        if cancelOwnedTurn, activeFlowToken != nil {
            source?.cancel()
        }
        if !keepingMode { activeFlowToken = nil }
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
