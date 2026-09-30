// AskUserQuestionTests.swift — the interactive questionnaire queue (v3).
//
// Drives synthetic `ClientEvent.askUserQuestion` / `.askUserQuestionResolved`
// through `EngineConversationSource.apply` (the `applyForTesting` seam) and
// asserts the pending-question queue semantics the chat card renders from:
// out-of-turn delivery (broker replay after a foreground re-connect), dedupe
// by request id, drop on resolved, clear on sessionEnded. Hermetic — no
// engine handle is ever built.

import XCTest

@testable import LingxiCode

#if canImport(harness_runtimeFFI)
    import harness_runtimeFFI
#endif

#if canImport(harness_runtimeFFI)

    @MainActor
    final class AskUserQuestionTests: XCTestCase {
        private func makeSource() -> EngineConversationSource {
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                apiKey: "",
                model: "",
                appSandboxRoot: NSTemporaryDirectory(),
                                sessionMode: .code,
visionDelegationEnabled: true)
            return EngineConversationSource(config: config)
        }

        private func request(
            id: UInt64,
            questions: [AskQuestionDto] = [AskQuestionDto(
                question: "应用主要记录什么？",
                header: "数据",
                options: [AskOptionDto(label: "笔记", description: "自由文本", preview: nil)],
                multiSelect: false
            )]
        ) -> AskUserQuestionRequestDto {
            AskUserQuestionRequestDto(requestId: id, questions: questions, timeoutSecs: nil)
        }

        /// The broker replays a still-pending question after a foreground
        /// re-connect — OUTSIDE any turn. The turn gate must let it through:
        /// with no turn in flight (`beginTurnForTesting` never called) the
        /// event still lands in the queue.
        func testQuestionArrivingOutsideATurnIsQueued() {
            let source = makeSource()
            XCTAssertFalse(source.model.streaming, "precondition: no turn is in flight")

            source.applyForTesting(.askUserQuestion(request: request(id: 7)))

            XCTAssertEqual(source.model.pendingQuestions.map(\.requestId), [7])
            XCTAssertEqual(source.model.pendingQuestions.first?.questions.first?.question, "应用主要记录什么？")
            XCTAssertEqual(source.model.pendingQuestions.first?.questions.first?.options.first?.label, "笔记")
        }

        /// A replayed request id must not stack a second card; a distinct id
        /// queues behind the first.
        func testQueueDedupesByRequestIdAndOrdersArrivals() {
            let source = makeSource()

            source.applyForTesting(.askUserQuestion(request: request(id: 1)))
            source.applyForTesting(.askUserQuestion(request: request(id: 1)))
            source.applyForTesting(.askUserQuestion(request: request(id: 2)))

            XCTAssertEqual(source.model.pendingQuestions.map(\.requestId), [1, 2])
        }

        /// `askUserQuestionResolved` — answered here, elsewhere, or
        /// auto-continued — drops exactly the named request. It also arrives
        /// outside a turn and must pass the gate.
        func testResolvedDropsOnlyTheNamedRequest() {
            let source = makeSource()
            source.applyForTesting(.askUserQuestion(request: request(id: 1)))
            source.applyForTesting(.askUserQuestion(request: request(id: 2)))

            source.applyForTesting(.askUserQuestionResolved(requestId: 1))

            XCTAssertEqual(source.model.pendingQuestions.map(\.requestId), [2])

            // Resolving an id that is not pending is a harmless no-op.
            source.applyForTesting(.askUserQuestionResolved(requestId: 99))
            XCTAssertEqual(source.model.pendingQuestions.map(\.requestId), [2])
        }

        func testSessionEndedClearsTheQueue() {
            let source = makeSource()
            source.applyForTesting(.askUserQuestion(request: request(id: 1)))
            source.applyForTesting(.askUserQuestion(request: request(id: 2)))

            source.applyForTesting(.sessionEnded)

            XCTAssertTrue(source.model.pendingQuestions.isEmpty)
        }

        /// The question queue must also survive a live turn: the same events
        /// during a turn behave identically (the allowlist is not a bypass
        /// that only works when idle).
        func testQuestionDuringALiveTurnIsQueuedToo() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 3)

            source.applyForTesting(.askUserQuestion(request: request(id: 5)))

            XCTAssertEqual(source.model.pendingQuestions.map(\.requestId), [5])
        }

        func testLiveWaitingForUserKeepsActiveStopInsteadOfDiscard() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 4, sessionId: "test-session")
            source.applyForTesting(.askUserQuestion(request: request(id: 6)))

            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: 4,
                state: .waitingForUser,
                firstSequence: 0,
                lastSequence: 0,
                safeToResume: false,
                reason: "question"
            )))

            XCTAssertTrue(source.model.streaming)
            XCTAssertFalse(source.model.hasInactiveDurableRecovery)
            XCTAssertTrue(source.model.hasUnresolvedTurnRecovery)
        }

        /// A background task finishing AFTER its turn ended (the normal case)
        /// appends a transcript notice line; pending/running transitions stay
        /// silent.
        func testTaskCompletionOutsideATurnAppendsATranscriptNotice() {
            let source = makeSource()
            XCTAssertFalse(source.model.streaming, "precondition: the turn already ended")

            source.applyForTesting(.taskStatusChanged(
                taskId: "task-abc12", status: .running, originSessionId: nil, error: nil))
            XCTAssertTrue(source.model.items.isEmpty, "a running transition is not a notice")

            source.applyForTesting(.taskStatusChanged(
                taskId: "task-abc12", status: .completed, originSessionId: nil, error: nil))
            source.applyForTesting(.taskStatusChanged(
                taskId: "task-def34", status: .failed, originSessionId: nil, error: nil))

            let notices: [ConversationExecutionNotice] = source.model.items.compactMap {
                if case let .notice(notice) = $0 { return notice }
                return nil
            }
            XCTAssertEqual(notices.count, 2)
            XCTAssertTrue(notices[0].text.contains("task-abc12"))
            XCTAssertEqual(notices[0].kind, .info)
            XCTAssertTrue(notices[1].text.contains("task-def34"))
            XCTAssertEqual(notices[1].kind, .error)
        }

        // MARK: card answer-building (pure logic)

        private func askQuestion(
            _ text: String,
            options: [String] = ["笔记", "打卡"],
            multiSelect: Bool = false
        ) -> ConversationAskQuestion {
            ConversationAskQuestion(
                question: text,
                header: "H",
                options: options.map { ConversationAskOption(label: $0, description: "", preview: nil) },
                multiSelect: multiSelect
            )
        }

        func testSingleSelectToggleReplacesAndMultiSelectAccumulates() {
            XCTAssertEqual(AskUserQuestionCard.toggled("A", in: [], multiSelect: false), ["A"])
            XCTAssertEqual(AskUserQuestionCard.toggled("B", in: ["A"], multiSelect: false), ["B"])
            XCTAssertEqual(AskUserQuestionCard.toggled("A", in: ["A"], multiSelect: false), [])
            XCTAssertEqual(AskUserQuestionCard.toggled("B", in: ["A"], multiSelect: true), ["A", "B"])
            XCTAssertEqual(AskUserQuestionCard.toggled("A", in: ["A", "B"], multiSelect: true), ["B"])
        }

        /// The wire answer map keys on the QUESTION TEXT; labels comma-join;
        /// free text rides as one more value; unanswered questions are
        /// omitted, and completeness requires every question answered.
        func testAnswersMapKeysOnQuestionTextAndJoinsLabels() {
            let questions = [
                askQuestion("记录什么？", multiSelect: true),
                askQuestion("要提醒吗？"),
            ]
            let selected = [0: ["笔记", "打卡"], 1: []]
            let custom = [1: " 每天晚上提醒 "]

            let answers = AskUserQuestionCard.answers(
                questions: questions, selected: selected, custom: custom
            )
            XCTAssertEqual(answers["记录什么？"], "笔记, 打卡")
            XCTAssertEqual(answers["要提醒吗？"], "每天晚上提醒", "free text is trimmed and used as the answer")
            XCTAssertTrue(AskUserQuestionCard.isComplete(questions: questions, selected: selected, custom: custom))

            let incomplete = AskUserQuestionCard.answers(
                questions: questions, selected: [0: ["笔记"]], custom: [:]
            )
            XCTAssertNil(incomplete["要提醒吗？"], "an unanswered question is omitted, not sent empty")
            XCTAssertFalse(AskUserQuestionCard.isComplete(questions: questions, selected: [0: ["笔记"]], custom: [:]))
        }

        func testFreeTextAppendsToMultiSelectLabels() {
            let questions = [askQuestion("要哪些功能？", multiSelect: true)]
            let answers = AskUserQuestionCard.answers(
                questions: questions,
                selected: [0: ["笔记"]],
                custom: [0: "语音输入"]
            )
            XCTAssertEqual(answers["要哪些功能？"], "笔记, 语音输入")
        }
    }

#endif
