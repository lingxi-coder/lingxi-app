import Combine
import UIKit
import XCTest

@testable import LingxiCode

#if canImport(engine_mobileFFI)
    import engine_mobileFFI
#endif

@MainActor
final class ConversationTurnCompletionTests: XCTestCase {
    func testMockBackgroundingKeepsActiveTurnRunning() {
        let source = MockConversationSource()

        XCTAssertNotNil(source.send("hello"))
        source.handleBackground()

        XCTAssertTrue(source.model.streaming)
        XCTAssertNil(source.model.turnCompletion)
        source.cancel()
    }

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

        func testEngineBackgroundingKeepsActiveTurnRunning() {
            let source = makeSource()

            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            source.applyForTesting(.turnStarted(turnId: token.clientTurnId))
            source.handleBackground()

            XCTAssertTrue(source.model.streaming)
            XCTAssertNil(source.model.turnCompletion)
            source.cancel()
        }

        func testEnginePublishesSequencedSpeechDeltasForOwnedTurn() {
            let source = makeSource()
            var updates: [ConversationTurnSpeechUpdate] = []
            let subscription = source.model.turnSpeechUpdates.sink { updates.append($0) }
            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }

            source.applyForTesting(.textDelta(text: "same"))
            XCTAssertEqual(updates.last, ConversationTurnSpeechUpdate(
                token: token,
                sequence: 1,
                delta: "same"
            ))

            source.applyForTesting(.textDelta(text: "same"))
            XCTAssertEqual(updates.map(\.sequence), [1, 2])
            XCTAssertEqual(updates.map(\.delta), ["same", "same"])
            withExtendedLifetime(subscription) {}
        }

        func testSessionSwitchDropsLateSpeechDelta() {
            let source = makeSource()
            var updates: [ConversationTurnSpeechUpdate] = []
            let subscription = source.model.turnSpeechUpdates.sink { updates.append($0) }
            XCTAssertNotNil(source.send("old question"))
            source.applyForTesting(.textDelta(text: "old"))
            XCTAssertEqual(updates.count, 1)

            source.expectSessionResumeForTesting("new-session")
            source.applyForTesting(.sessionResumed(sessionId: "new-session", messages: []))
            source.applyForTesting(.textDelta(text: "late"))
            XCTAssertEqual(updates.count, 1)
            withExtendedLifetime(subscription) {}
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
                        .toolUse(id: "tool-1", tool: "Read", inputJson: #"{"path":"/tmp/a"}"#, header: nil),
                        .text(text: "  spoken answer  "),
                        .toolResult(
                            id: "tool-1",
                            tool: "Read",
                            resultJson: #"{"result":"secret tool output"}"#,
                            isError: false,
                            oldString: nil,
                            newString: nil,
                            filePath: nil,
                            display: nil
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
            source.expectSessionResumeForTesting("new-session")
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
            source.expectSessionResumeForTesting("new-session")
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

@MainActor
final class ConversationBackgroundExecutionControllerTests: XCTestCase {
    func testActiveTurnHoldsFiniteBackgroundTaskUntilItSettles() {
        var expirationHandler: (() -> Void)?
        var endedTasks: [UIBackgroundTaskIdentifier] = []
        let controller = ConversationBackgroundExecutionController(
            beginTask: { handler in
                expirationHandler = handler
                return UIBackgroundTaskIdentifier(rawValue: 42)
            },
            endTask: { endedTasks.append($0) }
        )

        controller.setTurnActive(true)
        controller.setTurnActive(true)
        XCTAssertNotNil(expirationHandler)
        XCTAssertTrue(endedTasks.isEmpty)

        controller.setTurnActive(false)
        XCTAssertEqual(endedTasks.map(\.rawValue), [42])
    }

    func testExpirationEndsLeaseWithoutCancellingAndCanRearm() {
        var expirationHandlers: [() -> Void] = []
        var nextTask = 0
        var endedTasks: [UIBackgroundTaskIdentifier] = []
        let controller = ConversationBackgroundExecutionController(
            beginTask: { handler in
                nextTask += 1
                expirationHandlers.append(handler)
                return UIBackgroundTaskIdentifier(rawValue: nextTask)
            },
            endTask: { endedTasks.append($0) }
        )

        controller.setTurnActive(true)
        expirationHandlers[0]()
        XCTAssertEqual(endedTasks.map(\.rawValue), [1])

        controller.setTurnActive(true)
        XCTAssertEqual(expirationHandlers.count, 2)
    }

    func testInvalidBackgroundTaskIdentifierDoesNotLatchAndCanRetry() {
        var beginCount = 0
        var endedTasks: [UIBackgroundTaskIdentifier] = []
        let controller = ConversationBackgroundExecutionController(
            beginTask: { _ in
                beginCount += 1
                return beginCount == 1
                    ? .invalid
                    : UIBackgroundTaskIdentifier(rawValue: 73)
            },
            endTask: { endedTasks.append($0) }
        )

        controller.setTurnActive(true)
        controller.setTurnActive(true)

        XCTAssertEqual(beginCount, 2, "an invalid acquisition must not block a retry")
        controller.setTurnActive(false)
        XCTAssertEqual(
            endedTasks.map(\.rawValue),
            [73],
            "only a valid task identifier may be ended")
    }

    func testBackgroundExecutionCoversOutOfBandAndStructuredRunningWork() {
        let model = ConversationModel()
        model.streaming = false
        XCTAssertFalse(model.requiresBackgroundExecution)

        model.backgroundTasks = [
            BackgroundTaskSnapshot(id: "task-1", descriptionText: "Build", status: .running),
        ]
        XCTAssertTrue(
            model.requiresBackgroundExecution,
            "a background task outlives the assistant streaming turn")

        model.backgroundTasks[0].status = .completed
        model.items = [
            .run(ConversationExecutionRun(
                id: "run-1",
                sessionId: "session-a",
                turnId: 1,
                status: .completed,
                tools: [ConversationToolTrace(
                    id: "tool-1",
                    tool: "Shell",
                    status: .running,
                    inputSummary: nil,
                    outputSummary: nil,
                    elapsedMs: nil
                )]
            )),
        ]
        XCTAssertTrue(
            model.requiresBackgroundExecution,
            "a still-running tool must keep the finite background assertion")

        model.items = []
        XCTAssertFalse(model.requiresBackgroundExecution)
    }
}
