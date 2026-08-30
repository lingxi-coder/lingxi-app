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
                appSandboxRoot: FileManager.default.temporaryDirectory
                    .appendingPathComponent(UUID().uuidString, isDirectory: true).path,
                projectCwd: nil,
                visionDelegationEnabled: true
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

        func testRecoverablePauseSubmitsPauseInsteadOfCancel() async throws {
            let source = makeSource()
            source.model.activeSessionId = "test-session"
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { commands.append($0) }
            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            await flushTasks()
            commands.removeAll()

            let pauseTask = Task { try await source.markActiveTurnPausedRecoverable(token) }
            await flushTasks()

            XCTAssertEqual(commands.count, 1)
            XCTAssertTrue(source.model.streaming, "the turn stays owned until the host acknowledgement")
            XCTAssertNil(source.model.turnCompletion)
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                state: .pausedRecoverable,
                firstSequence: 0,
                lastSequence: 0,
                safeToResume: true,
                reason: "background_time_expired"
            )))
            try await pauseTask.value
            XCTAssertFalse(source.model.streaming, "only PausedRecoverable makes the turn sendable")
            guard case let .pauseTurn(turnId, reason) = commands[0] else {
                return XCTFail("expected PauseTurn, got \(commands[0])")
            }
            XCTAssertEqual(turnId, token.clientTurnId)
            XCTAssertEqual(reason, "background_time_expired")
        }

        func testRecoverablePauseSubmissionFailureIsVisibleAndNotCancelled() async {
            enum SubmitFailure: Error { case rejected }

            let source = makeSource()
            source.setCommandSubmitterForTesting { command in
                if case .pauseTurn = command {
                    throw SubmitFailure.rejected
                }
            }
            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            await flushTasks()

            do {
                try await source.markActiveTurnPausedRecoverable(token)
                XCTFail("a rejected PauseTurn must fail")
            } catch {
                // Expected: the source exposes the host error and retains turn
                // ownership because no PausedRecoverable acknowledgement arrived.
            }

            XCTAssertTrue(source.model.streaming)
            XCTAssertEqual(source.model.error?.kind, .host)
            XCTAssertNil(source.model.turnCompletion)
            XCTAssertNotEqual(source.model.notice, .cancelled)
        }

        func testWarmPausedTurnReattachesFromLatestCursorAndResumesOnForeground() async throws {
            let source = makeSource()
            source.model.activeSessionId = "test-session"
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                commands.append(command)
                if case let .resumeTurn(turnId) = command {
                    source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                        sessionId: "test-session",
                        turnId: turnId,
                        state: .running,
                        firstSequence: 1,
                        lastSequence: 7,
                        safeToResume: true,
                        reason: nil
                    )))
                }
            }
            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            await flushTasks()
            commands.removeAll()
            source.recordDurableSequenceForTesting(turnId: token.clientTurnId, sequence: 7)
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                state: .pausedRecoverable,
                firstSequence: 1,
                lastSequence: 7,
                safeToResume: true,
                reason: "background_time_expired"
            )))

            XCTAssertFalse(source.model.streaming)
            source.handleForeground()
            // A second foreground delivery while the first recovery chain is
            // running must not submit another AttachTurn.
            source.handleForeground()
            await flushTasks(16)

            let recoveryCommands = commands.compactMap { command -> ClientCommand? in
                switch command {
                case .attachTurn, .resumeTurn: return command
                default: return nil
                }
            }
            XCTAssertEqual(recoveryCommands.count, 2)
            guard case let .attachTurn(turnId, afterSequence) = recoveryCommands[0] else {
                return XCTFail("expected AttachTurn first, got \(recoveryCommands)")
            }
            XCTAssertEqual(turnId, token.clientTurnId)
            XCTAssertEqual(afterSequence, 7)
            guard case let .resumeTurn(resumedTurnID) = recoveryCommands[1] else {
                return XCTFail("expected ResumeTurn second, got \(recoveryCommands)")
            }
            XCTAssertEqual(resumedTurnID, token.clientTurnId)
            XCTAssertTrue(source.model.streaming, "the resumed turn owns the slot again")
        }

        func testAttachRunningSnapshotDoesNotUnlockWhenResumeFails() async throws {
            enum ResumeFailure: Error { case rejected }

            let source = makeSource()
            source.model.activeSessionId = "test-session"
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                commands.append(command)
                switch command {
                case let .attachTurn(turnID, _):
                    source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                        sessionId: "test-session",
                        turnId: turnID,
                        state: .running,
                        firstSequence: 1,
                        lastSequence: 2,
                        safeToResume: true,
                        reason: nil
                    )))
                    source.applyForTesting(.turnEventReplay(
                        sessionId: "test-session",
                        turnId: turnID,
                        sequence: 2,
                        eventJson: #"{"type":"text_delta","text":"replayed"}"#
                    ))
                case .resumeTurn:
                    throw ResumeFailure.rejected
                default:
                    break
                }
            }
            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            await flushTasks()
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                state: .pausedRecoverable,
                firstSequence: 1,
                lastSequence: 1,
                safeToResume: true,
                reason: "background_time_expired"
            )))

            source.handleForeground()
            await flushTasks(24)

            XCTAssertTrue(commands.contains { if case .resumeTurn = $0 { return true }; return false })
            XCTAssertFalse(source.model.streaming)
            XCTAssertTrue(source.model.hasInactiveDurableRecovery)
            XCTAssertTrue(source.model.hasUnresolvedTurnRecovery)
            XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, token.clientTurnId)
            XCTAssertFalse(source.model.requiresBackgroundExecution)
            XCTAssertNil(source.send("must wait"))
        }

        func testPausedAttachReplayToolResumeFailureSettlesOnlyCorrelatedMainRun() async throws {
            enum ResumeFailure: Error { case rejected }

            let source = makeSource()
            source.model.activeSessionId = "test-session"
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                commands.append(command)
                switch command {
                case let .attachTurn(turnID, _):
                    source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                        sessionId: "test-session",
                        turnId: turnID,
                        state: .pausedRecoverable,
                        firstSequence: 1,
                        lastSequence: 3,
                        safeToResume: true,
                        reason: "background_time_expired"
                    )))
                    source.applyForTesting(.turnEventReplay(
                        sessionId: "test-session",
                        turnId: turnID,
                        sequence: 3,
                        eventJson: #"{"type":"tool_use_started","id":"replayed-tool","tool":"Read","input_json":"{}"}"#
                    ))
                case .resumeTurn:
                    throw ResumeFailure.rejected
                default:
                    break
                }
            }
            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            await flushTasks()
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                state: .pausedRecoverable,
                firstSequence: 1,
                lastSequence: 2,
                safeToResume: true,
                reason: "background_time_expired"
            )))

            source.handleForeground()
            await flushTasks(24)

            XCTAssertTrue(source.model.hasInactiveDurableRecovery)
            XCTAssertTrue(source.model.hasUnresolvedTurnRecovery)
            XCTAssertFalse(source.model.requiresBackgroundExecution)
            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected the replayed main run")
            }
            XCTAssertEqual(run.status, .restored)
            XCTAssertFalse(run.tools.contains(where: { $0.status == .running }))
            XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, token.clientTurnId)
            XCTAssertTrue(commands.contains { if case .resumeTurn = $0 { return true }; return false })
        }

        func testPausedRecoveryGatesSendAndNewConversationUntilTerminal() async throws {
            let source = makeSource()
            source.model.activeSessionId = "test-session"
            source.setCommandSubmitterForTesting { _ in }
            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                state: .pausedRecoverable,
                firstSequence: 1,
                lastSequence: 1,
                safeToResume: true,
                reason: "background_time_expired"
            )))

            XCTAssertNil(source.send("must wait"))
            source.startNewConversation()
            XCTAssertEqual(source.model.activeSessionId, "test-session")
            XCTAssertEqual(source.durableAttachCursorForTesting(), 0)

            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                state: .completed,
                firstSequence: 1,
                lastSequence: 1,
                safeToResume: false,
                reason: nil
            )))
            XCTAssertNotNil(source.send("after recovery"))
        }

        func testResumeWaitingForUserRemainsGatedAndForegroundIsIdempotent() async throws {
            let source = makeSource()
            source.model.activeSessionId = "test-session"
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                commands.append(command)
                if case let .resumeTurn(turnId) = command {
                    source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                        sessionId: "test-session",
                        turnId: turnId,
                        state: .waitingForUser,
                        firstSequence: 1,
                        lastSequence: 1,
                        safeToResume: false,
                        reason: "replay_boundary_requires_user"
                    )))
                }
            }
            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            await flushTasks()
            commands.removeAll()
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                state: .pausedRecoverable,
                firstSequence: 1,
                lastSequence: 1,
                safeToResume: true,
                reason: "background_time_expired"
            )))

            source.handleForeground()
            await flushTasks(16)
            source.handleForeground()
            await flushTasks(8)

            XCTAssertEqual(commands.filter {
                if case .attachTurn = $0 { return true }
                return false
            }.count, 1)
            XCTAssertEqual(commands.filter {
                if case .resumeTurn = $0 { return true }
                return false
            }.count, 1)
            XCTAssertFalse(source.model.streaming)
            XCTAssertNil(source.send("must wait"))
        }

        func testInactiveWaitingForUserCanCancelAndTerminalStateReleasesGates() async throws {
            let source = makeSource()
            source.model.activeSessionId = "test-session"
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                commands.append(command)
                guard case let .cancel(turnId?) = command else { return }
                // The host terminalizes the matching inactive checkpoint before
                // the cancel command's completion is observed by the caller.
                source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                    sessionId: "test-session",
                    turnId: turnId,
                    state: .cancelled,
                    firstSequence: 1,
                    lastSequence: 1,
                    safeToResume: false,
                    reason: "user_discarded"
                )))
            }
            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            await flushTasks()
            commands.removeAll()

            // A live executor-backed WaitingForUser remains ordinary active
            // Stop state. Enter the post-reattach executor-less path first so
            // this test exercises the visible Discard affordance.
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                state: .pausedRecoverable,
                firstSequence: 1,
                lastSequence: 1,
                safeToResume: false,
                reason: "background_time_expired"
            )))
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                state: .waitingForUser,
                firstSequence: 1,
                lastSequence: 1,
                safeToResume: false,
                reason: "permission_required"
            )))
            XCTAssertFalse(source.model.streaming)
            XCTAssertTrue(source.model.hasInactiveDurableRecovery)
            XCTAssertNil(source.send("must wait"))

            try await source.cancelAndWait()

            guard case let .cancel(cancelledTurnID) = commands.first else {
                return XCTFail("expected Cancel for the inactive durable turn, got \(commands)")
            }
            XCTAssertEqual(cancelledTurnID, token.clientTurnId)
            XCTAssertFalse(source.model.streaming)
            XCTAssertFalse(source.model.isCancelling)
            XCTAssertFalse(source.model.hasInactiveDurableRecovery)
            XCTAssertEqual(source.model.notice, .cancelled)
            XCTAssertNil(source.durableAttachCursorForTesting())
            XCTAssertNotNil(source.send("after discard"), "terminal recovery must release the send gate")
        }

        func testCancelCommandOKRetainsOwnershipUntilMatchingTerminalRecoveryState() async throws {
            let source = makeSource()
            source.model.activeSessionId = "test-session"
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { commands.append($0) }
            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            await flushTasks()
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                state: .pausedRecoverable,
                firstSequence: 0,
                lastSequence: 0,
                safeToResume: false,
                reason: "background_time_expired"
            )))

            let cancellation = Task { try await source.cancelAndWait() }
            await flushTasks(8)
            XCTAssertTrue(source.model.isCancelling)
            XCTAssertTrue(source.model.hasInactiveDurableRecovery)
            XCTAssertTrue(commands.contains {
                if case let .cancel(turnID) = $0 { return turnID == token.clientTurnId }
                return false
            })

            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId + 1,
                state: .cancelled,
                firstSequence: 0,
                lastSequence: 0,
                safeToResume: false,
                reason: "stale"
            )))
            await flushTasks(8)
            XCTAssertTrue(source.model.isCancelling, "a stale turn must not acknowledge Cancel")
            XCTAssertTrue(source.model.hasInactiveDurableRecovery)

            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                state: .cancelled,
                firstSequence: 0,
                lastSequence: 0,
                safeToResume: false,
                reason: "user_discarded"
            )))
            try await cancellation.value
            XCTAssertFalse(source.model.isCancelling)
            XCTAssertFalse(source.model.hasInactiveDurableRecovery)
            XCTAssertNotNil(source.send("after discard"))
        }

        func testEngineListenerPreservesFIFOEventOrder() async {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 900, sessionId: "test-session")
            let listener = source.makeEventListenerForTesting()

            listener.enqueueForTesting(.textDelta(text: "before tool"))
            listener.enqueueForTesting(.toolUseStarted(
                id: "ordered-tool",
                tool: "Read",
                inputJson: #"{"path":"a"}"#,
                header: nil
            ))
            listener.enqueueForTesting(.textDelta(text: "after tool"))
            await listener.waitUntilIdle()

            let kinds = source.model.items.map { item -> String in
                switch item {
                case .message: return "message"
                case .run: return "run"
                case .toolCall: return "tool"
                case .notice: return "notice"
                case .commandOutput: return "command"
                }
            }
            XCTAssertEqual(kinds, ["message", "run", "message"])
            XCTAssertEqual(source.model.messages.map(\.text), ["before tool", "after tool"])
        }

        func testEngineListenerLargeBurstYieldsBetweenBoundedBatchesAndWaitsUntilIdle() async {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 901, sessionId: "test-session")
            let listener = source.makeEventListenerForTesting()
            let deltas = (0..<8192).map { "\($0)," }

            for delta in deltas {
                listener.enqueueForTesting(.textDelta(text: delta))
            }
            await listener.waitUntilIdle()

            XCTAssertEqual(source.model.messages.map(\.text), [deltas.joined()])
            XCTAssertEqual(listener.pumpInvocationsForTesting, 32)
            XCTAssertEqual(listener.pumpYieldCountForTesting, 31)
            XCTAssertTrue(listener.isIdleForTesting)
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
        var expirations = 0
        let controller = ConversationBackgroundExecutionController(
            beginTask: { handler in
                nextTask += 1
                expirationHandlers.append(handler)
                return UIBackgroundTaskIdentifier(rawValue: nextTask)
            },
            endTask: { endedTasks.append($0) }
        )
        controller.setExpirationObserver { expirations += 1 }

        controller.setTurnActive(true)
        expirationHandlers[0]()
        XCTAssertEqual(endedTasks.map(\.rawValue), [1])
        XCTAssertEqual(expirations, 1)

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

    func testContinuedProcessingDevicesDoNotAcquireFiniteBackgroundAssertion() {
        var beginCount = 0
        let controller = ConversationBackgroundExecutionController(
            beginTask: { _ in
                beginCount += 1
                return UIBackgroundTaskIdentifier(rawValue: 88)
            },
            endTask: { _ in },
            shouldUseFiniteAssertion: { false }
        )

        controller.setTurnActive(true)

        XCTAssertEqual(beginCount, 0)
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
