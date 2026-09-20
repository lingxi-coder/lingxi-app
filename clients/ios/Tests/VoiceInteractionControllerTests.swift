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
        let first = ControllerVoiceSession(
            transcript: "hello agent",
            automaticallyFinishesWhenEndpointingEnabled: true
        )
        let second = ControllerVoiceSession(transcript: "next turn")
        let player = RecordingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [first, second], player: player)

        controller.startFlow(source: source)
        await settle()

        XCTAssertEqual(source.sentTexts, ["hello agent"])
        XCTAssertEqual(controller.phase, .thinking)
        XCTAssertEqual(first.automaticEndpointAfterSilence, .milliseconds(1_200))
        let token = try! XCTUnwrap(source.lastToken)
        source.complete(token: token, text: "agent reply")
        controller.handleTurnCompletion(try! XCTUnwrap(source.model.turnCompletion))
        await settle()

        XCTAssertEqual(player.requests.map(\.text), ["agent reply"])
        XCTAssertEqual(controller.phase, .listening)
        XCTAssertEqual(second.transcribeCalls, 1)
    }

    func testFlowEnqueuesNaturalSentenceBeforeTurnCompletes() async {
        let capture = ControllerVoiceSession(
            transcript: "stream please",
            automaticallyFinishesWhenEndpointingEnabled: true
        )
        let nextCapture = ControllerVoiceSession(transcript: "next")
        let player = RecordingStreamingSpeechPlayer()
        let barge = ControllerBargeInRecognizer(sessions: [ControllerBargeInSession()])
        let source = ControllerConversationSource()
        let controller = makeController(
            sessions: [capture, nextCapture],
            player: player,
            bargeInRecognizer: barge
        )

        controller.startFlow(source: source)
        await settle()
        let token = try! XCTUnwrap(source.lastToken)
        controller.handleTurnSpeechUpdate(.init(
            token: token,
            sequence: 1,
            delta: "这是流式第一句话。后面"
        ))
        await settle()

        XCTAssertEqual(player.session?.enqueued, ["这是流式第一句话。"])
        XCTAssertTrue(source.model.streaming)
        XCTAssertEqual(controller.phase, .speaking)

        source.complete(token: token, text: "这是流式第一句话。后面完成")
        controller.handleTurnCompletion(try! XCTUnwrap(source.model.turnCompletion))
        await settle()

        XCTAssertEqual(player.session?.enqueued, ["这是流式第一句话。", "后面完成"])
        XCTAssertEqual(player.session?.finishCalls, 1)
    }

    func testConfirmedBargeInCancelsOwnedTurnThenSendsReplacement() async {
        let capture = ControllerVoiceSession(
            transcript: "first question",
            automaticallyFinishesWhenEndpointingEnabled: true
        )
        let firstMonitor = ControllerBargeInSession()
        let secondMonitor = ControllerBargeInSession()
        let barge = ControllerBargeInRecognizer(sessions: [firstMonitor, secondMonitor])
        let source = ControllerConversationSource()
        let controller = makeController(
            sessions: [capture],
            bargeInRecognizer: barge
        )

        controller.startFlow(source: source)
        await settle()
        firstMonitor.emit(.speechStarted)
        await settle()

        XCTAssertEqual(controller.phase, .interrupting)
        XCTAssertEqual(source.cancelCalls, 0)

        firstMonitor.emit(.partial("new"))
        firstMonitor.emit(.transcript("new question"))
        await settle(30)

        XCTAssertEqual(source.cancelCalls, 1)
        XCTAssertEqual(source.sentTexts, ["first question", "new question"])
        XCTAssertEqual(controller.phase, .thinking)
        XCTAssertEqual(barge.startCalls, 2)
    }

    func testFalseBargeInResumesWithoutCancellingTurn() async {
        let capture = ControllerVoiceSession(
            transcript: "keep working",
            automaticallyFinishesWhenEndpointingEnabled: true
        )
        let firstMonitor = ControllerBargeInSession()
        let replacementMonitor = ControllerBargeInSession()
        let barge = ControllerBargeInRecognizer(sessions: [firstMonitor, replacementMonitor])
        let source = ControllerConversationSource()
        let controller = makeController(
            sessions: [capture],
            bargeInRecognizer: barge
        )

        controller.startFlow(source: source)
        await settle()
        firstMonitor.emit(.speechStarted)
        firstMonitor.emit(.empty)
        await settle(24)

        XCTAssertEqual(source.cancelCalls, 0)
        XCTAssertTrue(source.model.streaming)
        XCTAssertEqual(controller.phase, .thinking)
        XCTAssertEqual(barge.startCalls, 2)
    }

    func testFalseBargeInDuringStreamingSpeechResumesAndDrainsBufferedSentence() async {
        let capture = ControllerVoiceSession(
            transcript: "keep speaking",
            automaticallyFinishesWhenEndpointingEnabled: true
        )
        let firstMonitor = ControllerBargeInSession()
        let replacementMonitor = ControllerBargeInSession()
        let barge = ControllerBargeInRecognizer(sessions: [firstMonitor, replacementMonitor])
        let player = RecordingStreamingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(
            sessions: [capture],
            player: player,
            bargeInRecognizer: barge
        )

        controller.startFlow(source: source)
        await settle()
        let token = try! XCTUnwrap(source.lastToken)
        controller.handleTurnSpeechUpdate(.init(
            token: token,
            sequence: 1,
            delta: "这是第一句话。"
        ))
        await settle()
        XCTAssertEqual(player.session?.enqueued, ["这是第一句话。"])

        firstMonitor.emit(.speechStarted)
        await settle()
        XCTAssertEqual(player.session?.pauseCalls, 1)

        controller.handleTurnSpeechUpdate(.init(
            token: token,
            sequence: 2,
            delta: "这是第二句话。"
        ))
        firstMonitor.emit(.empty)
        await settle(24)

        XCTAssertEqual(source.cancelCalls, 0)
        XCTAssertEqual(player.session?.resumeCalls, 1)
        XCTAssertEqual(player.session?.enqueued, ["这是第一句话。", "这是第二句话。"])
        XCTAssertEqual(controller.phase, .speaking)
    }

    func testFalseBargeInFallsBackToTapWhenDuplexRestartFails() async {
        let capture = ControllerVoiceSession(
            transcript: "keep speaking",
            automaticallyFinishesWhenEndpointingEnabled: true
        )
        let monitor = ControllerBargeInSession()
        let barge = FailingRestartBargeInRecognizer(firstSession: monitor)
        let player = RecordingStreamingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(
            sessions: [capture],
            player: player,
            bargeInRecognizer: barge
        )

        controller.startFlow(source: source)
        await settle()
        let token = try! XCTUnwrap(source.lastToken)
        controller.handleTurnSpeechUpdate(.init(
            token: token,
            sequence: 1,
            delta: "这是第一句话。"
        ))
        await settle()

        monitor.emit(.speechStarted)
        monitor.emit(.empty)
        await settle(30)

        XCTAssertEqual(source.cancelCalls, 0)
        XCTAssertEqual(player.session?.resumeCalls, 1)
        XCTAssertEqual(controller.phase, .speaking)
        XCTAssertTrue(controller.statusDetail.contains("轻点光球"))
    }

    /// A barge-in that lands while the speech stream is still opening makes the
    /// post-await guard abandon that stream. The same FlowResponseContext stays
    /// alive when the barge-in resolves `.empty`, so the opening flag must be
    /// released or every later speech attempt is wedged on it.
    func testFalseBargeInReopensSpeechAbandonedWhileTheStreamWasStillOpening() async {
        let capture = ControllerVoiceSession(
            transcript: "keep speaking",
            automaticallyFinishesWhenEndpointingEnabled: true
        )
        let firstMonitor = ControllerBargeInSession()
        let replacementMonitor = ControllerBargeInSession()
        let barge = ControllerBargeInRecognizer(sessions: [firstMonitor, replacementMonitor])
        let player = GatedStreamingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(
            sessions: [capture],
            player: player,
            bargeInRecognizer: barge
        )

        controller.startFlow(source: source)
        await settle()
        let token = try! XCTUnwrap(source.lastToken)
        controller.handleTurnSpeechUpdate(.init(
            token: token,
            sequence: 1,
            delta: "这是第一句话。"
        ))
        await settle()

        XCTAssertEqual(player.openCalls, 1)
        XCTAssertNil(player.session, "openStream must still be suspended")

        firstMonitor.emit(.speechStarted)
        await settle()
        XCTAssertEqual(controller.phase, .interrupting)

        player.releaseOpen()
        await settle(20)
        XCTAssertEqual(player.session?.stopCalls, 1, "the abandoned stream is stopped")

        firstMonitor.emit(.empty)
        await settle(30)

        XCTAssertEqual(source.cancelCalls, 0)
        XCTAssertEqual(player.openCalls, 2, "recovery must be able to reopen the stream")
        XCTAssertEqual(player.session?.enqueued, ["这是第一句话。"])
        XCTAssertEqual(controller.phase, .speaking)
    }

    /// `openStream` throwing `CancellationError` leaves the context live, so the
    /// opening flag has to be released there too — otherwise the next streamed
    /// sentence can never open a stream.
    func testCancelledStreamOpenDoesNotWedgeTheNextSentence() async {
        let capture = ControllerVoiceSession(
            transcript: "keep speaking",
            automaticallyFinishesWhenEndpointingEnabled: true
        )
        let monitor = ControllerBargeInSession()
        let barge = ControllerBargeInRecognizer(sessions: [monitor])
        let player = CancellingFirstOpenStreamingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(
            sessions: [capture],
            player: player,
            bargeInRecognizer: barge
        )

        controller.startFlow(source: source)
        await settle()
        let token = try! XCTUnwrap(source.lastToken)
        controller.handleTurnSpeechUpdate(.init(
            token: token,
            sequence: 1,
            delta: "这是第一句话。"
        ))
        await settle(20)

        XCTAssertEqual(player.openCalls, 1)
        XCTAssertNil(player.session)

        controller.handleTurnSpeechUpdate(.init(
            token: token,
            sequence: 2,
            delta: "这是第二句话。"
        ))
        await settle(30)

        XCTAssertEqual(player.openCalls, 2)
        XCTAssertEqual(player.session?.enqueued, ["这是第一句话。", "这是第二句话。"])
        XCTAssertEqual(controller.phase, .speaking)
    }

    /// A barge-in `pause()` can arrive after `finish()` has already parked its
    /// continuation, and the queue can then drain while paused — `didFinish`
    /// bails out of `completeIfDrained` on `!isPaused`. `resume()` must re-check
    /// the drained state or that continuation is never resumed and the whole
    /// hands-free loop stops advancing.
    func testResumeCompletesAStreamThatDrainedWhilePaused() async {
        let stream = SystemVoiceSpeechStream(
            configuration: VoiceSpeechConfiguration(
                voiceIdentifier: "com.lingxi.tests.absent-voice",
                languageIdentifier: "zh-CN",
                speed: 0.5
            ),
            audioLease: nil
        )
        // Pausing before `finish()` reproduces the exact wedged state the
        // barge-in path reaches (isFinishing, no active utterance, empty queue,
        // paused) without depending on real AVSpeechSynthesizer callbacks.
        stream.pause()

        let box = SpeechOutcomeBox()
        let finishing = Task { @MainActor in
            box.outcome = try? await stream.finish()
        }
        await settle(20)
        XCTAssertNil(box.outcome, "finish() parks while the stream is paused")

        stream.resume()
        await settle(40)

        XCTAssertEqual(box.outcome, VoiceSpeechPlaybackOutcome.completed)

        // Bounded teardown: if the assertion above failed the continuation is
        // still parked, and cancelling resolves it so the suite cannot hang.
        finishing.cancel()
        _ = await finishing.value
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

    func testStreamingSpeechOpenFailureStopsDuplexMonitor() async {
        let capture = ControllerVoiceSession(
            transcript: "speak this",
            automaticallyFinishesWhenEndpointingEnabled: true
        )
        let monitor = ControllerBargeInSession()
        let barge = ControllerBargeInRecognizer(sessions: [monitor])
        let source = ControllerConversationSource()
        let controller = makeController(
            sessions: [capture],
            player: FailingStreamingSpeechPlayer(),
            bargeInRecognizer: barge
        )

        controller.startFlow(source: source)
        await settle()
        let token = try! XCTUnwrap(source.lastToken)
        controller.handleTurnSpeechUpdate(.init(
            token: token,
            sequence: 1,
            delta: "这句话会触发流式播报。"
        ))
        await settle(30)

        XCTAssertEqual(controller.phase, .failed)
        XCTAssertTrue(controller.statusDetail.contains("语音播报失败"))
        XCTAssertEqual(monitor.stopCalls, 1)
        XCTAssertTrue(source.model.streaming)
        XCTAssertEqual(source.cancelCalls, 0)
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

    func testUndeterminedPermissionsAreRequestedBeforeDictationStarts() async {
        let session = ControllerVoiceSession(transcript: "permission granted")
        let readiness = MutableVoiceReadiness(.permissionUndetermined)
        var permissionRequestCount = 0
        let controller = makeController(
            sessions: [session],
            readinessProvider: { readiness.value },
            permissionRequester: {
                permissionRequestCount += 1
                readiness.value = .ready
            }
        )

        controller.startDictation { _ in }
        XCTAssertEqual(controller.phase, .requestingPermission)
        XCTAssertEqual(session.transcribeCalls, 0)

        await settle()

        XCTAssertEqual(permissionRequestCount, 1)
        XCTAssertEqual(controller.phase, .listening)
        XCTAssertEqual(session.transcribeCalls, 1)
        controller.close()
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

    func testBackgroundedFlowTurnStopsAudioButCompletesSilentlyUntilTapResumesListening() async {
        let firstCapture = ControllerVoiceSession(
            transcript: "keep working",
            automaticallyFinishesWhenEndpointingEnabled: true
        )
        let secondCapture = ControllerVoiceSession(transcript: "resume only after tap")
        let monitor = ControllerBargeInSession()
        let barge = ControllerBargeInRecognizer(sessions: [monitor])
        let player = RecordingStreamingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(
            sessions: [firstCapture, secondCapture],
            player: player,
            bargeInRecognizer: barge
        )

        controller.startFlow(source: source)
        await settle()
        let token = try! XCTUnwrap(source.lastToken)
        controller.handleTurnSpeechUpdate(.init(
            token: token,
            sequence: 1,
            delta: "这是前台播报。"
        ))
        await settle()

        XCTAssertEqual(player.openCalls, 1)
        XCTAssertEqual(player.session?.enqueued, ["这是前台播报。"])

        controller.handleBackground()
        await settle()

        controller.handleTurnSpeechUpdate(.init(
            token: token,
            sequence: 2,
            delta: "这是后台补发。"
        ))
        source.complete(token: token, text: "这是前台播报。这是后台补发。")
        controller.handleTurnCompletion(try! XCTUnwrap(source.model.turnCompletion))
        await settle(30)

        XCTAssertEqual(source.cancelCalls, 0)
        XCTAssertEqual(player.openCalls, 1, "backgrounded flow must not reopen playback")
        XCTAssertEqual(player.session?.enqueued, ["这是前台播报。"])
        XCTAssertEqual(player.session?.stopCalls, 1)
        XCTAssertEqual(monitor.stopCalls, 1)
        XCTAssertEqual(controller.phase, .paused)
        XCTAssertEqual(secondCapture.transcribeCalls, 0)

        controller.handleOrbTap()
        await settle(30)

        XCTAssertEqual(controller.phase, .listening)
        XCTAssertEqual(secondCapture.transcribeCalls, 1)
    }

    func testReopeningFlowWaitsForPreviousAudioCleanup() async {
        let firstCapture = ControllerVoiceSession(
            transcript: "first turn",
            automaticallyFinishesWhenEndpointingEnabled: true
        )
        let secondCapture = ControllerVoiceSession(transcript: "second turn")
        let player = BlockingStopStreamingSpeechPlayer()
        let barge = ControllerBargeInRecognizer(sessions: [ControllerBargeInSession()])
        let firstSource = ControllerConversationSource()
        let secondSource = ControllerConversationSource()
        let controller = makeController(
            sessions: [firstCapture, secondCapture],
            player: player,
            bargeInRecognizer: barge
        )

        controller.startFlow(source: firstSource)
        await settle()
        let token = try! XCTUnwrap(firstSource.lastToken)
        controller.handleTurnSpeechUpdate(.init(
            token: token,
            sequence: 1,
            delta: "开始播报第一句话。"
        ))
        await settle()
        XCTAssertNotNil(player.session)

        controller.close()
        await settle()
        controller.startFlow(source: secondSource)
        await settle()

        XCTAssertEqual(secondCapture.transcribeCalls, 0)
        player.session?.releaseStop()
        await settle(30)

        XCTAssertEqual(secondCapture.transcribeCalls, 1)
        XCTAssertEqual(controller.phase, .listening)
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

        // The old turn completed while cancellation was being retried, so the
        // controller can reopen listening without submitting a stale Cancel.
        XCTAssertEqual(source.cancelCalls, 1)
        XCTAssertEqual(controller.phase, .listening)
        XCTAssertEqual(second.transcribeCalls, 1)
    }

    private func makeController(
        sessions: [ControllerVoiceSession],
        player: (any VoiceSpeechPlaying)? = nil,
        bargeInRecognizer: (any VoiceBargeInRecognizing)? = nil,
        readiness: VoiceConfigurationReadiness? = nil,
        readinessProvider: (@MainActor () -> VoiceConfigurationReadiness)? = nil,
        permissionRequester: (@MainActor () async -> Void)? = nil,
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
            bargeInRecognizer: bargeInRecognizer,
            readinessOverride: readinessProvider ?? { ready },
            permissionRequester: permissionRequester,
            loopDelay: .zero
        )
    }

    private func settle(_ count: Int = 12) async {
        for _ in 0..<count { await Task.yield() }
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
    private let automaticallyFinishesWhenEndpointingEnabled: Bool
    private var continuation: CheckedContinuation<String, Error>?
    private var pendingFinish = false
    private var pendingCancellation = false

    private(set) var transcribeCalls = 0
    private(set) var finishCalls = 0
    private(set) var cancelCalls = 0
    private(set) var automaticEndpointAfterSilence: Duration?

    init(
        transcript: String,
        ignoresCancellation: Bool = false,
        automaticallyFinishesWhenEndpointingEnabled: Bool = false
    ) {
        self.transcript = transcript
        self.ignoresCancellation = ignoresCancellation
        self.automaticallyFinishesWhenEndpointingEnabled = automaticallyFinishesWhenEndpointingEnabled
    }

    func transcribe(
        language _: String?,
        automaticEndpointAfterSilence: Duration?
    ) async throws -> String {
        transcribeCalls += 1
        self.automaticEndpointAfterSilence = automaticEndpointAfterSilence
        if automaticEndpointAfterSilence != nil,
           automaticallyFinishesWhenEndpointingEnabled {
            return transcript
        }
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
private final class FailingStreamingSpeechPlayer: VoiceSpeechPlaying {
    private enum PlaybackError: LocalizedError {
        case unavailable
        var errorDescription: String? { "playback unavailable" }
    }

    func speak(_: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        throw PlaybackError.unavailable
    }

    func stop() {}

    func openStream(
        configuration _: VoiceSpeechConfiguration,
        managesAudioSession _: Bool
    ) async throws -> any VoiceSpeechStreamingSession {
        throw PlaybackError.unavailable
    }
}

@MainActor
private final class RecordingStreamingSpeechPlayer: VoiceSpeechPlaying {
    private(set) var session: RecordingStreamingSpeechSession?
    private(set) var openCalls = 0

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        session?.enqueue(request.text)
        return .completed
    }

    func stop() {}

    func openStream(
        configuration _: VoiceSpeechConfiguration,
        managesAudioSession _: Bool
    ) async throws -> any VoiceSpeechStreamingSession {
        openCalls += 1
        let session = RecordingStreamingSpeechSession()
        self.session = session
        return session
    }
}

@MainActor
private final class RecordingStreamingSpeechSession: VoiceSpeechStreamingSession {
    private(set) var enqueued: [String] = []
    private(set) var pauseCalls = 0
    private(set) var resumeCalls = 0
    private(set) var finishCalls = 0
    private(set) var stopCalls = 0

    func enqueue(_ text: String) { enqueued.append(text) }
    func pause() { pauseCalls += 1 }
    func resume() { resumeCalls += 1 }

    func finish() async throws -> VoiceSpeechPlaybackOutcome {
        finishCalls += 1
        return .completed
    }

    func stop() async { stopCalls += 1 }
}

/// Suspends the FIRST `openStream` until `releaseOpen()`, so a test can land a
/// barge-in while the controller is still opening its speech stream.
@MainActor
private final class GatedStreamingSpeechPlayer: VoiceSpeechPlaying {
    private(set) var session: RecordingStreamingSpeechSession?
    private(set) var openCalls = 0
    private var gate: CheckedContinuation<Void, Never>?
    private var gateReleased = false

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        session?.enqueue(request.text)
        return .completed
    }

    func stop() {}

    func openStream(
        configuration _: VoiceSpeechConfiguration,
        managesAudioSession _: Bool
    ) async throws -> any VoiceSpeechStreamingSession {
        openCalls += 1
        if !gateReleased {
            await withCheckedContinuation { continuation in
                if gateReleased {
                    continuation.resume()
                } else {
                    gate = continuation
                }
            }
        }
        let session = RecordingStreamingSpeechSession()
        self.session = session
        return session
    }

    func releaseOpen() {
        gateReleased = true
        gate?.resume()
        gate = nil
    }
}

/// Fails the FIRST `openStream` with `CancellationError` and succeeds after.
@MainActor
private final class CancellingFirstOpenStreamingSpeechPlayer: VoiceSpeechPlaying {
    private(set) var session: RecordingStreamingSpeechSession?
    private(set) var openCalls = 0

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        session?.enqueue(request.text)
        return .completed
    }

    func stop() {}

    func openStream(
        configuration _: VoiceSpeechConfiguration,
        managesAudioSession _: Bool
    ) async throws -> any VoiceSpeechStreamingSession {
        openCalls += 1
        if openCalls == 1 { throw CancellationError() }
        let session = RecordingStreamingSpeechSession()
        self.session = session
        return session
    }
}

@MainActor
private final class SpeechOutcomeBox {
    var outcome: VoiceSpeechPlaybackOutcome?
}

@MainActor
private final class BlockingStopStreamingSpeechPlayer: VoiceSpeechPlaying {
    private(set) var session: BlockingStopStreamingSpeechSession?

    func speak(_: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome { .completed }
    func stop() {}

    func openStream(
        configuration _: VoiceSpeechConfiguration,
        managesAudioSession _: Bool
    ) async throws -> any VoiceSpeechStreamingSession {
        let session = BlockingStopStreamingSpeechSession()
        self.session = session
        return session
    }
}

@MainActor
private final class BlockingStopStreamingSpeechSession: VoiceSpeechStreamingSession {
    private var stopContinuation: CheckedContinuation<Void, Never>?
    private var stopReleased = false

    func enqueue(_: String) {}
    func pause() {}
    func resume() {}
    func finish() async throws -> VoiceSpeechPlaybackOutcome { .completed }

    func stop() async {
        guard !stopReleased else { return }
        await withCheckedContinuation { continuation in
            if stopReleased {
                continuation.resume()
            } else {
                stopContinuation = continuation
            }
        }
    }

    func releaseStop() {
        stopReleased = true
        stopContinuation?.resume()
        stopContinuation = nil
    }
}

@MainActor
private final class ControllerBargeInRecognizer: VoiceBargeInRecognizing {
    private var sessions: [ControllerBargeInSession]
    private(set) var startCalls = 0

    init(sessions: [ControllerBargeInSession]) {
        self.sessions = sessions
    }

    func start(language _: String?, prefersOnDevice _: Bool) async throws -> any VoiceBargeInSession {
        startCalls += 1
        precondition(!sessions.isEmpty, "Test did not provide enough barge-in sessions")
        return sessions.removeFirst()
    }
}

@MainActor
private final class FailingRestartBargeInRecognizer: VoiceBargeInRecognizing {
    private enum RestartError: LocalizedError {
        case unavailable
        var errorDescription: String? { "duplex unavailable" }
    }

    private var firstSession: ControllerBargeInSession?

    init(firstSession: ControllerBargeInSession) {
        self.firstSession = firstSession
    }

    func start(language _: String?, prefersOnDevice _: Bool) async throws -> any VoiceBargeInSession {
        if let firstSession {
            self.firstSession = nil
            return firstSession
        }
        throw RestartError.unavailable
    }
}

@MainActor
private final class ControllerBargeInSession: VoiceBargeInSession {
    let events: AsyncStream<VoiceBargeInEvent>
    private let continuation: AsyncStream<VoiceBargeInEvent>.Continuation
    private(set) var stopCalls = 0

    init() {
        var captured: AsyncStream<VoiceBargeInEvent>.Continuation?
        events = AsyncStream { captured = $0 }
        continuation = captured!
    }

    func emit(_ event: VoiceBargeInEvent) {
        continuation.yield(event)
    }

    func stop() async {
        stopCalls += 1
        continuation.finish()
    }
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

    static let permissionUndetermined = VoiceConfigurationReadiness(
        speechConfigured: true,
        ttsConfigured: true,
        speechReady: false,
        ttsReady: true,
        issues: [
            .init(
                component: .microphone,
                kind: .permissionUndetermined,
                message: "需要麦克风权限"
            ),
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
    let sessionMode: SessionMode = .code
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
    func setPermissionMode(_: String) {}
    func confirmAndSetBypassPermissions(suppressWarning _: Bool) {}
    func setReasoningSelection(_: String) {}
    func setFastMode(_: Bool) {}
    func openSession(_: SessionRef) { startNewConversation() }
    func handleBackground() { cancel() }
}
