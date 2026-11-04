import XCTest
@testable import LingxiCode

@MainActor
final class VoiceInteractionControllerTests: XCTestCase {
    func testStreamingOfflineSpeechUsesModelDefaultWhenReadyRouteHasNoVoiceID() async throws {
        let model = try XCTUnwrap(GeneratedVoiceModelCatalog.all.first(where: { $0.kind == .tts }))
        let route = AudioRouteResolution(
            requested: .init(source: .offline, offlineModelId: model.id, voice: nil),
            effective: .init(source: .offline, modelId: model.id, voiceId: nil),
            status: .ready,
            reason: "ready",
            fallbackReason: nil
        )
        let effective = try XCTUnwrap(route.effective)
        let defaultVoiceID = try XCTUnwrap(model.voices.first?.id)

        XCTAssertNil(effective.voiceId)
        XCTAssertEqual(
            SystemVoiceSpeechPlayer.resolvedOfflineVoiceID(for: effective.voiceId, model: model),
            defaultVoiceID
        )
        XCTAssertEqual(
            SystemVoiceSpeechPlayer.resolvedOfflineVoiceID(for: defaultVoiceID, model: model),
            defaultVoiceID
        )

        let player = SystemVoiceSpeechPlayer(modelRoot: { _ in FileManager.default.temporaryDirectory })
        let stream = try await player.openStream(
            configuration: VoiceSpeechConfiguration(
                voiceIdentifier: "",
                languageIdentifier: "en-US",
                speed: 1,
                route: route,
                maxPayloadBytes: 16_384
            ),
            managesAudioSession: false
        )
        XCTAssertTrue(stream is SherpaVoiceSpeechStream)
        XCTAssertFalse(VoiceModelStore.shared.canRemove(model.id))
        await stream.stop()
        XCTAssertTrue(VoiceModelStore.shared.canRemove(model.id))
    }

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

    func testContextNavigationDoesNotCancelSubmittedFlowTurn() async {
        let capture = ControllerVoiceSession(transcript: "continue work", automaticallyFinishesWhenEndpointingEnabled: true)
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [capture])
        controller.startFlow(source: source)
        await settle()
        XCTAssertTrue(source.model.streaming)
        controller.handleContextChange()
        await settle()
        XCTAssertEqual(source.cancelCalls, 0)
        XCTAssertTrue(source.model.streaming)
    }

    func testHiddenFlowKeepsItsOwnerAndFinishesSpeechBeforeAnotherOwnerCaptures() async throws {
        let ownership = VoiceInteractionOwnership()
        let capture = ControllerVoiceSession(transcript: "continue work", automaticallyFinishesWhenEndpointingEnabled: true)
        let laterCapture = ControllerVoiceSession(transcript: "another owner")
        let player = RecordingStreamingSpeechPlayer()
        let first = ControllerConversationSource()
        let second = ControllerConversationSource()
        let attachmentA = try XCTUnwrap(IOSMediaAttachmentIdentity(hostID: "11111111-1111-4111-8111-111111111111", generation: 1))
        let attachmentB = try XCTUnwrap(IOSMediaAttachmentIdentity(hostID: "22222222-2222-4222-8222-222222222222", generation: 1))
        let ownerA = IOSAudioOwner.attached(attachment: attachmentA, logicalOwner: .session(sessionID: "A"))
        first.voiceAudioOwner = ownerA
        let voiceA = makeController(sessions: [capture], player: player, ownership: ownership)
        let voiceB = makeController(sessions: [laterCapture], player: player, ownership: ownership)
        let host = SessionHost { voiceA }
        _ = try host.retain(source: first, scope: .global, mode: .code,
                            sessionID: "A", workspace: "/a", draft: "")

        voiceA.startFlow(source: first)
        await settle()
        let token = try XCTUnwrap(first.lastToken)
        let pinned = voiceA.capability.configurationSnapshot
        voiceA.handleContextChange()
        first.voiceAudioOwner = .attached(attachment: attachmentB, logicalOwner: .session(sessionID: "B"))
        voiceB.startFlow(source: second)
        await settle()
        XCTAssertEqual(laterCapture.transcribeCalls, 0)
        XCTAssertEqual(voiceB.phase, .failed)
        XCTAssertTrue(first.model.streaming)
        XCTAssertEqual(first.cancelCalls, 0)

        first.model.turnSpeechUpdates.send(.init(token: token, sequence: 1, delta: "Reply from A!"))
        let openDeadline = ContinuousClock.now.advanced(by: .seconds(3))
        while player.configurations.isEmpty && ContinuousClock.now < openDeadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        let speechConfiguration = try XCTUnwrap(player.configurations.first)
        XCTAssertEqual(speechConfiguration.audioOwner, ownerA)
        XCTAssertEqual(speechConfiguration.configurationSnapshot, pinned)
        first.complete(token: token, text: "Reply from A!")
        await settle(50)
        XCTAssertFalse(voiceA.isPresented)
        XCTAssertFalse(voiceA.hasOwnedAudio)

        voiceB.retry()
        await settle(30)
        XCTAssertEqual(laterCapture.transcribeCalls, 1)
        voiceB.close()
        withExtendedLifetime(host) {}
    }

    func testNavigationPreservesRejectedCapturedSpeechForOriginalDraft() async {
        let capture = ControllerVoiceSession(transcript: "do not lose this")
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [capture])
        var originalDraft = ""
        controller.startFlow(source: source, onRejectedTranscript: { originalDraft = $0 })
        await settle()
        controller.handleContextChange()
        source.model.streaming = true
        controller.finishListening()
        await settle(30)
        XCTAssertEqual(originalDraft, "do not lose this")
        XCTAssertTrue(source.sentTexts.isEmpty)
        XCTAssertFalse(controller.isPresented)
    }

    func testFlowPinsAudioConfigurationThroughReplyAndUsesLatestRevisionOnNextListen() async {
        let first = ControllerVoiceSession(
            transcript: "pinned turn",
            automaticallyFinishesWhenEndpointingEnabled: true
        )
        let second = ControllerVoiceSession(transcript: "latest turn")
        let player = RecordingStreamingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(
            sessions: [first, second],
            player: player,
            bargeInRecognizer: nil
        )
        let initialSnapshot = controller.capability.configurationSnapshot

        controller.startFlow(source: source)
        await settle()
        XCTAssertEqual(first.configurationSnapshots.first, initialSnapshot)

        controller.capability.setSpeed(1.25)
        let changedSnapshot = controller.capability.configurationSnapshot
        XCTAssertGreaterThan(changedSnapshot.revision, initialSnapshot.revision)

        let token = try! XCTUnwrap(source.lastToken)
        controller.handleTurnSpeechUpdate(.init(
            token: token,
            sequence: 1,
            delta: "Pinned audio response!"
        ))
        await settle(30)

        let pinnedSpeechConfiguration = try! XCTUnwrap(player.configurations.first)
        XCTAssertEqual(pinnedSpeechConfiguration.route?.requested.source, initialSnapshot.configuration.speech.source)
        XCTAssertEqual(pinnedSpeechConfiguration.configurationRevision, initialSnapshot.revision)

        source.complete(token: token, text: "Pinned audio response!")
        controller.handleTurnCompletion(try! XCTUnwrap(source.model.turnCompletion))
        await settle()

        XCTAssertEqual(second.configurationSnapshots.first, changedSnapshot)
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

    func testNavigationDuringDictationPermissionDeliversToOriginalDraft() async throws {
        let capture = ControllerVoiceSession(transcript: "original draft")
        let readiness = MutableVoiceReadiness(.permissionUndetermined)
        let gate = VoiceStartupGate()
        let controller = makeController(sessions: [capture], readinessProvider: { readiness.value },
                                        permissionRequester: { await gate.wait() })
        let attachment = try XCTUnwrap(IOSMediaAttachmentIdentity(hostID: UUID().uuidString.lowercased(), generation: 1))
        let owner = IOSAudioOwner.attached(attachment: attachment, logicalOwner: .session(sessionID: "original"))
        let originalSnapshot = controller.capability.configurationSnapshot
        var originalDraft = ""
        controller.startDictation(audioOwner: owner) { originalDraft = $0 }
        await settle()
        XCTAssertTrue(gate.started)
        XCTAssertTrue(controller.hasOwnedAudio)
        controller.handleContextChange()
        controller.capability.setSpeed(1.5)
        readiness.value = .ready
        gate.release()
        await settle(30)
        XCTAssertEqual(capture.transcribeCalls, 1)
        XCTAssertEqual(capture.audioOwners, [owner])
        XCTAssertEqual(capture.configurationSnapshots.first, originalSnapshot)
        controller.finishListening()
        await settle(30)
        XCTAssertEqual(originalDraft, "original draft")
        XCTAssertFalse(controller.isPresented)
    }

    func testNavigationDuringFlowPermissionSubmitsToOriginalOwner() async throws {
        let capture = ControllerVoiceSession(transcript: "original owner")
        let source = ControllerConversationSource()
        let attachment = try XCTUnwrap(IOSMediaAttachmentIdentity(hostID: UUID().uuidString.lowercased(), generation: 1))
        let owner = IOSAudioOwner.attached(attachment: attachment, logicalOwner: .session(sessionID: "original"))
        source.voiceAudioOwner = owner
        let readiness = MutableVoiceReadiness(.permissionUndetermined)
        let gate = VoiceStartupGate()
        let controller = makeController(sessions: [capture], readinessProvider: { readiness.value },
                                        permissionRequester: { await gate.wait() })
        controller.startFlow(source: source)
        let originalSnapshot = controller.capability.configurationSnapshot
        await settle()
        XCTAssertTrue(gate.started)
        XCTAssertTrue(controller.hasOwnedAudio)
        controller.handleContextChange()
        source.voiceAudioOwner = nil
        controller.capability.setSpeed(1.5)
        readiness.value = .ready
        gate.release()
        await settle(30)
        XCTAssertEqual(capture.transcribeCalls, 1)
        XCTAssertEqual(capture.audioOwners, [owner])
        XCTAssertEqual(capture.configurationSnapshots.first, originalSnapshot)
        controller.finishListening()
        await settle(30)
        XCTAssertEqual(source.sentTexts, ["original owner"])
        let token = try XCTUnwrap(source.lastToken)
        source.complete(token: token, text: "finished")
        controller.handleTurnCompletion(try XCTUnwrap(source.model.turnCompletion))
        await settle(30)
        XCTAssertFalse(controller.isPresented)
    }

    func testNavigationDuringFlowAdmissionRetainsTheOriginalOwner() async throws {
        let capture = ControllerVoiceSession(transcript: "admitted text", automaticallyFinishesWhenEndpointingEnabled: true)
        let source = ControllerConversationSource()
        let gate = VoiceStartupGate()
        source.sendGate = gate
        let controller = makeController(sessions: [capture])
        controller.startFlow(source: source)
        await settle(30)
        XCTAssertTrue(gate.started)
        controller.handleContextChange()
        XCTAssertTrue(controller.hasOwnedAudio)
        XCTAssertTrue(controller.isPresented)
        gate.release()
        await settle(30)
        XCTAssertEqual(source.sentTexts, ["admitted text"])
        let token = try XCTUnwrap(source.lastToken)
        source.complete(token: token, text: "finished")
        controller.handleTurnCompletion(try XCTUnwrap(source.model.turnCompletion))
        await settle(30)
        XCTAssertFalse(controller.isPresented)
    }

    func testExplicitStopDuringFlowAdmissionDoesNotSendLater() async {
        let capture = ControllerVoiceSession(transcript: "cancelled text", automaticallyFinishesWhenEndpointingEnabled: true)
        let source = ControllerConversationSource()
        let gate = VoiceStartupGate()
        source.sendGate = gate
        let controller = makeController(sessions: [capture])
        controller.startFlow(source: source)
        await settle(30)
        XCTAssertTrue(gate.started)
        controller.close()
        gate.release()
        await settle(30)
        XCTAssertTrue(source.sentTexts.isEmpty)
        XCTAssertFalse(controller.isPresented)
    }

    func testExplicitStopDuringPermissionCannotStartCaptureLater() async {
        let capture = ControllerVoiceSession(transcript: "must not be delivered")
        let readiness = MutableVoiceReadiness(.permissionUndetermined)
        let gate = VoiceStartupGate()
        let controller = makeController(sessions: [capture], readinessProvider: { readiness.value },
                                        permissionRequester: { await gate.wait() })
        var delivered = false
        controller.startDictation { _ in delivered = true }
        await settle()
        XCTAssertTrue(gate.started)
        controller.close()
        readiness.value = .ready
        gate.release()
        await settle(30)
        XCTAssertEqual(capture.transcribeCalls, 0)
        XCTAssertFalse(delivered)
        XCTAssertFalse(controller.isPresented)
        XCTAssertFalse(controller.hasOwnedAudio)
    }

    func testNavigationThenPermissionFailureReleasesTheHiddenOwner() async {
        let readiness = MutableVoiceReadiness(.permissionUndetermined)
        let gate = VoiceStartupGate()
        let controller = makeController(sessions: [], readinessProvider: { readiness.value },
                                        permissionRequester: { await gate.wait() })
        controller.startDictation { _ in XCTFail("Denied permission cannot produce a transcript") }
        await settle()
        XCTAssertTrue(gate.started)
        controller.handleContextChange()
        readiness.value = .notReady
        gate.release()
        await settle(30)
        XCTAssertFalse(controller.isPresented)
        XCTAssertFalse(controller.hasOwnedAudio)
    }

    func testNavigationClosesUnconfiguredIdleVoicePanel() async {
        let controller = makeController(sessions: [], readiness: .notReady)
        controller.startDictation { _ in XCTFail("No capture was admitted") }
        XCTAssertEqual(controller.phase, .configurationRequired)
        controller.handleContextChange()
        await settle(30)
        XCTAssertFalse(controller.isPresented)
        XCTAssertFalse(controller.hasOwnedAudio)
    }

    func testNavigationRetainsRealtimeStartupUntilItsOwnerCanDrain() async {
        let realtime = ControlledRealtimeSession()
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [], realtimeSession: realtime)
        controller.startFlow(source: source)
        controller.handleContextChange() // before the scheduled start task runs
        XCTAssertTrue(controller.isPresented)
        XCTAssertTrue(controller.hasOwnedAudio)
        await settle()
        XCTAssertTrue(realtime.startGate.started)
        XCTAssertEqual(realtime.startCalls, 1)
        XCTAssertEqual(realtime.finishCalls, 0)
        realtime.startGate.release()
        await settle(30)
        XCTAssertEqual(realtime.finishCalls, 1)
        XCTAssertFalse(controller.isPresented)
        XCTAssertFalse(controller.hasOwnedAudio)
    }

    func testExplicitStopDuringRealtimeStartupDoesNotResumeIt() async {
        let realtime = ControlledRealtimeSession()
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [], realtimeSession: realtime)
        controller.startFlow(source: source)
        await settle()
        XCTAssertTrue(realtime.startGate.started)
        controller.close()
        realtime.startGate.release()
        await settle(30)
        XCTAssertEqual(realtime.finishCalls, 0)
        XCTAssertEqual(realtime.stopCalls, 1)
        XCTAssertFalse(controller.isPresented)
        XCTAssertFalse(controller.hasOwnedAudio)
    }

    func testReturningDuringRealtimeStartupKeepsTheReattachedOwner() async {
        let realtime = ControlledRealtimeSession()
        let controller = makeController(sessions: [], realtimeSession: realtime)
        controller.startFlow(source: ControllerConversationSource())
        controller.handleContextChange()
        await settle()
        XCTAssertTrue(realtime.startGate.started)
        controller.handleContextRestored()
        realtime.startGate.release()
        await settle(30)
        XCTAssertEqual(realtime.finishCalls, 0)
        XCTAssertTrue(controller.isPresented)
        XCTAssertEqual(controller.phase, .listening)
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

    func testBackgroundDuringBargeInTeardownDoesNotRestartCapture() async {
        let first = ControllerVoiceSession(transcript: "question", automaticallyFinishesWhenEndpointingEnabled: true)
        let second = ControllerVoiceSession(transcript: "must stay paused")
        let monitor = ControllerBargeInSession()
        monitor.holdStop = true
        let player = RecordingStreamingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [first, second], player: player,
                                        bargeInRecognizer: ControllerBargeInRecognizer(sessions: [monitor]))
        controller.startFlow(source: source)
        await settle(30)
        let token = try! XCTUnwrap(source.lastToken)
        controller.handleTurnSpeechUpdate(.init(token: token, sequence: 1, delta: "这是已经完成的回复。"))
        await settle(30)
        source.complete(token: token, text: "这是已经完成的回复。")
        controller.handleTurnCompletion(try! XCTUnwrap(source.model.turnCompletion))
        await settle(30)
        XCTAssertGreaterThan(monitor.stopCalls, 0, "completion must be waiting for monitor teardown")
        controller.handleBackground()
        monitor.releaseStop()
        await settle(50)
        XCTAssertEqual(controller.phase, .paused)
        XCTAssertEqual(second.transcribeCalls, 0)
        controller.close()
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

    func testNavigationDuringOwnedFlowCleanupKeepsTheOriginalStart() async throws {
        let firstCapture = ControllerVoiceSession(transcript: "first", automaticallyFinishesWhenEndpointingEnabled: true)
        let nextCapture = ControllerVoiceSession(transcript: "after cleanup")
        let player = BlockingStopStreamingSpeechPlayer()
        let barge = ControllerBargeInRecognizer(sessions: [ControllerBargeInSession(), ControllerBargeInSession()])
        let first = ControllerConversationSource()
        let original = ControllerConversationSource()
        let controller = makeController(sessions: [firstCapture, nextCapture], player: player, bargeInRecognizer: barge)
        controller.startFlow(source: first)
        await settle(30)
        let firstToken = try XCTUnwrap(first.lastToken)
        controller.handleTurnSpeechUpdate(.init(token: firstToken, sequence: 1, delta: "开始播报第一句话。"))
        await settle(30)
        let stream = try XCTUnwrap(player.session)
        controller.close()
        await settle()
        controller.startFlow(source: original)
        await settle()
        XCTAssertTrue(controller.hasOwnedAudio)
        XCTAssertEqual(nextCapture.transcribeCalls, 0)
        controller.handleContextChange()
        stream.releaseStop()
        await settle(30)
        XCTAssertEqual(nextCapture.transcribeCalls, 1)
        controller.finishListening()
        await settle(30)
        XCTAssertEqual(original.sentTexts, ["after cleanup"])
        XCTAssertEqual(barge.startCalls, 2)
        controller.close()
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
        let tokenValue = await source.send("ordinary question")
        let token = try! XCTUnwrap(tokenValue)
        controller.registerAutomaticPlaybackCandidate(token)

        source.complete(token: token, text: "ordinary reply")
        controller.handleTurnCompletion(try! XCTUnwrap(source.model.turnCompletion))
        await settle()

        XCTAssertEqual(player.requests.map(\.text), ["ordinary reply"])
        XCTAssertNil(controller.mode)
        XCTAssertEqual(controller.phase, .paused)
    }

    func testNavigationKeepsHiddenOrdinaryTurnEligibleForAutomaticSpeech() async throws {
        let player = RecordingSpeechPlayer()
        let source = ControllerConversationSource()
        let voice = makeController(sessions: [], player: player, autoPlay: true)
        let host = SessionHost { voice }
        _ = try host.retain(source: source, scope: .global, mode: .code,
                            sessionID: "A", workspace: "/a", draft: "")
        let tokenValue = await source.send("ordinary question")
        let token = try XCTUnwrap(tokenValue)
        voice.registerAutomaticPlaybackCandidate(token)
        voice.handleContextChange()
        source.complete(token: token, text: "reply from A")
        await settle(50)
        XCTAssertEqual(player.requests.map(\.text), ["reply from A"])
        XCTAssertEqual(source.cancelCalls, 0)
    }

    func testAutomaticReplyKeepsTheAudioOwnerAndConfigurationCapturedAtAdmission() async throws {
        let player = RecordingSpeechPlayer()
        let source = ControllerConversationSource()
        let voice = makeController(sessions: [], player: player, autoPlay: true)
        let firstAttachment = try XCTUnwrap(IOSMediaAttachmentIdentity(hostID: "11111111-1111-4111-8111-111111111111", generation: 1))
        let laterAttachment = try XCTUnwrap(IOSMediaAttachmentIdentity(hostID: "22222222-2222-4222-8222-222222222222", generation: 1))
        let firstOwner = IOSAudioOwner.attached(attachment: firstAttachment, logicalOwner: .session(sessionID: "A"))
        source.voiceAudioOwner = firstOwner
        let tokenValue = await source.send("A")
        let token = try XCTUnwrap(tokenValue)
        let snapshot = voice.capability.configurationSnapshot
        voice.registerAutomaticPlaybackCandidate(token, audioOwner: source.voiceAudioOwner)
        voice.handleContextChange()
        source.voiceAudioOwner = .attached(attachment: laterAttachment, logicalOwner: .session(sessionID: "B"))
        source.complete(token: token, text: "owner-bound reply")
        voice.handleTurnCompletion(try XCTUnwrap(source.model.turnCompletion))
        await settle()
        XCTAssertEqual(player.requests.first?.audioOwner, firstOwner)
        XCTAssertEqual(player.requests.first?.configurationSnapshot, snapshot)
    }

    func testExplicitCaptureWaitsForHiddenAutomaticPlaybackToStop() async throws {
        let ownership = VoiceInteractionOwnership()
        let player = GatedAutomaticSpeechPlayer()
        let capture = ControllerVoiceSession(transcript: "B")
        let source = ControllerConversationSource()
        let voiceA = makeController(sessions: [], player: player, autoPlay: true, ownership: ownership)
        let voiceB = makeController(sessions: [capture], player: player, ownership: ownership)
        let tokenValue = await source.send("A")
        let token = try XCTUnwrap(tokenValue)
        voiceA.registerAutomaticPlaybackCandidate(token)
        source.complete(token: token, text: "reply from A")
        voiceA.handleTurnCompletion(try XCTUnwrap(source.model.turnCompletion))
        await settle()
        XCTAssertEqual(player.requests.map(\.text), ["reply from A"])
        voiceA.handleContextChange()
        XCTAssertEqual(player.stopCalls, 0)
        XCTAssertFalse(player.stopStarted)
        voiceB.startDictation { _ in }
        await settle()
        XCTAssertTrue(player.stopStarted)
        XCTAssertEqual(capture.transcribeCalls, 0)
        XCTAssertEqual(source.cancelCalls, 0)
        player.finishStop()
        await settle(50)
        XCTAssertEqual(capture.transcribeCalls, 1)
        voiceB.cancelDictation()
    }

    func testAutomaticReplyWaitsUntilAnotherOwnersCaptureFinishes() async throws {
        let ownership = VoiceInteractionOwnership()
        let playerA = RecordingSpeechPlayer()
        let capture = ControllerVoiceSession(transcript: "B")
        let source = ControllerConversationSource()
        let voiceA = makeController(sessions: [], player: playerA, autoPlay: true, ownership: ownership)
        let voiceB = makeController(sessions: [capture], ownership: ownership)
        let tokenValue = await source.send("A")
        let token = try XCTUnwrap(tokenValue)
        voiceA.registerAutomaticPlaybackCandidate(token)
        voiceA.handleContextChange()
        voiceB.startDictation { _ in }
        await settle()
        source.complete(token: token, text: "queued reply")
        voiceA.handleTurnCompletion(try XCTUnwrap(source.model.turnCompletion))
        await settle()
        XCTAssertTrue(playerA.requests.isEmpty)
        voiceB.cancelDictation()
        await settle(50)
        XCTAssertEqual(playerA.requests.map(\.text), ["queued reply"])
        XCTAssertEqual(source.cancelCalls, 0)
    }

    func testBlockedAutomaticOwnerDoesNotPreventAnotherQueuedReply() async throws {
        let ownership = VoiceInteractionOwnership()
        let playerA = RecordingSpeechPlayer(), playerC = RecordingSpeechPlayer()
        let captureB = ControllerVoiceSession(transcript: "B")
        let sourceA = ControllerConversationSource(), sourceC = ControllerConversationSource()
        let voiceA = makeController(sessions: [], player: playerA, autoPlay: true, ownership: ownership)
        let voiceB = makeController(sessions: [captureB], ownership: ownership)
        let voiceC = makeController(sessions: [], player: playerC, autoPlay: true, ownership: ownership)
        let tokenAValue = await sourceA.send("A")
        let tokenA = try XCTUnwrap(tokenAValue)
        let tokenCValue = await sourceC.send("C")
        let tokenC = try XCTUnwrap(tokenCValue)
        voiceA.registerAutomaticPlaybackCandidate(tokenA)
        voiceC.registerAutomaticPlaybackCandidate(tokenC)
        voiceB.startDictation { _ in }
        await settle()
        voiceA.startFlow(source: sourceA)
        XCTAssertEqual(voiceA.phase, .failed)
        sourceA.complete(token: tokenA, text: "reply A")
        voiceA.handleTurnCompletion(try XCTUnwrap(sourceA.model.turnCompletion))
        sourceC.complete(token: tokenC, text: "reply C")
        voiceC.handleTurnCompletion(try XCTUnwrap(sourceC.model.turnCompletion))
        voiceB.cancelDictation()
        await settle(50)
        XCTAssertTrue(playerA.requests.isEmpty)
        XCTAssertEqual(playerC.requests.map(\.text), ["reply C"])
        voiceA.close()
        await settle(50)
        XCTAssertEqual(playerA.requests.map(\.text), ["reply A"])
    }

    func testSameOwnerExplicitCaptureWaitsForItsAutomaticPlaybackCleanup() async throws {
        let player = GatedAutomaticSpeechPlayer()
        let capture = ControllerVoiceSession(transcript: "next")
        let source = ControllerConversationSource()
        let voice = makeController(sessions: [capture], player: player, autoPlay: true)
        let tokenValue = await source.send("A")
        let token = try XCTUnwrap(tokenValue)
        voice.registerAutomaticPlaybackCandidate(token)
        source.complete(token: token, text: "automatic reply")
        voice.handleTurnCompletion(try XCTUnwrap(source.model.turnCompletion))
        await settle()
        voice.startDictation { _ in }
        await settle()
        XCTAssertTrue(player.stopStarted)
        XCTAssertEqual(capture.transcribeCalls, 0)
        player.finishStop()
        await settle(50)
        XCTAssertEqual(capture.transcribeCalls, 1)
        voice.cancelDictation()
    }

    func testFailedAutomaticPlaybackStopKeepsTheLaneUntilTheOldOperationFinishes() async throws {
        let ownership = VoiceInteractionOwnership()
        let player = GatedAutomaticSpeechPlayer(stopSucceeds: false)
        let capture = ControllerVoiceSession(transcript: "B")
        let source = ControllerConversationSource()
        let voiceA = makeController(sessions: [], player: player, autoPlay: true, ownership: ownership)
        let voiceB = makeController(sessions: [capture], player: player, ownership: ownership)
        let tokenValue = await source.send("A")
        let token = try XCTUnwrap(tokenValue)
        voiceA.registerAutomaticPlaybackCandidate(token)
        source.complete(token: token, text: "reply from A")
        voiceA.handleTurnCompletion(try XCTUnwrap(source.model.turnCompletion))
        await settle()
        voiceB.startDictation { _ in }
        await settle()
        XCTAssertTrue(player.stopStarted)
        player.finishStop()
        await settle(30)
        XCTAssertEqual(voiceB.phase, .failed)
        XCTAssertEqual(capture.transcribeCalls, 0)
        XCTAssertTrue(voiceA.hasOwnedAudio)
        XCTAssertEqual(source.cancelCalls, 0)
        player.finishPlayback()
        await settle(30)
        XCTAssertTrue(voiceA.hasOwnedAudio, "a finished caller is not a native stop receipt")
        player.allowStop()
        voiceB.retry()
        await settle()
        player.finishStop()
        await settle(30)
        XCTAssertEqual(capture.transcribeCalls, 1)
        XCTAssertFalse(voiceA.hasOwnedAudio)
        voiceB.cancelDictation()
    }

    func testOrdinaryTurnDoesNotAutoPlayWhenPreferenceIsDisabled() async {
        let player = RecordingSpeechPlayer()
        let source = ControllerConversationSource()
        let controller = makeController(sessions: [], player: player, autoPlay: false)
        let tokenValue = await source.send("ordinary question")
        let token = try! XCTUnwrap(tokenValue)
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
        realtimeSession: (any VoiceRealtimeControlling)? = nil,
        autoPlay: Bool = false,
        ownership: VoiceInteractionOwnership? = nil
    ) -> VoiceInteractionController {
        let factory = ControllerVoiceSessionFactory(sessions: sessions)
        let capture = VoiceCapture(makeSession: { factory.next() })
        let suiteName = "VoiceInteractionControllerTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        let store = AudioConfigurationStore(defaults: defaults)
        if realtimeSession != nil {
            var configuration = store.configuration
            configuration.conversation.mode = "realtime"
            _ = try! store.save(configuration, expectedRevision: store.revision)
        }
        let capability = VoiceCapabilityModel(defaults: defaults, store: store)
        capability.setAutoPlay(autoPlay)
        let ready = readiness ?? VoiceConfigurationReadiness(
            speechConfigured: true,
            ttsConfigured: true,
            speechReady: true,
            ttsReady: true,
            issues: []
        )
        let selectedPlayer: any VoiceSpeechPlaying = player ?? RecordingSpeechPlayer()
        let selectedOwnership = ownership ?? VoiceInteractionOwnership()
        let selectedReadiness: @MainActor () -> VoiceConfigurationReadiness = readinessProvider ?? { ready }
        let selectedRealtimeReadiness: (@MainActor () -> Bool)?
        if realtimeSession != nil {
            selectedRealtimeReadiness = { true }
        } else {
            selectedRealtimeReadiness = nil
        }
        return VoiceInteractionController(
            voiceCapture: capture,
            capability: capability,
            speechPlayer: selectedPlayer,
            bargeInRecognizer: bargeInRecognizer,
            ownership: selectedOwnership,
            readinessOverride: selectedReadiness,
            permissionRequester: permissionRequester,
            realtimeSession: realtimeSession,
            realtimeReadinessOverride: selectedRealtimeReadiness,
            loopDelay: .zero
        )
    }

    private func settle(_ count: Int = 12) async {
        for _ in 0..<count { await Task.yield() }
    }
}

@MainActor
private final class VoiceStartupGate {
    private(set) var started = false
    private var released = false
    private var continuation: CheckedContinuation<Void, Never>?
    func wait() async {
        started = true
        guard !released else { return }
        await withCheckedContinuation { continuation = $0 }
    }
    func release() { released = true; continuation?.resume(); continuation = nil }
}

@MainActor
private final class ControlledRealtimeSession: VoiceRealtimeControlling {
    let startGate = VoiceStartupGate()
    var isActive = false
    var isAttached = true
    var onStateChange: ((IOSRealtimeAudioPhase, String, String?) -> Void)?
    private(set) var startCalls = 0
    private(set) var stopCalls = 0
    private(set) var finishCalls = 0
    func reserveStart(owner: IOSAudioOwner?) -> Bool {
        guard !isActive else { return false }
        isActive = true
        return true
    }
    func restoreReservedStart(owner: IOSAudioOwner?) {}
    func start(snapshot: AudioConfigurationSnapshot) async throws {
        startCalls += 1
        isActive = true
        onStateChange?(.connecting, "", nil)
        await startGate.wait()
        try Task.checkCancellation()
        onStateChange?(.listening, "", nil)
    }
    func stop(abort: Bool) async -> Bool { stopCalls += 1; isActive = false; return true }
    func commitInput() {}
    func interrupt() async {}
    func finishCurrentTurnAndClose() {
        finishCalls += 1
        isActive = false
        onStateChange?(.idle, "", nil)
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
    private(set) var configurationSnapshots: [AudioConfigurationSnapshot] = []
    private(set) var audioOwners: [IOSAudioOwner?] = []

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

    func transcribe(
        language: String?,
        automaticEndpointAfterSilence: Duration?,
        configurationSnapshot: AudioConfigurationSnapshot,
        audioOwner: IOSAudioOwner?
    ) async throws -> String {
        configurationSnapshots.append(configurationSnapshot)
        audioOwners.append(audioOwner)
        return try await transcribe(
            language: language,
            automaticEndpointAfterSilence: automaticEndpointAfterSilence
        )
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

    func openStream(configuration: VoiceSpeechConfiguration, managesAudioSession: Bool) async throws -> any VoiceSpeechStreamingSession {
        let stream = RecordingStreamingSpeechSession()
        stream.onEnqueue = { [weak self] text in self?.requests.append(VoiceSpeechRequest(text: text, voiceIdentifier: configuration.voiceIdentifier, languageIdentifier: configuration.languageIdentifier, speed: configuration.speed, route: configuration.route, maxPayloadBytes: configuration.maxPayloadBytes, configurationRevision: configuration.configurationRevision)) }
        stream.onStop = { [weak self] in self?.stop() }
        return stream
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

    func openStream(configuration: VoiceSpeechConfiguration, managesAudioSession: Bool) async throws -> any VoiceSpeechStreamingSession {
        let stream = RecordingStreamingSpeechSession()
        stream.onEnqueue = { [weak self] text in self?.requests.append(VoiceSpeechRequest(text: text, voiceIdentifier: configuration.voiceIdentifier, languageIdentifier: configuration.languageIdentifier, speed: configuration.speed, route: configuration.route, maxPayloadBytes: configuration.maxPayloadBytes, configurationRevision: configuration.configurationRevision)) }
        stream.onFinish = { [weak self] in
            guard let self else { return .interrupted }
            await withCheckedContinuation { self.continuation = $0 }
            try Task.checkCancellation()
            return .completed
        }
        stream.onStop = { [weak self] in self?.stop() }
        return stream
    }

    func stop() {
        stopCalls += 1
        continuation?.resume()
        continuation = nil
    }
}

@MainActor
private final class GatedAutomaticSpeechPlayer: VoiceSpeechPlaying {
    private(set) var requests: [VoiceSpeechRequest] = []
    private(set) var stopCalls = 0
    private(set) var stopStarted = false
    private var playback: CheckedContinuation<VoiceSpeechPlaybackOutcome, Never>?
    private var stopGate: CheckedContinuation<Void, Never>?
    private var stopSucceeds: Bool

    init(stopSucceeds: Bool = true) { self.stopSucceeds = stopSucceeds }

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        requests.append(request)
        return await withCheckedContinuation { playback = $0 }
    }
    func openStream(configuration: VoiceSpeechConfiguration, managesAudioSession: Bool) async throws -> any VoiceSpeechStreamingSession {
        RecordingStreamingSpeechSession()
    }
    func stop() { stopCalls += 1 }
    func stopAutomaticPlaybackAndWait() async -> Bool {
        stopStarted = true
        await withCheckedContinuation { stopGate = $0 }
        guard stopSucceeds else { return false }
        finishPlayback()
        return true
    }
    func finishPlayback() {
        playback?.resume(returning: .interrupted)
        playback = nil
    }
    func allowStop() { stopSucceeds = true }
    func finishStop() { stopGate?.resume(); stopGate = nil }
}

@MainActor
private final class InterruptingSpeechPlayer: VoiceSpeechPlaying {
    private(set) var requests: [VoiceSpeechRequest] = []

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        requests.append(request)
        return .interrupted
    }

    func openStream(configuration: VoiceSpeechConfiguration, managesAudioSession: Bool) async throws -> any VoiceSpeechStreamingSession {
        let stream = RecordingStreamingSpeechSession()
        stream.onEnqueue = { [weak self] text in self?.requests.append(VoiceSpeechRequest(text: text, voiceIdentifier: configuration.voiceIdentifier, languageIdentifier: configuration.languageIdentifier, speed: configuration.speed, route: configuration.route, maxPayloadBytes: configuration.maxPayloadBytes, configurationRevision: configuration.configurationRevision)) }
        stream.outcome = .interrupted
        return stream
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
    private(set) var configurations: [VoiceSpeechConfiguration] = []

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        session?.enqueue(request.text)
        return .completed
    }

    func stop() {}

    func openStream(
        configuration: VoiceSpeechConfiguration,
        managesAudioSession _: Bool
    ) async throws -> any VoiceSpeechStreamingSession {
        openCalls += 1
        configurations.append(configuration)
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

    var onEnqueue: ((String) -> Void)?
    var onFinish: (() async throws -> VoiceSpeechPlaybackOutcome)?
    var onStop: (() -> Void)?
    var outcome: VoiceSpeechPlaybackOutcome = .completed

    func enqueue(_ text: String) { enqueued.append(text); onEnqueue?(text) }
    func pause() { pauseCalls += 1 }
    func resume() { resumeCalls += 1 }

    func finish() async throws -> VoiceSpeechPlaybackOutcome {
        finishCalls += 1
        if let onFinish { return try await onFinish() }
        return outcome
    }

    func stop() async { stopCalls += 1; onStop?() }
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
    var holdStop = false
    private var stopWaiters: [CheckedContinuation<Void, Never>] = []

    func releaseStop() {
        holdStop = false
        let waiters = stopWaiters
        stopWaiters.removeAll()
        for waiter in waiters { waiter.resume() }
    }

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
        if holdStop { await withCheckedContinuation { stopWaiters.append($0) } }
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
    var voiceAudioOwner: IOSAudioOwner?
    private(set) var sentTexts: [String] = []
    private(set) var lastToken: ConversationTurnToken?
    private(set) var cancelCalls = 0
    var sendGate: VoiceStartupGate?
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
    func send(_ text: String) async -> ConversationTurnToken? {
        if let sendGate { await sendGate.wait() }
        guard !Task.isCancelled else { return nil }
        guard !model.streaming else { return nil }
        let token = ConversationTurnToken(clientTurnId: nextTurnID, sessionEpoch: 1)
        nextTurnID += 1
        sentTexts.append(text)
        lastToken = token
        model.streaming = true
        model.turnCompletion = nil
        return token
    }

    @discardableResult
    func send(
        _ text: String,
        sourceCodeUnits: [UInt16]?,
        images: [ImageRefDto],
        selectedSlashCommand: ConversationSlashCommand?
    ) async -> ConversationTurnToken? {
        await send(text)
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
