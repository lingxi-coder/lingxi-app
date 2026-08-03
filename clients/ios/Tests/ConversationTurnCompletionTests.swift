import XCTest

@testable import LingxiCode

#if canImport(engine_mobileFFI)
    import engine_mobileFFI
#endif

@MainActor
final class ConversationTurnCompletionTests: XCTestCase {
    func testMockSendReturnsTokenAndCancellationPublishesMatchingTerminal() {
        let source = MockConversationSource()

        let token = source.send("hello")
        XCTAssertNotNil(token)
        XCTAssertNil(source.model.turnCompletion)

        source.cancel()

        XCTAssertEqual(source.model.turnCompletion?.token, token)
        XCTAssertEqual(source.model.turnCompletion?.outcome, .cancelled)
        XCTAssertEqual(source.model.turnCompletion?.finalAssistantText, "")
    }

    func testMockBlockedSendDoesNotMintTokenOrConsumeMessage() {
        let source = MockConversationSource()
        let originalCount = source.model.messages.count

        XCTAssertNotNil(source.send("first"))
        XCTAssertNil(source.send("blocked"))

        XCTAssertEqual(source.model.messages.count, originalCount + 1)
        XCTAssertEqual(source.model.messages.last?.text, "first")
        source.cancel()
    }

    #if canImport(engine_mobileFFI)

        private let zeroCost = CostDto(
            totalUsd: 0,
            inputTokens: 0,
            outputTokens: 0,
            apiCalls: 0,
            sessionDurationSecs: 0,
            formatted: "$0.00"
        )

        private func makeSource() -> EngineConversationSource {
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                apiKey: "",
                model: "",
                appSandboxRoot: NSTemporaryDirectory(),
                projectCwd: nil
            )
            let source = EngineConversationSource(config: config)
            source.setCommandSubmitterForTesting { _ in }
            return source
        }

        private func flushTasks(_ count: Int = 6) async {
            for _ in 0..<count { await Task.yield() }
        }

        func testEnginePublishesTrimmedFinalTextForMatchingCompletedTurn() {
            let source = makeSource()

            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            source.applyForTesting(.turnStarted(turnId: token.clientTurnId))
            source.applyForTesting(.textDelta(text: "  final answer  \n"))
            source.applyForTesting(
                .turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost)
            )

            XCTAssertEqual(
                source.model.turnCompletion,
                ConversationTurnCompletion(
                    token: token,
                    outcome: .completed,
                    finalAssistantText: "final answer"
                )
            )
        }

        func testCompletedTurnPublishesOnlyAssistantTextBlocksForSpeech() {
            let source = makeSource()

            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            source.applyForTesting(
                .messageComplete(
                    stopReason: "end_turn",
                    message: MessageDto(role: "assistant", blocks: [
                        .thinking(thinking: "private reasoning", signature: nil),
                        .toolUse(id: "tool-1", tool: "Read", inputJson: #"{"path":"/tmp/a"}"#),
                        .text(text: "  spoken answer  "),
                        .toolResult(
                            id: "tool-1",
                            tool: "Read",
                            resultJson: #"{"result":"secret tool output"}"#,
                            isError: false,
                            oldString: nil,
                            newString: nil,
                            filePath: nil
                        ),
                    ])
                )
            )
            source.applyForTesting(
                .turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost)
            )

            XCTAssertEqual(source.model.turnCompletion?.token, token)
            XCTAssertEqual(source.model.turnCompletion?.outcome, .completed)
            XCTAssertEqual(source.model.turnCompletion?.finalAssistantText, "spoken answer")
        }

        func testEngineFailurePublishesNonSpeakableCompletion() {
            let source = makeSource()

            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            source.applyForTesting(.textDelta(text: "partial response"))
            source.applyForTesting(.error(kind: .transport, message: "offline"))

            XCTAssertEqual(source.model.turnCompletion?.token, token)
            XCTAssertEqual(source.model.turnCompletion?.outcome, .failed)
            XCTAssertEqual(source.model.turnCompletion?.finalAssistantText, "")
        }

        func testSessionSwitchInvalidatesTokenAndDropsLateTerminal() {
            let source = makeSource()

            guard let oldToken = source.send("old question") else {
                return XCTFail("expected a turn token")
            }
            source.applyForTesting(.textDelta(text: "stale answer"))
            source.applyForTesting(.sessionResumed(sessionId: "new-session", messages: []))
            source.applyForTesting(
                .turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost)
            )

            XCTAssertNil(source.model.turnCompletion)
            XCTAssertFalse(source.model.streaming)

            guard let newToken = source.send("new question") else {
                return XCTFail("expected a token in the new session")
            }
            XCTAssertNotEqual(newToken, oldToken)
            XCTAssertNotEqual(newToken.sessionEpoch, oldToken.sessionEpoch)
        }

        func testCancelledCompletionNeverCarriesPartialAssistantText() {
            let source = makeSource()

            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            source.applyForTesting(.textDelta(text: "partial response"))
            source.applyForTesting(
                .turnEnded(outcome: .cancelled, stopReason: "cancelled", cost: zeroCost)
            )

            XCTAssertEqual(source.model.turnCompletion?.token, token)
            XCTAssertEqual(source.model.turnCompletion?.outcome, .cancelled)
            XCTAssertEqual(source.model.turnCompletion?.finalAssistantText, "")
        }

        func testLatePromptSubmissionFailureAfterSessionSwitchIsIgnored() async {
            enum SubmitFailure: Error { case rejected }

            let source = makeSource()
            var releaseSubmission: CheckedContinuation<Void, Never>?
            source.setCommandSubmitterForTesting { command in
                guard case .sendPrompt = command else { return }
                await withCheckedContinuation { continuation in
                    releaseSubmission = continuation
                }
                throw SubmitFailure.rejected
            }

            XCTAssertNotNil(source.send("old question"))
            await flushTasks()
            source.applyForTesting(.sessionResumed(sessionId: "new-session", messages: []))
            releaseSubmission?.resume()
            releaseSubmission = nil
            await flushTasks()

            XCTAssertNil(source.model.turnCompletion)
            XCTAssertNil(source.model.error)
            XCTAssertTrue(source.model.messages.isEmpty)
        }

    #endif
}
