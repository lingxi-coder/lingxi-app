import Combine
import UIKit
import XCTest

@testable import LingxiCode

#if canImport(harness_runtimeFFI)
    import harness_runtimeFFI
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

    func testMockRunningSendQueuesPendingMessage() {
        let source = MockConversationSource()
        let originalCount = source.model.messages.count

        let token = source.send("first")
        XCTAssertNotNil(token)
        XCTAssertEqual(source.send("pending"), token)

        XCTAssertEqual(source.model.messages.count, originalCount + 2)
        XCTAssertEqual(source.model.messages.last?.text, "pending")
        source.cancel()
    }

    #if canImport(harness_runtimeFFI)

        private let zeroCost = CostDto(
            totalUsd: 0,
            inputTokens: 0,
            outputTokens: 0,
            apiCalls: 0,
            sessionDurationSecs: 0,
            formatted: "$0.00"
        )

        private func makeSource(
            appSandboxRoot: String = FileManager.default.temporaryDirectory
                .appendingPathComponent(UUID().uuidString, isDirectory: true).path
        ) -> EngineConversationSource {
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                apiKey: "",
                model: "",
                appSandboxRoot: appSandboxRoot,
                projectCwd: nil,
                sessionMode: .code,
                visionDelegationEnabled: true
            )
            let source = EngineConversationSource(config: config, recoveryStoreRootForTesting: appSandboxRoot)
            source.setCommandSubmitterForTesting { _ in }
            return source
        }

        private func flushTasks(_ count: Int = 6) async {
            for _ in 0..<count { await Task.yield() }
        }

        private func recoverySnapshot(
            sessionID: String,
            turnID: UInt64,
            revision: UInt64,
            state: TurnRecoveryStateDto,
            lastSequence: UInt64 = 0,
            projectionUncertain: Bool = false,
            reason: String? = nil
        ) -> TurnRecoverySnapshotDto {
            TurnRecoverySnapshotDto(
                sessionId: sessionID,
                turnId: turnID,
                revision: revision,
                state: state,
                firstSequence: 0,
                lastSequence: lastSequence,
                safeToResume: false,
                projectionUncertain: projectionUncertain,
                reason: reason
            )
        }

        func testEnginePublishesTrimmedFinalTextForMatchingCompletedTurn() {
            let source = makeSource()

            guard let token = source.send("question") else {
                return XCTFail("expected a turn token")
            }
            source.applyForTesting(.turnStarted(turnId: token.clientTurnId))
            source.applyForTesting(.textDelta(text: "  final answer  \n".clientTextDto))
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

        func testPromptSuggestionSourceUnitsReachSendPrompt() async {
            let source = makeSource()
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { submitted.append($0) }
            let sourceUnits: [UInt16] = [0xD800, 0x0078]

            XCTAssertNotNil(source.send(
                "\u{FFFD}x",
                sourceCodeUnits: sourceUnits,
                images: [],
                selectedSlashCommand: nil
            ))
            await flushTasks()

            guard case let .sendPrompt(_, text, _, _, _, _) = submitted.first else {
                return XCTFail("accepted suggestion must submit SendPrompt")
            }
            XCTAssertEqual(text.codeUnits, sourceUnits)
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

        func testEngineRunningSendSubmitsPendingMessageWithoutReplacingActiveTurn() async {
            let source = makeSource()
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in submitted.append(command) }

            guard let token = source.send("first") else {
                return XCTFail("expected a turn token")
            }
            await flushTasks()
            source.applyForTesting(.turnStarted(turnId: token.clientTurnId))

            XCTAssertEqual(source.send("pending 🧭"), token)
            await flushTasks()

            XCTAssertTrue(source.model.streaming)
            XCTAssertEqual(source.model.messages.suffix(2).map(\.text), ["first", "pending 🧭"])
            XCTAssertEqual(submitted.count, 2)
            guard case let .sendPrompt(_, text, _, images, _, _) = submitted.last else {
                return XCTFail("pending send must use SendPrompt")
            }
            XCTAssertEqual(text.codeUnits, Array("pending 🧭".utf16))
            XCTAssertTrue(images.isEmpty)
        }

        func testEngineRunningSendQueuesBashModeAndExactCommandBody() async {
            let source = makeSource()
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { submitted.append($0) }

            guard let token = source.send("first") else {
                return XCTFail("expected a turn token")
            }
            await flushTasks()
            source.applyForTesting(.turnStarted(turnId: token.clientTurnId))

            XCTAssertEqual(source.send("!  printf !!  "), token)
            await flushTasks()

            XCTAssertEqual(source.model.messages.suffix(2).map(\.text), ["first", "!  printf !!  "])
            guard case let .sendPrompt(_, text, mode, _, _, _) = submitted.last else {
                return XCTFail("queued bang input must use SendPrompt")
            }
            XCTAssertEqual(text.displayText, "  printf !!  ")
            XCTAssertEqual(mode, .bash)
        }

        func testLeadingBangSubmitsExactBashCommandAndMode() async {
            let source = makeSource()
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { submitted.append($0) }

            XCTAssertNotNil(source.send("!  printf !!  "))
            await flushTasks()

            guard case let .sendPrompt(_, text, mode, _, _, _) = submitted.first else {
                return XCTFail("leading bang must submit a SendPrompt")
            }
            XCTAssertEqual(text.displayText, "  printf !!  ")
            XCTAssertEqual(mode, .bash)
            XCTAssertEqual(source.model.messages.last?.text, "!  printf !!  ")
        }

        func testLeadingBangBeforeCombiningMarkUsesOneUtf16UnitAndBashMode() async {
            let source = makeSource()
            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { submitted.append($0) }
            let input = "!\u{0301}cmd"

            XCTAssertNotNil(source.send(input))
            await flushTasks()

            guard case let .sendPrompt(_, text, mode, _, _, _) = submitted.first else {
                return XCTFail("leading UTF-16 bang must submit a SendPrompt")
            }
            XCTAssertEqual(mode, .bash)
            XCTAssertEqual(text.codeUnits, Array("\u{0301}cmd".utf16))
            XCTAssertEqual(source.model.messages.last?.text, input)
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
                revision: 1,
                state: .pausedRecoverable,
                firstSequence: 0,
                lastSequence: 0,
                safeToResume: true,
                projectionUncertain: false,
                reason: "background_time_expired"
            )))
            try await pauseTask.value
            XCTAssertFalse(source.model.streaming, "only PausedRecoverable makes the turn sendable")
            guard case let .pauseTurn(sessionID, turnId, reason) = commands[0] else {
                return XCTFail("expected PauseTurn, got \(commands[0])")
            }
            XCTAssertEqual(turnId, token.clientTurnId)
            XCTAssertEqual(sessionID, "test-session")
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
                if case let .attachTurn(sessionID, turnID, _, requestID) = command {
                    source.applyForTesting(.turnAttachStarted(requestId: requestID, snapshot: self.recoverySnapshot(
                        sessionID: sessionID, turnID: turnID, revision: 1, state: .pausedRecoverable
                    )))
                    source.applyForTesting(.turnAttachFinished(requestId: requestID, sessionId: sessionID, turnId: turnID))
                }
                if case let .resumeTurn(_, turnId) = command {
                    source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                        sessionId: "test-session",
                        turnId: turnId,
                        revision: 2,
                        state: .running,
                        firstSequence: 1,
                        lastSequence: 7,
                        safeToResume: true,
                        projectionUncertain: false,
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
                revision: 1,
                state: .pausedRecoverable,
                firstSequence: 1,
                lastSequence: 7,
                safeToResume: true,
                projectionUncertain: false,
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
            guard case let .attachTurn(owner, turnId, afterSequence, requestID) = recoveryCommands[0] else {
                return XCTFail("expected AttachTurn first, got \(recoveryCommands)")
            }
            XCTAssertEqual(turnId, token.clientTurnId)
            XCTAssertEqual(owner, "test-session")
            XCTAssertFalse(requestID.isEmpty)
            XCTAssertEqual(afterSequence, 7)
            guard case let .resumeTurn(resumeOwner, resumedTurnID) = recoveryCommands[1] else {
                return XCTFail("expected ResumeTurn second, got \(recoveryCommands)")
            }
            XCTAssertEqual(resumedTurnID, token.clientTurnId)
            XCTAssertEqual(resumeOwner, "test-session")
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
                case let .attachTurn(_, turnID, _, requestID):
                    source.applyForTesting(.turnAttachStarted(requestId: requestID, snapshot: TurnRecoverySnapshotDto(
                        sessionId: "test-session",
                        turnId: turnID,
                        revision: 2,
                        state: .running,
                        firstSequence: 1,
                        lastSequence: 2,
                        safeToResume: true,
                        projectionUncertain: false,
                        reason: nil
                    )))
                    source.applyForTesting(.turnEventReplay(
                        sessionId: "test-session",
                        turnId: turnID,
                        sequence: 2,
                        eventJson: #"{"type":"text_delta","text":"replayed"}"#
                    ))
                    source.applyForTesting(.turnAttachFinished(requestId: requestID, sessionId: "test-session", turnId: turnID))
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
            source.recordDurableSequenceForTesting(turnId: token.clientTurnId, sequence: 1)
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                revision: 1,
                state: .pausedRecoverable,
                firstSequence: 1,
                lastSequence: 1,
                safeToResume: true,
                projectionUncertain: false,
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
                case let .attachTurn(_, turnID, _, requestID):
                    source.applyForTesting(.turnAttachStarted(requestId: requestID, snapshot: TurnRecoverySnapshotDto(
                        sessionId: "test-session",
                        turnId: turnID,
                        revision: 1,
                        state: .pausedRecoverable,
                        firstSequence: 1,
                        lastSequence: 3,
                        safeToResume: true,
                        projectionUncertain: false,
                        reason: "background_time_expired"
                    )))
                    source.applyForTesting(.turnEventReplay(
                        sessionId: "test-session",
                        turnId: turnID,
                        sequence: 3,
                        eventJson: #"{"type":"tool_use_started","id":"replayed-tool","tool":"Read","input_json":"{}","header":{"verb":"read","label":"Read","title":"Read"}}"#
                    ))
                    source.applyForTesting(.turnAttachFinished(requestId: requestID, sessionId: "test-session", turnId: turnID))
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
            source.recordDurableSequenceForTesting(turnId: token.clientTurnId, sequence: 2)
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                revision: 1,
                state: .pausedRecoverable,
                firstSequence: 1,
                lastSequence: 2,
                safeToResume: true,
                projectionUncertain: false,
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
                revision: 1,
                state: .pausedRecoverable,
                firstSequence: 1,
                lastSequence: 1,
                safeToResume: true,
                projectionUncertain: false,
                reason: "background_time_expired"
            )))

            XCTAssertNil(source.send("must wait"))
            source.startNewConversation()
            XCTAssertEqual(source.model.activeSessionId, "test-session")
            XCTAssertEqual(source.durableAttachCursorForTesting(), 0)

            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                revision: 2,
                state: .completed,
                firstSequence: 1,
                lastSequence: 1,
                safeToResume: false,
                projectionUncertain: false,
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
                if case let .attachTurn(sessionID, turnID, _, requestID) = command {
                    source.applyForTesting(.turnAttachStarted(requestId: requestID, snapshot: self.recoverySnapshot(
                        sessionID: sessionID, turnID: turnID, revision: 1, state: .pausedRecoverable
                    )))
                    source.applyForTesting(.turnAttachFinished(requestId: requestID, sessionId: sessionID, turnId: turnID))
                }
                if case let .resumeTurn(_, turnId) = command {
                    source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                        sessionId: "test-session",
                        turnId: turnId,
                        revision: 2,
                        state: .waitingForUser,
                        firstSequence: 1,
                        lastSequence: 1,
                        safeToResume: false,
                        projectionUncertain: false,
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
                revision: 1,
                state: .pausedRecoverable,
                firstSequence: 1,
                lastSequence: 1,
                safeToResume: true,
                projectionUncertain: false,
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
                guard case let .cancel(_, turnId?) = command else { return }
                // The host terminalizes the matching inactive checkpoint before
                // the cancel command's completion is observed by the caller.
                source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                    sessionId: "test-session",
                    turnId: turnId,
                    revision: 3,
                    state: .cancelled,
                    firstSequence: 1,
                    lastSequence: 1,
                    safeToResume: false,
                    projectionUncertain: false,
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
                revision: 1,
                state: .pausedRecoverable,
                firstSequence: 1,
                lastSequence: 1,
                safeToResume: false,
                projectionUncertain: false,
                reason: "background_time_expired"
            )))
            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                revision: 2,
                state: .waitingForUser,
                firstSequence: 1,
                lastSequence: 1,
                safeToResume: false,
                projectionUncertain: false,
                reason: "permission_required"
            )))
            XCTAssertFalse(source.model.streaming)
            XCTAssertTrue(source.model.hasInactiveDurableRecovery)
            XCTAssertNil(source.send("must wait"))

            try await source.cancelAndWait()

            guard case let .cancel(owner, cancelledTurnID) = commands.first else {
                return XCTFail("expected Cancel for the inactive durable turn, got \(commands)")
            }
            XCTAssertEqual(cancelledTurnID, token.clientTurnId)
            XCTAssertEqual(owner, "test-session")
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
                revision: 1,
                state: .pausedRecoverable,
                firstSequence: 0,
                lastSequence: 0,
                safeToResume: false,
                projectionUncertain: false,
                reason: "background_time_expired"
            )))

            let cancellation = Task { try await source.cancelAndWait() }
            await flushTasks(8)
            XCTAssertTrue(source.model.isCancelling)
            XCTAssertTrue(source.model.hasInactiveDurableRecovery)
            XCTAssertTrue(commands.contains {
                if case let .cancel(_, turnID) = $0 { return turnID == token.clientTurnId }
                return false
            })

            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId + 1,
                revision: 2,
                state: .cancelled,
                firstSequence: 0,
                lastSequence: 0,
                safeToResume: false,
                projectionUncertain: false,
                reason: "stale"
            )))
            await flushTasks(8)
            XCTAssertTrue(source.model.isCancelling, "a stale turn must not acknowledge Cancel")
            XCTAssertTrue(source.model.hasInactiveDurableRecovery)

            source.applyForTesting(.turnRecoveryState(snapshot: TurnRecoverySnapshotDto(
                sessionId: "test-session",
                turnId: token.clientTurnId,
                revision: 2,
                state: .cancelled,
                firstSequence: 0,
                lastSequence: 0,
                safeToResume: false,
                projectionUncertain: false,
                reason: "user_discarded"
            )))
            try await cancellation.value
            XCTAssertFalse(source.model.isCancelling)
            XCTAssertFalse(source.model.hasInactiveDurableRecovery)
            XCTAssertNotNil(source.send("after discard"))
        }

        func testCancellationBeatsLatePauseAndFailsItsAcknowledgement() async throws {
            let source = makeSource()
            source.model.activeSessionId = "revision-race"
            guard let token = source.send("question") else { return XCTFail("expected a turn") }
            await flushTasks()
            var received: [TurnRecoveryStateDto] = []
            source.setExternalEventHandler { event in
                if case let .turnRecoveryState(snapshot) = event { received.append(snapshot.state) }
            }
            let pause = Task { try await source.markActiveTurnPausedRecoverable(token) }
            await flushTasks()
            let cancel = Task { try await source.cancelAndWait() }
            await flushTasks(8)
            let terminal = recoverySnapshot(sessionID: "revision-race", turnID: token.clientTurnId,
                                            revision: 2, state: .cancelled)
            source.applyForTesting(.turnRecoveryState(snapshot: terminal))
            do {
                try await pause.value
                XCTFail("a terminal state must fail the pending pause acknowledgement")
            } catch { }
            try await cancel.value
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: "revision-race", turnID: token.clientTurnId,
                revision: 1, state: .pausedRecoverable
            )))
            XCTAssertEqual(received, [.cancelled])
            XCTAssertEqual(source.model.notice, .cancelled)
            XCTAssertFalse(source.model.hasInactiveDurableRecovery)
            XCTAssertFalse(source.model.hasUnresolvedTurnRecovery)
            XCTAssertNotNil(source.send("after cancellation"))
        }

        func testReentrantRecoveryCallbackIsAppliedAfterCurrentProjection() {
            let source = makeSource()
            let session = "reentrant-\(UUID().uuidString)"
            source.beginTurnForTesting(turnId: 71, sessionId: session)
            var order: [String] = []
            source.setExternalEventHandler { event in
                guard case let .turnRecoveryState(snapshot) = event else { return }
                if snapshot.state == .pausedRecoverable {
                    order.append("pause-enter")
                    source.applyForTesting(.turnRecoveryState(snapshot: self.recoverySnapshot(
                        sessionID: session, turnID: 71, revision: 2, state: .cancelled
                    )))
                    order.append("pause-return")
                } else if snapshot.state == .cancelled {
                    order.append("cancelled")
                }
            }
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: 71, revision: 1, state: .pausedRecoverable
            )))
            XCTAssertEqual(order, ["pause-enter", "pause-return", "cancelled"])
            XCTAssertEqual(source.model.notice, .cancelled)
        }

        func testExternalHandlerRejectsReplayForAnotherSessionOrTurn() {
            let source = makeSource()
            let session = "replay-owner-\(UUID().uuidString)"
            source.beginTurnForTesting(turnId: 71, sessionId: session)
            var forwarded: [String] = []
            source.setExternalEventHandler { event in
                if case let .textDelta(text) = event { forwarded.append(text.displayText) }
            }
            let payload = #"{"type":"text_delta","text":"accepted"}"#
            source.applyForTesting(.turnEventReplay(sessionId: "other", turnId: 71, sequence: 1, eventJson: payload))
            source.applyForTesting(.turnEventReplay(sessionId: session, turnId: 72, sequence: 1, eventJson: payload))
            source.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1, eventJson: payload))
            XCTAssertEqual(forwarded, ["accepted"])
        }

        func testReentrantRecoveryAcrossSourcesSharesTheSamePump() {
            let root = FileManager.default.temporaryDirectory
                .appendingPathComponent(UUID().uuidString, isDirectory: true).path
            let older = makeSource(appSandboxRoot: root)
            let newer = makeSource(appSandboxRoot: root)
            let session = "cross-source-\(UUID().uuidString)"
            older.beginTurnForTesting(turnId: 71, sessionId: session)
            newer.beginTurnForTesting(turnId: 71, sessionId: session)
            var order: [String] = []
            newer.setExternalEventHandler { event in
                if case .turnRecoveryState = event { order.append("cancelled") }
            }
            older.setExternalEventHandler { event in
                guard case .turnRecoveryState = event else { return }
                order.append("pause-enter")
                newer.applyForTesting(.turnRecoveryState(snapshot: self.recoverySnapshot(
                    sessionID: session, turnID: 71, revision: 2, state: .cancelled
                )))
                order.append("pause-return")
            }
            older.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: 71, revision: 1, state: .pausedRecoverable
            )))
            XCTAssertEqual(order, ["pause-enter", "pause-return", "cancelled"])
            XCTAssertEqual(newer.model.notice, .cancelled)
        }

        func testRecoveryVersionsSurviveSourceReplacementAndProtectCheckpointIdentity() {
            let root = FileManager.default.temporaryDirectory
                .appendingPathComponent(UUID().uuidString, isDirectory: true).path
            let previous = makeSource(appSandboxRoot: root)
            let replacement = makeSource(appSandboxRoot: root)
            previous.beginTurnForTesting(turnId: 71, sessionId: "session-a")
            replacement.beginTurnForTesting(turnId: 71, sessionId: "session-a")
            previous.beginDurableTurnForTesting(turnId: 71, sessionId: "session-a")
            var forwarded: [String?] = []
            previous.setExternalEventHandler { event in
                if case let .turnRecoveryState(snapshot) = event { forwarded.append(snapshot.reason) }
            }

            replacement.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: "session-a", turnID: 71, revision: 8, state: .cancelled
            )))
            previous.beginDurableTurnForTesting(turnId: 71, sessionId: "session-b")
            previous.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: "session-a", turnID: 71, revision: 7, state: .pausedRecoverable
            )))
            previous.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: "session-a", turnID: 71, revision: 8, state: .pausedRecoverable
            )))
            XCTAssertTrue(previous.model.streaming)
            XCTAssertTrue(forwarded.isEmpty)
            XCTAssertNotNil(previous.durableAttachCursorForTesting())

            // Equal revision/state is a valid repeat with a different reason
            // and window. Its session cannot clear session-b's checkpoint.
            previous.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: "session-a", turnID: 71, revision: 8, state: .cancelled,
                lastSequence: 90, reason: "replay_history_truncated"
            )))
            XCTAssertEqual(forwarded, ["replay_history_truncated"])
            XCTAssertNotNil(previous.durableAttachCursorForTesting())
        }

        func testSameRevisionReasonsAndWindowsDoNotAdvanceTheReplayCursor() {
            let source = makeSource()
            source.model.activeSessionId = "same-revision"
            guard let token = source.send("question") else { return XCTFail("expected a turn") }
            source.recordDurableSequenceForTesting(turnId: token.clientTurnId, sequence: 1)
            source.recordDurableSequenceForTesting(turnId: token.clientTurnId, sequence: 2)
            source.recordDurableSequenceForTesting(turnId: token.clientTurnId, sequence: 3)
            var reasons: [String?] = []
            source.setExternalEventHandler { event in
                if case let .turnRecoveryState(snapshot) = event { reasons.append(snapshot.reason) }
            }
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: "same-revision", turnID: token.clientTurnId,
                revision: 4, state: .pausedRecoverable, lastSequence: 100, reason: "attach"
            )))
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: "same-revision", turnID: token.clientTurnId,
                revision: 4, state: .pausedRecoverable, lastSequence: 50, reason: "resume"
            )))
            XCTAssertEqual(reasons, ["attach", "resume"])
            XCTAssertEqual(source.durableAttachCursorForTesting(), 3)
        }

        func testOnlyProjectedRetainedEventsAdvanceTheSourceLocalCursor() async {
            let source = makeSource()
            source.model.activeSessionId = "replay-cursor"
            source.setCommandSubmitterForTesting { command in
                switch command {
                case let .attachTurn(_, turnID, _, requestID):
                    source.applyForTesting(.turnAttachStarted(requestId: requestID, snapshot: self.recoverySnapshot(
                        sessionID: "replay-cursor", turnID: turnID, revision: 1, state: .pausedRecoverable
                    )))
                    source.applyForTesting(.turnEventReplay(
                        sessionId: "replay-cursor", turnId: turnID, sequence: 2,
                        eventJson: #"{"type":"text_delta","text":"retained"}"#
                    ))
                    source.applyForTesting(.turnEventReplay(
                        sessionId: "replay-cursor", turnId: turnID, sequence: 1,
                        eventJson: #"{"type":"text_delta","text":"first"}"#
                    ))
                    source.applyForTesting(.turnEventReplay(
                        sessionId: "replay-cursor", turnId: turnID, sequence: 99,
                        eventJson: "invalid JSON"
                    ))
                    source.applyForTesting(.turnEventReplay(
                        sessionId: "replay-cursor", turnId: turnID + 1, sequence: 100,
                        eventJson: #"{"type":"text_delta","text":"wrong turn"}"#
                    ))
                    source.applyForTesting(.turnAttachFinished(requestId: requestID, sessionId: "replay-cursor", turnId: turnID))
                case let .resumeTurn(_, turnID):
                    source.applyForTesting(.turnRecoveryState(snapshot: self.recoverySnapshot(
                        sessionID: "replay-cursor", turnID: turnID, revision: 2, state: .running
                    )))
                default: break
                }
            }
            guard let token = source.send("question") else { return XCTFail("expected a turn") }
            await flushTasks()
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: "replay-cursor", turnID: token.clientTurnId,
                revision: 1, state: .pausedRecoverable, lastSequence: 50
            )))
            source.handleForeground()
            await flushTasks(24)
            XCTAssertEqual(source.durableAttachCursorForTesting(), 2)
            XCTAssertTrue(source.model.messages.contains(where: { $0.text.contains("retained") }))
        }

        func testLiveEnvelopesProjectOnceAndAdvanceOnlyContiguousSequences() {
            let session = "live-cursor-\(UUID().uuidString)"
            func readySource() -> EngineConversationSource {
                let source = makeSource()
                source.beginTurnForTesting(turnId: 71, sessionId: session)
                source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
                return source
            }
            func envelope(_ owner: String, _ turn: UInt64, _ sequence: UInt64, _ text: String) -> ClientEvent {
                .turnEventReplay(sessionId: owner, turnId: turn, sequence: sequence,
                                 eventJson: "{\"type\":\"text_delta\",\"text\":\"\(text)\"}")
            }

            let missingRaw = readySource()
            missingRaw.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 99, eventJson: "invalid"))
            XCTAssertEqual(missingRaw.durableAttachCursorForTesting(), 0)
            XCTAssertNotNil(missingRaw.model.error)

            let unprojected = readySource()
            unprojected.applyForTesting(envelope(session, 71, 1, "only-envelope"))
            XCTAssertEqual(unprojected.durableAttachCursorForTesting(), 1)
            XCTAssertTrue(unprojected.model.messages.contains(where: { $0.text == "only-envelope" }))

            let blocked = readySource()
            blocked.model.streaming = false
            blocked.applyForTesting(envelope(session, 71, 1, "blocked"))
            XCTAssertEqual(blocked.durableAttachCursorForTesting(), 0)
            XCTAssertFalse(blocked.model.messages.contains(where: { $0.text.contains("blocked") }))

            let source = readySource()
            source.applyForTesting(envelope("other-session", 71, 100, "second"))
            source.applyForTesting(envelope(session, 72, 200, "second"))
            source.applyForTesting(envelope(session, 71, 2, "second"))
            XCTAssertEqual(source.durableAttachCursorForTesting(), 0)
            XCTAssertFalse(source.model.messages.contains(where: { $0.text.contains("second") }))
            source.applyForTesting(envelope(session, 71, 1, "first"))
            XCTAssertEqual(source.durableAttachCursorForTesting(), 2)
            XCTAssertTrue(source.model.messages.contains(where: { $0.text == "firstsecond" }))
            source.applyForTesting(envelope(session, 71, 1, "first"))
            XCTAssertTrue(source.model.messages.contains(where: { $0.text == "firstsecond" }))
        }

        func testColdThenWarmAttachAdvancesAcrossHandledTurnBoundaryAndTools() async {
            let source = makeSource()
            let session = "ordered-replay-\(UUID().uuidString)"
            source.model.activeSessionId = session
            var attachCursors: [UInt64?] = []
            var resumeCount = 0
            source.setCommandSubmitterForTesting { command in
                switch command {
                case let .attachTurn(_, turnID, afterSequence, requestID):
                    attachCursors.append(afterSequence)
                    source.applyForTesting(.turnAttachStarted(requestId: requestID, snapshot: self.recoverySnapshot(
                        sessionID: session, turnID: turnID, revision: UInt64(resumeCount * 2 + 1), state: .pausedRecoverable
                    )))
                    if afterSequence == 0 {
                        let retained = [
                            "{\"type\":\"turn_started\",\"turn_id\":\(turnID)}",
                            #"{"type":"text_delta","text":"first"}"#,
                            #"{"type":"tool_use_started","id":"read-1","tool":"Read","input_json":"{}","header":{"verb":"read","label":"Read","title":"Read"}}"#,
                            #"{"type":"tool_use_result","id":"read-1","tool":"Read","result_json":"{}","is_error":false,"display":{"body_lines":0}}"#,
                            #"{"type":"text_delta","text":"last"}"#,
                        ]
                        for (index, json) in retained.enumerated() {
                            source.applyForTesting(.turnEventReplay(
                                sessionId: session, turnId: turnID, sequence: UInt64(index + 1), eventJson: json
                            ))
                        }
                    }
                    source.applyForTesting(.turnAttachFinished(requestId: requestID, sessionId: session, turnId: turnID))
                case let .resumeTurn(_, turnID):
                    resumeCount += 1
                    source.applyForTesting(.turnRecoveryState(snapshot: self.recoverySnapshot(
                        sessionID: session, turnID: turnID, revision: UInt64(resumeCount * 2), state: .running
                    )))
                default: break
                }
            }
            guard let token = source.send("question") else { return XCTFail("expected a turn") }
            await flushTasks()
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: token.clientTurnId, revision: 1, state: .pausedRecoverable
            )))
            source.handleForeground()
            await flushTasks(24)
            XCTAssertEqual(source.durableAttachCursorForTesting(), 5)
            let messages = source.model.messages.map(\.text)
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: token.clientTurnId, revision: 3, state: .pausedRecoverable
            )))
            source.handleForeground()
            await flushTasks(24)
            XCTAssertEqual(attachCursors, [0, 5])
            XCTAssertEqual(source.model.messages.map(\.text), messages)
            XCTAssertNil(source.model.error)
        }

        func testLiveOnlyDeliveryRendersButRequiresReopeningBeforeAttach() async {
            let root = FileManager.default.temporaryDirectory
                .appendingPathComponent(UUID().uuidString, isDirectory: true).path
            let session = "unretained-\(UUID().uuidString)"
            let source = makeSource(appSandboxRoot: root)
            source.beginTurnForTesting(turnId: 71, sessionId: session)
            source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
            source.applyForTesting(.turnEventReplay(
                sessionId: session, turnId: 71, sequence: 1,
                eventJson: #"{"type":"text_delta","text":"retained"}"#
            ))
            source.applyForTesting(.turnEventLiveUnretained(
                sessionId: "other", turnId: 71,
                eventJson: #"{"type":"text_delta","text":"wrong"}"#
            ))
            source.applyForTesting(.turnEventLiveUnretained(
                sessionId: session, turnId: 71,
                eventJson: #"{"type":"text_delta","text":"live-only"}"#
            ))
            XCTAssertEqual(source.durableAttachCursorForTesting(), 1)
            XCTAssertTrue(source.model.messages.contains(where: { $0.text == "retainedlive-only" }))
            XCTAssertNotNil(source.model.error)
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: 71, revision: 1, state: .pausedRecoverable
            )))
            var warmAttachCount = 0
            source.setCommandSubmitterForTesting { command in
                if case .attachTurn = command { warmAttachCount += 1 }
            }
            source.handleForeground()
            await flushTasks()
            XCTAssertEqual(warmAttachCount, 0)

            // Simulates a crash before any extra degradation marker could be
            // saved: a fresh source does not reuse the previous UI cursor.
            let reopened = makeSource(appSandboxRoot: root)
            reopened.beginTurnForTesting(turnId: 71, sessionId: session)
            var attachedAfter: UInt64?
            reopened.setCommandSubmitterForTesting { command in
                if case let .attachTurn(_, _, afterSequence, _) = command { attachedAfter = afterSequence }
            }
            reopened.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: 71, revision: 1, state: .pausedRecoverable
            )))
            reopened.handleForeground()
            await flushTasks(24)
            XCTAssertEqual(attachedAfter, 0)
            reopened.retireRecoveryIngress()
        }

        func testFirstPromptBindsSessionStartedBeforeProjectingItsJournal() async {
            let source = makeSource()
            guard let token = source.send("first prompt") else { return XCTFail("expected a turn") }
            await flushTasks()
            XCTAssertNil(source.durableAttachCursorForTesting())
            source.applyForTesting(.sessionStarted(sessionId: "fresh-session", mode: .code))
            source.applyForTesting(.turnEventReplay(
                sessionId: "fresh-session", turnId: token.clientTurnId, sequence: 1,
                eventJson: "{\"type\":\"turn_started\",\"turn_id\":\(token.clientTurnId)}"
            ))
            source.applyForTesting(.turnEventReplay(
                sessionId: "fresh-session", turnId: token.clientTurnId, sequence: 2,
                eventJson: #"{"type":"text_delta","text":"first answer"}"#
            ))
            XCTAssertEqual(source.durableAttachCursorForTesting(), 2)
            XCTAssertTrue(source.model.messages.contains(where: { $0.text == "first answer" }))
        }

        func testSlashPromotedToTurnCreatesCheckpointButLocalSlashDoesNot() async {
            let source = makeSource()
            source.model.activeSessionId = "slash-session"
            source.model.slashCommandsLoaded = true
            source.model.slashCommands = [ConversationSlashCommand(
                name: "skill", description: "skill", aliases: [], argumentHint: nil,
                menuDescription: nil, source: "builtin", hidden: false
            )]
            guard let token = source.send("/skill") else { return XCTFail("expected slash token") }
            await flushTasks()
            XCTAssertNil(source.durableAttachCursorForTesting())
            source.applyForTesting(.turnEventReplay(
                sessionId: "slash-session", turnId: token.clientTurnId, sequence: 1,
                eventJson: "{\"type\":\"turn_started\",\"turn_id\":\(token.clientTurnId)}"
            ))
            source.applyForTesting(.turnEventReplay(
                sessionId: "slash-session", turnId: token.clientTurnId, sequence: 2,
                eventJson: #"{"type":"text_delta","text":"skill output"}"#
            ))
            XCTAssertEqual(source.durableAttachCursorForTesting(), 2)
            XCTAssertTrue(source.model.messages.contains(where: { $0.text == "skill output" }))
            source.applyForTesting(.turnEventReplay(
                sessionId: "slash-session", turnId: token.clientTurnId, sequence: 3,
                eventJson: #"{"type":"turn_ended","outcome":{"type":"end_turn"},"cost":{"total_usd":0,"input_tokens":0,"output_tokens":0,"api_calls":0,"session_duration_secs":0,"formatted":"$0"}}"#
            ))
            XCTAssertNotNil(source.durableAttachCursorForTesting())
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: "slash-session", turnID: token.clientTurnId, revision: 1, state: .completed
            )))
            XCTAssertNil(source.durableAttachCursorForTesting())

            let local = makeSource()
            local.model.activeSessionId = "local-slash"
            local.model.slashCommandsLoaded = true
            local.model.slashCommands = source.model.slashCommands
            guard let localToken = local.send("/skill") else { return XCTFail("expected local slash token") }
            local.applyForTesting(.slashCommandResult(turnId: localToken.clientTurnId, display: "done".clientTextDto, isError: false))
            XCTAssertNil(local.durableAttachCursorForTesting())
        }

        func testExplicitQueuedPromptAndBashAdoptOnlyTheirOwnNextTurn() async {
            for queuedText in ["queued prompt", "!printf hi"] {
                let source = makeSource()
                let session = "queued-\(UUID().uuidString)"
                source.model.activeSessionId = session
                var submitted: [ClientCommand] = []
                source.setCommandSubmitterForTesting { submitted.append($0) }
                guard let first = source.send("first") else { return XCTFail("expected first turn") }
                XCTAssertNotNil(source.send(queuedText))
                await flushTasks()
                let turns = submitted.compactMap { command -> UInt64? in
                    if case let .sendPrompt(_, _, _, _, turnID, _) = command { return turnID }
                    return nil
                }
                guard turns.count == 2 else { return XCTFail("expected both submitted ids") }
                let queued = turns[1]
                source.applyForTesting(.turnEventReplay(
                    sessionId: session, turnId: first.clientTurnId, sequence: 1,
                    eventJson: "{\"type\":\"turn_started\",\"turn_id\":\(first.clientTurnId)}"
                ))
                source.applyForTesting(.turnEventReplay(
                    sessionId: session, turnId: first.clientTurnId, sequence: 2,
                    eventJson: #"{"type":"turn_ended","outcome":{"type":"end_turn"},"cost":{"total_usd":0,"input_tokens":0,"output_tokens":0,"api_calls":0,"session_duration_secs":0,"formatted":"$0"}}"#
                ))
                source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                    sessionID: session, turnID: first.clientTurnId, revision: 1, state: .completed
                )))
                source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                    sessionID: session, turnID: queued + 100, revision: 1, state: .running
                )))
                XCTAssertFalse(source.model.streaming)
                source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                    sessionID: session, turnID: queued, revision: 1, state: .running
                )))
                source.applyForTesting(.turnEventReplay(
                    sessionId: session, turnId: queued, sequence: 1,
                    eventJson: "{\"type\":\"turn_started\",\"turn_id\":\(queued)}"
                ))
                source.applyForTesting(.turnEventReplay(
                    sessionId: session, turnId: queued, sequence: 2,
                    eventJson: #"{"type":"text_delta","text":"queued output"}"#
                ))
                XCTAssertEqual(source.durableAttachCursorForTesting(), 2)
                XCTAssertTrue(source.model.messages.contains(where: { $0.text == "queued output" }))
                source.applyForTesting(.turnEventReplay(
                    sessionId: session, turnId: queued, sequence: 3,
                    eventJson: #"{"type":"turn_ended","outcome":{"type":"end_turn"},"cost":{"total_usd":0,"input_tokens":0,"output_tokens":0,"api_calls":0,"session_duration_secs":0,"formatted":"$0"}}"#
                ))
                XCTAssertFalse(source.model.streaming)
            }
        }

        func testQueuedTurnWaitsForPriorTerminalAndTimesOutVisibly() async {
            let source = makeSource()
            let session = "queue-reordered-\(UUID().uuidString)"
            source.model.activeSessionId = session
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { commands.append($0) }
            guard let first = source.send("first") else { return XCTFail("expected first") }
            XCTAssertNotNil(source.send("!queued"))
            await flushTasks()
            let turns = commands.compactMap { command -> UInt64? in
                if case let .sendPrompt(_, _, _, _, turnID, _) = command { return turnID }
                return nil
            }
            guard turns.count == 2 else { return XCTFail("expected queued turn") }
            let queued = turns[1]
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: queued, revision: 1, state: .running
            )))
            source.applyForTesting(.turnEventReplay(
                sessionId: session, turnId: queued, sequence: 1,
                eventJson: "{\"type\":\"turn_started\",\"turn_id\":\(queued)}"
            ))
            XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, first.clientTurnId)
            source.applyForTesting(.turnEventReplay(
                sessionId: session, turnId: first.clientTurnId, sequence: 1,
                eventJson: #"{"type":"turn_ended","outcome":{"type":"end_turn"},"cost":{"total_usd":0,"input_tokens":0,"output_tokens":0,"api_calls":0,"session_duration_secs":0,"formatted":"$0"}}"#
            ))
            XCTAssertNil(source.model.activeTurnToken)
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: first.clientTurnId, revision: 1, state: .completed
            )))
            XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, queued)
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: first.clientTurnId, revision: 2, state: .cancelled
            )))
            XCTAssertEqual(source.model.activeTurnToken?.clientTurnId, queued)
            XCTAssertTrue(source.model.streaming)

            let stalled = makeSource()
            stalled.model.activeSessionId = "stalled-\(UUID().uuidString)"
            var stalledCommands: [ClientCommand] = []
            stalled.setCommandSubmitterForTesting { stalledCommands.append($0) }
            guard let owner = stalled.send("owner") else { return XCTFail("expected owner") }
            XCTAssertNotNil(stalled.send("!next"))
            await flushTasks()
            let pending = stalledCommands.compactMap { command -> UInt64? in
                if case let .sendPrompt(_, _, _, _, turnID, _) = command { return turnID }
                return nil
            }.last!
            stalled.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: stalled.model.activeSessionId, turnID: pending, revision: 1, state: .running
            )))
            XCTAssertNotNil(stalled.recoveryWatermarkForTesting(sessionID: stalled.model.activeSessionId, turnID: pending))
            stalled.expireDeferredQueuedTurnsForTesting()
            XCTAssertNotNil(stalled.recoveryWatermarkForTesting(sessionID: stalled.model.activeSessionId, turnID: pending))
            XCTAssertEqual(try? stalled.pendingSubmissionsForTesting().first?.projectionIncomplete, true)
            XCTAssertNotNil(stalled.model.error)
            XCTAssertEqual(stalled.model.activeTurnToken?.clientTurnId, owner.clientTurnId)
        }

        func testQueuedSubmissionKeepsAuthorityWhenPreviousTurnEndsDuringSubmit() async {
            let source = makeSource()
            let session = "late-submit-\(UUID().uuidString)"
            source.model.activeSessionId = session
            var submitted: [UInt64] = []
            var release: CheckedContinuation<Void, Never>?
            source.setCommandSubmitterForTesting { command in
                guard case let .sendPrompt(_, _, _, _, turnID?, _) = command else { return }
                submitted.append(turnID)
                if submitted.count == 2 {
                    await withCheckedContinuation { release = $0 }
                }
            }
            guard let first = source.send("first") else { return XCTFail("expected first") }
            XCTAssertNotNil(source.send("queued"))
            await flushTasks()
            guard submitted.count == 2 else { return XCTFail("expected queued submit") }
            source.applyForTesting(.turnEventReplay(
                sessionId: session, turnId: first.clientTurnId, sequence: 1,
                eventJson: #"{"type":"turn_ended","outcome":{"type":"end_turn"},"cost":{"total_usd":0,"input_tokens":0,"output_tokens":0,"api_calls":0,"session_duration_secs":0,"formatted":"$0"}}"#
            ))
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: first.clientTurnId, revision: 1, state: .completed
            )))
            XCTAssertTrue(source.maySubmitPromptForTesting(owner: session, turnID: submitted[1]))
            release?.resume()
            await flushTasks()
            XCTAssertNil(source.model.error)
        }

        func testUnretainedCrashUsesPersistentUncertaintyOnColdAttach() async {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).path
            let session = "uncertain-\(UUID().uuidString)"
            let prior = makeSource(appSandboxRoot: root)
            prior.beginTurnForTesting(turnId: 71, sessionId: session)
            prior.beginDurableTurnForTesting(turnId: 71, sessionId: session)
            prior.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
                eventJson: #"{"type":"turn_started","turn_id":71}"#))
            prior.applyForTesting(.turnEventLiveUnretained(sessionId: session, turnId: 71,
                eventJson: #"{"type":"text_delta","text":"visible before crash"}"#))
            XCTAssertTrue(prior.model.messages.contains { $0.text == "visible before crash" })
            let reopened = makeSource(appSandboxRoot: root)
            reopened.setCommandSubmitterForTesting { command in
                if case let .attachTurn(_, _, _, requestID) = command {
                    reopened.applyForTesting(.turnAttachStarted(requestId: requestID, snapshot: self.recoverySnapshot(
                        sessionID: session, turnID: 71, revision: 2, state: .pausedRecoverable,
                        lastSequence: 1, projectionUncertain: true
                    )))
                    reopened.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
                        eventJson: #"{"type":"turn_started","turn_id":71}"#))
                    reopened.applyForTesting(.turnAttachFinished(requestId: requestID, sessionId: session, turnId: 71))
                }
            }
            reopened.expectSessionResumeForTesting(session)
            reopened.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: [
                MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "authoritative".clientTextDto)])
            ]))
            await flushTasks(24)
            XCTAssertNotNil(reopened.model.error)
            XCTAssertEqual(reopened.model.messages.map(\.text), ["authoritative"])
            XCTAssertEqual(reopened.durableAttachCursorForTesting(), 1)
        }

        func testHealthyLiveProjectionUncertaintyDoesNotShowWarning() {
            let source = makeSource()
            let session = "live-uncertain-\(UUID().uuidString)"
            source.beginTurnForTesting(turnId: 71, sessionId: session)
            source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: 71, revision: 1, state: .running, projectionUncertain: true
            )))
            source.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
                eventJson: #"{"type":"text_delta","text":"live"}"#))
            XCTAssertNil(source.model.error)
            XCTAssertTrue(source.model.messages.contains { $0.text == "live" })
        }

        func testRetiredSourceCannotMutateSharedCheckpointOrRevision() {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).path
            let session = "retired-\(UUID().uuidString)"
            let old = makeSource(appSandboxRoot: root)
            old.beginTurnForTesting(turnId: 71, sessionId: session)
            old.beginDurableTurnForTesting(turnId: 71, sessionId: session)
            var observed = 0
            old.setExternalEventHandler { _ in observed += 1 }
            old.retireRecoveryIngress()
            old.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: 71, revision: 9, state: .cancelled
            )))
            old.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
                eventJson: #"{"type":"text_delta","text":"stale"}"#))
            XCTAssertEqual(observed, 0)
            XCTAssertEqual(old.durableAttachCursorForTesting(), 0)

            let replacement = makeSource(appSandboxRoot: root)
            replacement.beginTurnForTesting(turnId: 71, sessionId: session)
            replacement.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: 71, revision: 1, state: .running
            )))
            replacement.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
                eventJson: #"{"type":"text_delta","text":"current"}"#))
            XCTAssertTrue(replacement.model.messages.contains { $0.text == "current" })
            XCTAssertEqual(replacement.durableAttachCursorForTesting(), 1)
        }

        func testDurableCheckpointsPartitionScopesAndMigrateMatchingLegacyRecord() throws {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            func config(_ cwd: String) -> EngineConfig {
                EngineConfig(apiBase: "https://example.invalid", apiKey: "", model: "", appSandboxRoot: root.path,
                             projectCwd: cwd, sessionMode: .code, visionDelegationEnabled: false)
            }
            let legacyURL = root.appendingPathComponent("durable-conversation-turn.json")
            try JSONEncoder().encode(DurableConversationTurnRecord(
                scope: "/project-a", sessionID: "legacy-a", turnID: 11, lastSequence: 0
            )).write(to: legacyURL, options: .atomic)
            let a = DurableConversationTurnClientStore(config: config("/project-a"))
            let b = DurableConversationTurnClientStore(config: config("/project-b"))
            b.begin(sessionID: "session-b", turnID: 22)
            XCTAssertTrue(FileManager.default.fileExists(atPath: legacyURL.path))
            XCTAssertEqual(a.load()?.sessionID, "legacy-a")
            XCTAssertEqual(b.load()?.sessionID, "session-b")
            let stores = [a, b]
            DispatchQueue.concurrentPerform(iterations: 2) { index in
                stores[index].begin(sessionID: index == 0 ? "new-a" : "new-b", turnID: UInt64(31 + index))
            }
            let reopenedA = DurableConversationTurnClientStore(config: config("/project-a"))
            let reopenedB = DurableConversationTurnClientStore(config: config("/project-b"))
            XCTAssertEqual(reopenedA.load()?.sessionID, "new-a")
            XCTAssertEqual(reopenedB.load()?.sessionID, "new-b")
            reopenedA.clear(turnID: 31, sessionID: "new-a")
            XCTAssertNil(reopenedA.load())
            XCTAssertEqual(reopenedB.load()?.sessionID, "new-b")
        }

        func testDetachedModNotificationIsSessionScopedAndIndependentOfRun() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 71, sessionId: "session-b")
            let initial = source.model.items
            source.applyForTesting(.modSessionEvent(sessionId: "session-a", modTurnId: "opaque", payload:
                .notice(message: "old".clientTextDto, isError: false)))
            XCTAssertEqual(source.model.items, initial)
            source.applyForTesting(.modSessionEvent(sessionId: "session-b", modTurnId: "opaque", payload:
                .notify(plugin: "plugin", text: "hello".clientTextDto)))
            XCTAssertTrue(source.model.items.contains { item in
                if case let .notice(notice) = item { return notice.text == "plugin: hello" }
                return false
            })
        }

        func testCancelledSnapshotKeepsOwnerUntilLateBlockResultIsProjected() async {
            let source = makeSource()
            let session = "stop-block-\(UUID().uuidString)"
            source.beginTurnForTesting(turnId: 71, sessionId: session)
            source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
            source.applyForTesting(.turnEventReplay(sessionId: session, turnId: 71, sequence: 1,
                eventJson: #"{"type":"tool_use_started","id":"block-1","tool":"Read","input_json":"{}","header":{"verb":"read","label":"Read","title":"Read"}}"#))
            var release: CheckedContinuation<Void, Never>?
            source.setCommandSubmitterForTesting { command in
                if case .cancel = command { await withCheckedContinuation { release = $0 } }
            }
            let stopping = Task { try? await source.cancelAndWait() }
            await flushTasks()
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: 71, revision: 2, state: .cancelled
            )))
            source.applyForTesting(.turnEventLiveUnretained(sessionId: session, turnId: 71,
                eventJson: #"{"type":"tool_use_result","id":"block-1","tool":"Read","result_json":"{}","is_error":false,"display":{"body_lines":0}}"#))
            guard case let .run(run)? = source.model.items.first(where: { if case .run = $0 { return true }; return false }) else {
                release?.resume()
                return XCTFail("expected original run")
            }
            XCTAssertEqual(run.tools.first(where: { $0.id == "block-1" })?.status, .completed)
            XCTAssertTrue(source.model.isCancelling)
            XCTAssertEqual(source.model.error?.message, String(localized: "chat_recovery_projection_uncertain"))
            release?.resume()
            await stopping.value
            XCTAssertFalse(source.model.streaming)
            XCTAssertEqual(source.model.notice, .cancelled)
        }

        func testColdRestoreDoesNotAppendAnUnprovenOverlappingJournal() async {
            let root = FileManager.default.temporaryDirectory
                .appendingPathComponent(UUID().uuidString, isDirectory: true).path
            let session = "cold-overlap-\(UUID().uuidString)"
            let previous = makeSource(appSandboxRoot: root)
            previous.beginDurableTurnForTesting(turnId: 71, sessionId: session)
            let reopened = makeSource(appSandboxRoot: root)
            reopened.setCommandSubmitterForTesting { command in
                switch command {
                case let .attachTurn(_, _, _, requestID):
                    reopened.applyForTesting(.turnAttachStarted(requestId: requestID, snapshot: self.recoverySnapshot(
                        sessionID: session, turnID: 71, revision: 1, state: .pausedRecoverable
                    )))
                    reopened.applyForTesting(.turnEventReplay(
                        sessionId: session, turnId: 71, sequence: 1,
                        eventJson: #"{"type":"text_delta","text":"already restored"}"#
                    ))
                    reopened.applyForTesting(.turnAttachFinished(requestId: requestID, sessionId: session, turnId: 71))
                case .resumeTurn:
                    reopened.applyForTesting(.turnRecoveryState(snapshot: self.recoverySnapshot(
                        sessionID: session, turnID: 71, revision: 2, state: .running
                    )))
                default: break
                }
            }
            reopened.expectSessionResumeForTesting(session)
            reopened.applyForTesting(.sessionResumed(
                sessionId: session, mode: .code,
                messages: [MessageDto(transcriptUuid: nil, role: "assistant", blocks: [
                    .text(text: "already restored".clientTextDto),
                ])]
            ))
            await flushTasks(24)
            XCTAssertEqual(reopened.model.messages.map(\.text), ["already restored"])
            XCTAssertEqual(reopened.durableAttachCursorForTesting(), 0)
            XCTAssertNotNil(reopened.model.error)
        }

        func testTerminalEnvelopeKeepsIdentityForFollowingRecoveryState() {
            let source = makeSource()
            let session = "terminal-envelope-\(UUID().uuidString)"
            source.beginTurnForTesting(turnId: 71, sessionId: session)
            source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
            var recoveryStates: [TurnRecoveryStateDto] = []
            source.setExternalEventHandler { event in
                if case let .turnRecoveryState(snapshot) = event { recoveryStates.append(snapshot.state) }
            }
            source.applyForTesting(.turnEventReplay(
                sessionId: session, turnId: 71, sequence: 1,
                eventJson: #"{"type":"turn_ended","outcome":{"type":"end_turn"},"cost":{"total_usd":0,"input_tokens":0,"output_tokens":0,"api_calls":0,"session_duration_secs":0,"formatted":"$0"}}"#
            ))
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: 71, revision: 2, state: .completed
            )))
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: 71, revision: 1, state: .pausedRecoverable
            )))
            XCTAssertEqual(recoveryStates, [.completed])
            XCTAssertFalse(source.model.streaming)
            XCTAssertFalse(source.model.hasUnresolvedTurnRecovery)
        }

        func testReplayOverflowShowsErrorAndReopeningCanAttachAgain() async {
            let root = FileManager.default.temporaryDirectory
                .appendingPathComponent(UUID().uuidString, isDirectory: true).path
            let session = "overflow-\(UUID().uuidString)"
            let source = makeSource(appSandboxRoot: root)
            source.beginTurnForTesting(turnId: 71, sessionId: session)
            source.beginDurableTurnForTesting(turnId: 71, sessionId: session)
            for sequence in 2...514 {
                source.applyForTesting(.turnEventReplay(
                    sessionId: session, turnId: 71, sequence: UInt64(sequence),
                    eventJson: #"{"type":"text_delta","text":"x"}"#
                ))
            }
            XCTAssertNotNil(source.model.error)
            source.applyForTesting(.turnEventReplay(
                sessionId: session, turnId: 71, sequence: 1,
                eventJson: #"{"type":"text_delta","text":"x"}"#
            ))
            XCTAssertEqual(source.durableAttachCursorForTesting(), 0)

            let reopened = makeSource(appSandboxRoot: root)
            reopened.beginTurnForTesting(turnId: 71, sessionId: session)
            var attachedAfter: UInt64?
            reopened.setCommandSubmitterForTesting { command in
                if case let .attachTurn(_, _, afterSequence, _) = command { attachedAfter = afterSequence }
            }
            reopened.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: 71, revision: 1, state: .pausedRecoverable
            )))
            reopened.handleForeground()
            await flushTasks(24)
            XCTAssertEqual(attachedAfter, 0)

            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: session, turnID: 71, revision: 2, state: .cancelled
            )))
            XCTAssertEqual(source.model.notice, .cancelled)
            XCTAssertNotNil(source.model.error, "terminal state must not erase the replay-gap warning")
            reopened.retireRecoveryIngress()
        }

        func testRecoveryVersionKeysIncludeSessionAndTurn() {
            let source = makeSource()
            var forwarded = 0
            source.setExternalEventHandler { event in
                if case .turnRecoveryState = event { forwarded += 1 }
            }
            source.beginTurnForTesting(turnId: 71, sessionId: "session-a")
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: "session-a", turnID: 71, revision: 9, state: .cancelled
            )))
            source.beginTurnForTesting(turnId: 71, sessionId: "session-b")
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: "session-b", turnID: 71, revision: 1, state: .pausedRecoverable
            )))
            source.beginTurnForTesting(turnId: 72, sessionId: "session-a")
            source.applyForTesting(.turnRecoveryState(snapshot: recoverySnapshot(
                sessionID: "session-a", turnID: 72, revision: 1, state: .pausedRecoverable
            )))
            XCTAssertEqual(forwarded, 3)
        }

        func testEngineListenerPreservesFIFOEventOrder() async {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 900, sessionId: "test-session")
            let listener = source.makeEventListenerForTesting()

            listener.enqueueForTesting(.textDelta(text: "before tool".clientTextDto))
            listener.enqueueForTesting(.toolUseStarted(
                id: "ordered-tool",
                tool: "Read",
                inputJson: #"{"path":"a"}"#,
                header: currentToolHeader()
            ))
            listener.enqueueForTesting(.textDelta(text: "after tool".clientTextDto))
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
                listener.enqueueForTesting(.textDelta(text: delta.clientTextDto))
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

            source.applyForTesting(.textDelta(text: "same".clientTextDto))
            XCTAssertEqual(updates.last, ConversationTurnSpeechUpdate(
                token: token,
                sequence: 1,
                delta: "same"
            ))

            source.applyForTesting(.textDelta(text: "same".clientTextDto))
            XCTAssertEqual(updates.map(\.sequence), [1, 2])
            XCTAssertEqual(updates.map(\.delta), ["same", "same"])
            withExtendedLifetime(subscription) {}
        }

        func testSessionSwitchDropsLateSpeechDelta() {
            let source = makeSource()
            var updates: [ConversationTurnSpeechUpdate] = []
            let subscription = source.model.turnSpeechUpdates.sink { updates.append($0) }
            XCTAssertNotNil(source.send("old question"))
            source.applyForTesting(.textDelta(text: "old".clientTextDto))
            XCTAssertEqual(updates.count, 1)

            source.expectSessionResumeForTesting("new-session")
            source.applyForTesting(.sessionResumed(sessionId: "new-session", mode: .code, messages: []))
            source.applyForTesting(.textDelta(text: "late".clientTextDto))
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
                    message: MessageDto(transcriptUuid: nil, role: "assistant", blocks: [
                        .thinking(thinking: "private reasoning", signature: nil),
                        .toolUse(id: "tool-1", tool: "Read", inputJson: #"{"path":"/tmp/a"}"#, header: currentToolHeader()),
                        .text(text: "  spoken answer  ".clientTextDto),
                        .toolResult(
                            id: "tool-1",
                            tool: "Read",
                            resultJson: #"{"result":"secret tool output"}"#,
                            isError: false,
                            display: currentToolDisplay()
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
            source.applyForTesting(.textDelta(text: "partial response".clientTextDto))
            source.applyForTesting(.error(kind: .transport, message: "offline".clientTextDto))

            XCTAssertEqual(source.model.turnCompletion?.token, token)
            XCTAssertEqual(source.model.turnCompletion?.outcome, .failed)
            XCTAssertEqual(source.model.turnCompletion?.finalAssistantText, "")
        }

        func testSessionSwitchInvalidatesTokenAndDropsLateTerminal() {
            let source = makeSource()

            guard let oldToken = source.send("old question") else {
                return XCTFail("expected a turn token")
            }
            source.applyForTesting(.textDelta(text: "stale answer".clientTextDto))
            source.expectSessionResumeForTesting("new-session")
            source.applyForTesting(.sessionResumed(sessionId: "new-session", mode: .code, messages: []))
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
            source.applyForTesting(.textDelta(text: "partial response".clientTextDto))
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
            source.applyForTesting(.sessionResumed(sessionId: "new-session", mode: .code, messages: []))
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
