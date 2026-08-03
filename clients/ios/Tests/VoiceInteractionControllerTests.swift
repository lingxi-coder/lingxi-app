import XCTest
@testable import LingxiCode

@MainActor
final class VoiceInteractionControllerTests: XCTestCase {
    func testDictationWritesTranscriptWithoutSendingTurn() async {
        let session = ControllerVoiceSession(transcript: "hello world")
        let controller = makeController(sessions: [session])
        var transcript: String?

        controller.startDictation { transcript = $0 }
        await settle()
        XCTAssertEqual(controller.phase, .listening)

        controller.finishListening()
        await settle()

        XCTAssertEqual(transcript, "hello world")
        XCTAssertFalse(controller.isPresented)
        XCTAssertEqual(controller.capturePhase, .idle)
    }

    func testFlowAutomaticallySendsSpeaksAndListensAgain() async {
        let first = ControllerVoiceSession(transcript: "hello agent")
        let second = ControllerVoiceSession(transcript: "next turn")
        let player = RecordingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [first, second], player: player)

        controller.startFlow(source: source)
        await settle()
        controller.finishListening()
        await settle()

        XCTAssertEqual(source.sentTexts, ["hello agent"])
        XCTAssertEqual(controller.phase, .thinking)
        let token = try! XCTUnwrap(source.lastToken)
        source.complete(token: token, text: "agent reply")
        controller.handleTurnCompletion(try! XCTUnwrap(source.model.turnCompletion))
        await settle()

        XCTAssertEqual(player.requests.map(\.text), ["agent reply"])
        XCTAssertEqual(controller.phase, .listening)
        XCTAssertEqual(second.transcribeCalls, 1)
    }

    func testStaleTurnCompletionCannotTriggerSpeech() async {
        let session = ControllerVoiceSession(transcript: "hello")
        let player = RecordingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [session], player: player)

        controller.startFlow(source: source)
        await settle()
        controller.finishListening()
        await settle()

        controller.handleTurnCompletion(ConversationTurnCompletion(
            token: .init(clientTurnId: 999, sessionEpoch: 1),
            outcome: .completed,
            finalAssistantText: "stale reply"
        ))
        await settle()

        XCTAssertTrue(player.requests.isEmpty)
        XCTAssertEqual(controller.phase, .thinking)
    }

    func testTapDuringSpeechStopsPlaybackBeforeListening() async {
        let first = ControllerVoiceSession(transcript: "hello")
        let second = ControllerVoiceSession(transcript: "interruption")
        let player = BlockingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [first, second], player: player)

        controller.startFlow(source: source)
        await settle()
        controller.finishListening()
        await settle()
        let token = try! XCTUnwrap(source.lastToken)
        source.complete(token: token, text: "a long reply")
        controller.handleTurnCompletion(try! XCTUnwrap(source.model.turnCompletion))
        await settle()
        XCTAssertEqual(controller.phase, .speaking)

        let stopsBeforeInterruption = player.stopCalls
        controller.handleOrbTap()
        await settle()

        XCTAssertGreaterThan(player.stopCalls, stopsBeforeInterruption)
        XCTAssertEqual(controller.phase, .listening)
        XCTAssertEqual(second.transcribeCalls, 1)
    }

    func testPlaybackInterruptionPausesFlowWithoutReopeningMicrophone() async {
        let first = ControllerVoiceSession(transcript: "hello")
        let second = ControllerVoiceSession(transcript: "must stay idle")
        let player = InterruptingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [first, second], player: player)

        controller.startFlow(source: source)
        await settle()
        controller.finishListening()
        await settle()
        let token = try! XCTUnwrap(source.lastToken)
        source.complete(token: token, text: "interrupted reply")
        controller.handleTurnCompletion(try! XCTUnwrap(source.model.turnCompletion))
        await settle()

        XCTAssertEqual(player.requests.map(\.text), ["interrupted reply"])
        XCTAssertEqual(controller.phase, .paused)
        XCTAssertTrue(controller.statusDetail.contains("中断"))
        XCTAssertEqual(second.transcribeCalls, 0)
    }

    func testMissingConfigurationShowsInlineGateWithoutOpeningMicrophone() async {
        let session = ControllerVoiceSession(transcript: "must not run")
        let notReady = VoiceConfigurationReadiness(
            speechConfigured: false,
            ttsConfigured: false,
            speechReady: false,
            ttsReady: false,
            issues: [
                .init(component: .speech, kind: .unconfigured, message: "请配置 Speech"),
                .init(component: .tts, kind: .unconfigured, message: "请配置 TTS"),
            ]
        )
        let controller = makeController(sessions: [session], readiness: notReady)

        controller.startDictation { _ in XCTFail("unconfigured dictation must not complete") }
        await settle()

        XCTAssertEqual(controller.phase, .configurationRequired)
        XCTAssertEqual(controller.statusDetail, "请配置 Speech")
        XCTAssertEqual(session.transcribeCalls, 0)
    }

    func testBackgroundCancelsCaptureAndLateResultCannotResumeFlow() async {
        let session = ControllerVoiceSession(transcript: "late", ignoresCancellation: true)
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [session])

        controller.startFlow(source: source)
        await settle()
        controller.handleBackground()
        session.finishRecording()
        await settle()

        XCTAssertEqual(controller.phase, .paused)
        XCTAssertTrue(source.sentTexts.isEmpty)
        XCTAssertEqual(session.cancelCalls, 1)
    }

    func testConfigurationIntentSurvivesSystemSettingsBackgroundRoundTrip() async {
        let session = ControllerVoiceSession(transcript: "configured")
        let source = ControllerConversationSource()
        let readiness = MutableVoiceReadiness(.notReady)
        let controller = makeController(
            sessions: [session],
            readinessProvider: { readiness.value }
        )

        controller.startFlow(source: source)
        XCTAssertEqual(controller.phase, .configurationRequired)

        controller.handleBackground()
        XCTAssertEqual(controller.phase, .paused)

        readiness.value = .ready
        controller.reloadConfigurationAndResumeIfPossible()
        await settle()

        XCTAssertEqual(controller.phase, .listening)
        XCTAssertEqual(session.transcribeCalls, 1)
    }

    func testOrdinaryTurnAutoPlaybackUsesPreferenceWithoutStartingFlow() async {
        let player = RecordingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [], player: player, autoPlay: true)
        let token = try! XCTUnwrap(source.send("ordinary question"))
        controller.registerAutomaticPlaybackCandidate(token)

        source.complete(token: token, text: "ordinary reply")
        controller.handleTurnCompletion(try! XCTUnwrap(source.model.turnCompletion))
        await settle()

        XCTAssertEqual(player.requests.map(\.text), ["ordinary reply"])
        XCTAssertNil(controller.mode)
        XCTAssertEqual(controller.phase, .paused)
    }

    func testOrdinaryTurnDoesNotAutoPlayWhenPreferenceIsDisabled() async {
        let player = RecordingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [], player: player, autoPlay: false)
        let token = try! XCTUnwrap(source.send("ordinary question"))
        controller.registerAutomaticPlaybackCandidate(token)

        source.complete(token: token, text: "silent reply")
        controller.handleTurnCompletion(try! XCTUnwrap(source.model.turnCompletion))
        await settle()

        XCTAssertTrue(player.requests.isEmpty)
    }

    func testFailedTurnCancellationCanBeRetriedWithoutSpeakingLateCompletion() async {
        let first = ControllerVoiceSession(transcript: "cancel this")
        let second = ControllerVoiceSession(transcript: "after retry")
        let player = RecordingSpeechPlayer()
        let source = ControllerConversationSource(cancellationFailuresRemaining: 1)
        let controller = makeController(sessions: [first, second], player: player)

        controller.startFlow(source: source)
        await settle()
        controller.finishListening()
        await settle()
        let token = try! XCTUnwrap(source.lastToken)

        controller.handleOrbTap()
        await settle()

        XCTAssertEqual(controller.phase, .failed)
        XCTAssertEqual(source.cancelCalls, 1)
        XCTAssertTrue(source.model.streaming)

        source.complete(token: token, text: "late completion must stay silent")
        controller.handleTurnCompletion(try! XCTUnwrap(source.model.turnCompletion))
        await settle()
        XCTAssertTrue(player.requests.isEmpty)

        controller.retry()
        await settle()

        XCTAssertEqual(source.cancelCalls, 2)
        XCTAssertEqual(controller.phase, .listening)
        XCTAssertEqual(second.transcribeCalls, 1)
    }

    private func makeController(
        sessions: [ControllerVoiceSession],
        player: (any VoiceSpeechPlaying)? = nil,
        readiness: VoiceConfigurationReadiness? = nil,
        readinessProvider: (@MainActor () -> VoiceConfigurationReadiness)? = nil,
        autoPlay: Bool = false
    ) -> VoiceInteractionController {
        let factory = ControllerVoiceSessionFactory(sessions: sessions)
        let capture = VoiceCapture(makeSession: { factory.next() })
        let suiteName = "VoiceInteractionControllerTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        let capability = VoiceCapabilityModel(defaults: defaults)
        capability.setAutoPlay(autoPlay)
        let ready = readiness ?? VoiceConfigurationReadiness(
            speechConfigured: true,
            ttsConfigured: true,
            speechReady: true,
            ttsReady: true,
            issues: []
        )
        return VoiceInteractionController(
            voiceCapture: capture,
            capability: capability,
            speechPlayer: player ?? RecordingSpeechPlayer(),
            readinessOverride: readinessProvider ?? { ready },
            loopDelay: .zero
        )
    }

    private func settle() async {
        for _ in 0..<12 { await Task.yield() }
    }
}

@MainActor
private final class ControllerVoiceSessionFactory {
    private var sessions: [ControllerVoiceSession]

    init(sessions: [ControllerVoiceSession]) {
        self.sessions = sessions
    }

    func next() -> ControllerVoiceSession {
        precondition(!sessions.isEmpty, "Test did not provide enough voice sessions")
        return sessions.removeFirst()
    }
}

@MainActor
private final class ControllerVoiceSession: VoiceTranscriptionSession {
    private let transcript: String
    private let ignoresCancellation: Bool
    private var continuation: CheckedContinuation<String, Error>?
    private var pendingFinish = false
    private var pendingCancellation = false

    private(set) var transcribeCalls = 0
    private(set) var finishCalls = 0
    private(set) var cancelCalls = 0

    init(transcript: String, ignoresCancellation: Bool = false) {
        self.transcript = transcript
        self.ignoresCancellation = ignoresCancellation
    }

    func transcribe(language _: String?) async throws -> String {
        transcribeCalls += 1
        return try await withCheckedThrowingContinuation { continuation in
            if pendingCancellation {
                continuation.resume(throwing: CancellationError())
            } else if pendingFinish {
                continuation.resume(returning: transcript)
            } else {
                self.continuation = continuation
            }
        }
    }

    func finishRecording() {
        finishCalls += 1
        pendingFinish = true
        continuation?.resume(returning: transcript)
        continuation = nil
    }

    func cancelRecognition() {
        cancelCalls += 1
        guard !ignoresCancellation else { return }
        pendingCancellation = true
        continuation?.resume(throwing: CancellationError())
        continuation = nil
    }
}

@MainActor
private final class RecordingSpeechPlayer: VoiceSpeechPlaying {
    private(set) var requests: [VoiceSpeechRequest] = []
    private(set) var stopCalls = 0

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        requests.append(request)
        return .completed
    }

    func stop() {
        stopCalls += 1
    }
}

@MainActor
private final class BlockingSpeechPlayer: VoiceSpeechPlaying {
    private(set) var requests: [VoiceSpeechRequest] = []
    private(set) var stopCalls = 0
    private var continuation: CheckedContinuation<Void, Never>?

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        requests.append(request)
        await withCheckedContinuation { continuation = $0 }
        try Task.checkCancellation()
        return .completed
    }

    func stop() {
        stopCalls += 1
        continuation?.resume()
        continuation = nil
    }
}

@MainActor
private final class InterruptingSpeechPlayer: VoiceSpeechPlaying {
    private(set) var requests: [VoiceSpeechRequest] = []

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        requests.append(request)
        return .interrupted
    }

    func stop() {}
}

@MainActor
private final class MutableVoiceReadiness {
    var value: VoiceConfigurationReadiness

    init(_ value: VoiceConfigurationReadiness) {
        self.value = value
    }
}

private extension VoiceConfigurationReadiness {
    static let ready = VoiceConfigurationReadiness(
        speechConfigured: true,
        ttsConfigured: true,
        speechReady: true,
        ttsReady: true,
        issues: []
    )

    static let notReady = VoiceConfigurationReadiness(
        speechConfigured: false,
        ttsConfigured: false,
        speechReady: false,
        ttsReady: false,
        issues: [
            .init(component: .speech, kind: .unconfigured, message: "请配置 Speech"),
            .init(component: .tts, kind: .unconfigured, message: "请配置 TTS"),
        ]
    )
}

@MainActor
private final class ControllerConversationSource: ConversationSource {
    private enum CancellationFailure: LocalizedError {
        case rejected

        var errorDescription: String? { "cancel rejected" }
    }

    let model = ConversationModel()
    private(set) var sentTexts: [String] = []
    private(set) var lastToken: ConversationTurnToken?
    private(set) var cancelCalls = 0
    private var nextTurnID: UInt64 = 1
    private var cancellationFailuresRemaining: Int

    init(cancellationFailuresRemaining: Int = 0) {
        self.cancellationFailuresRemaining = cancellationFailuresRemaining
    }

    func startNewConversation() {
        model.streaming = false
        model.turnCompletion = nil
    }

    @discardableResult
    func send(_ text: String) -> ConversationTurnToken? {
        guard !model.streaming else { return nil }
        let token = ConversationTurnToken(clientTurnId: nextTurnID, sessionEpoch: 1)
        nextTurnID += 1
        sentTexts.append(text)
        lastToken = token
        model.streaming = true
        model.turnCompletion = nil
        return token
    }

    func complete(token: ConversationTurnToken, text: String) {
        model.streaming = false
        model.turnCompletion = ConversationTurnCompletion(
            token: token,
            outcome: .completed,
            finalAssistantText: text
        )
    }

    func cancel() {
        model.streaming = false
    }

    func cancelAndWait() async throws {
        cancelCalls += 1
        if cancellationFailuresRemaining > 0 {
            cancellationFailuresRemaining -= 1
            throw CancellationFailure.rejected
        }
        cancel()
    }

    func dismissError() {}
    func setModel(_: String) {}
    func openSession(_: SessionRef) { startNewConversation() }
    func handleBackground() { cancel() }
}
