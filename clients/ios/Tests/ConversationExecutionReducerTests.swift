import XCTest

@testable import LingxiCode

#if canImport(engine_mobileFFI)
    import engine_mobileFFI
#endif

#if canImport(engine_mobileFFI)

    @MainActor
    final class ConversationExecutionReducerTests: XCTestCase {
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
                projectCwd: nil)
            return EngineConversationSource(config: config)
        }

        private func flushTasks(_ count: Int = 4) async {
            for _ in 0..<count { await Task.yield() }
        }

        private func waitForSubmittedCommands(
            _ expectedCount: Int,
            commands: @autoclosure () -> [ClientCommand]
        ) async {
            for _ in 0..<50 where commands().count < expectedCount {
                await Task.yield()
            }
        }

        func testShellLifecycleBuildsStructuredRunCard() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 7, sessionId: "session-a")

            source.applyForTesting(.turnStarted(turnId: 7))
            source.applyForTesting(
                .toolUseStarted(
                    id: "task-1",
                    tool: "bash",
                    inputJson: #"{"command":"echo ok","cwd":"/workspace"}"#
                ))
            source.applyForTesting(.toolHeartbeat(id: "task-1", tool: "bash", elapsedMs: 320))
            source.applyForTesting(
                .toolUseResult(
                    id: "task-1",
                    tool: "bash",
                    resultJson: #"{"data":{"stdout":"ok\n","stderr":"","exit_code":0,"duration_ms":480,"truncated":false}}"#,
                    isError: false
                ))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected a run render item")
            }

            XCTAssertEqual(run.status, .running, "tool events do not end the turn")
            XCTAssertEqual(run.shellCards.count, 1)
            XCTAssertEqual(run.shellCards[0].command, "echo ok")
            XCTAssertEqual(run.shellCards[0].cwd, "/workspace")
            XCTAssertEqual(run.shellCards[0].stdout, "ok\n")
            XCTAssertEqual(run.shellCards[0].durationMs, 480)
            XCTAssertEqual(run.shellCards[0].status, .completed)
            XCTAssertEqual(run.tools.first?.tool, "Shell")
            XCTAssertEqual(run.tools.first?.status, .completed)
            XCTAssertEqual(source.model.statusLine, "Shell 完成")
        }

        func testSettledDurableTurnRequestsAuthoritativeSessionRefresh() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 31, sessionId: "session-a")

            XCTAssertEqual(source.model.sessionRefreshRevision, 0)
            source.applyForTesting(.turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            XCTAssertEqual(source.model.sessionRefreshRevision, 1)
            XCTAssertFalse(source.model.streaming)
        }

        func testPendingSessionTransitionDoesNotRequestPrematureCatalogRefresh() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 32, sessionId: "session-a")
            source.model.sessionTransitionPending = true

            source.applyForTesting(.turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            XCTAssertEqual(source.model.sessionRefreshRevision, 0)
        }

        func testTelemetryAndCoordinatorEventsAccumulateOnActiveRun() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 3, sessionId: "session-a")

            source.applyForTesting(.turnStarted(turnId: 3))
            source.applyForTesting(.thinkingDelta(thinking: "reasoning", signature: nil))
            source.applyForTesting(.systemNotice(message: "still working", isError: false))
            source.applyForTesting(
                .usageUpdate(inputTokens: 11, outputTokens: 22, cacheReadTokens: 3, cacheCreationTokens: 4))
            source.applyForTesting(.apiRetry(message: "429", attempt: 2, maxRetries: 5, delayMs: 1200))
            source.applyForTesting(.costUpdate(
                totalUsd: 0.12, inputTokens: 11, outputTokens: 22, apiCalls: 1, sessionDurationSecs: 9, formatted: "$0.12"))
            source.applyForTesting(.compactionCompleted(messagesBefore: 20, messagesAfter: 7, bytesSaved: 4096))
            source.applyForTesting(.coordinatorStatus(activeWorkers: 2, team: "triage"))
            source.applyForTesting(
                .coordinatorWorker(
                    worker: CoordinatorWorkerDto(agentId: "w1", name: "worker-1", agentType: "executor", status: "working")))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected a run render item")
            }

            XCTAssertEqual(run.reasoning, "reasoning")
            XCTAssertEqual(run.notices.last?.text, "still working")
            XCTAssertEqual(run.usage, ConversationUsageSnapshot(inputTokens: 11, outputTokens: 22, cacheReadTokens: 3, cacheCreationTokens: 4))
            XCTAssertEqual(run.retry, ConversationRetrySnapshot(message: "429", attempt: 2, maxRetries: 5, delayMs: 1200))
            XCTAssertEqual(run.costFormatted, "$0.12")
            XCTAssertEqual(run.compactions.first?.messagesAfter, 7)
            XCTAssertEqual(run.activeWorkers, 2)
            XCTAssertEqual(run.coordinatorTeam, "triage")
            XCTAssertEqual(run.workers.first?.name, "worker-1")
        }

        func testMessageCompleteStoresStructuredBlocks() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 9, sessionId: "session-a")

            source.applyForTesting(.turnStarted(turnId: 9))
            source.applyForTesting(.textDelta(text: "draft"))
            source.applyForTesting(.messageComplete(
                stopReason: "end_turn",
                message: MessageDto(role: "assistant", blocks: [
                    .text(text: "正文"),
                    .thinking(thinking: "推理", signature: nil),
                    .toolUse(id: "t1", tool: "Read", inputJson: #"{"path":"/tmp/a"}"#),
                    .toolResult(id: "t1", tool: "Read", resultJson: #"{"result":"ok"}"#, isError: false, oldString: nil, newString: nil, filePath: nil),
                ])))

            XCTAssertEqual(source.model.messages.count, 1)
            let message = source.model.messages[0]
            let detail = source.model.messageDetails[message.id]

            XCTAssertNotNil(detail)
            XCTAssertEqual(detail?.blocks.count, 4)
            XCTAssertTrue(message.text.contains("正文"))
            XCTAssertTrue(message.text.contains("推理"))
            if case let .toolUse(_, tool, inputSummary, _)? = detail?.blocks[2] {
                XCTAssertEqual(tool, "Read")
                XCTAssertEqual(inputSummary, "/tmp/a")
            } else {
                XCTFail("expected structured tool use block")
            }
        }

        func testStreamingMessageKeepsStableIdentityAcrossUpdates() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 10, sessionId: "session-a")

            source.applyForTesting(.textDelta(text: "Hel"))
            let initialMessageID = source.model.messages[0].id
            let initialRenderID = source.model.items[0].id

            source.applyForTesting(.textDelta(text: "lo"))

            XCTAssertEqual(source.model.messages[0].id, initialMessageID)
            XCTAssertEqual(source.model.items[0].id, initialRenderID)
            XCTAssertEqual(source.model.messages[0].text, "Hello")

            source.applyForTesting(.messageComplete(
                stopReason: "end_turn",
                message: MessageDto(role: "assistant", blocks: [.text(text: "Hello!")])
            ))

            XCTAssertEqual(source.model.messages[0].id, initialMessageID)
            XCTAssertEqual(source.model.items[0].id, initialRenderID)
            XCTAssertEqual(source.model.messages[0].text, "Hello!")
        }

        func testLateEventsDroppedAfterSessionResume() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 4, sessionId: "old-session")

            source.applyForTesting(.turnStarted(turnId: 4))
            source.applyForTesting(
                .toolUseStarted(
                    id: "task-2",
                    tool: "bash",
                    inputJson: #"{"command":"pwd"}"#
                ))
            source.applyForTesting(.sessionResumed(sessionId: "new-session", messages: []))
            source.applyForTesting(
                .toolUseResult(
                    id: "task-2",
                    tool: "bash",
                    resultJson: #"{"data":{"stdout":"stale","stderr":"","exit_code":0}}"#,
                    isError: false
                ))

            XCTAssertTrue(source.model.messages.isEmpty)
            XCTAssertFalse(source.model.items.contains {
                if case let .run(run) = $0 {
                    return run.shellCards.contains(where: { $0.stdout == "stale" })
                }
                return false
            })
        }

        func testCancellationKeepsTurnOwnedAndDraftSendBlockedUntilHostReturns() async {
            let source = makeSource()
            var submittedCommands: [ClientCommand] = []
            var releaseCancellation: CheckedContinuation<Void, Never>?
            source.setCommandSubmitterForTesting { command in
                await MainActor.run {
                    submittedCommands.append(command)
                }
                if case .cancel = command {
                    await withCheckedContinuation { continuation in
                        releaseCancellation = continuation
                    }
                }
            }

            source.beginTurnForTesting(turnId: 1, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 1))
            source.applyForTesting(.toolUseStarted(
                id: "tool-a",
                tool: "WebSearch",
                inputJson: #"{"query":"weather"}"#
            ))
            source.cancel()
            await flushTasks()

            source.send("B prompt")
            await flushTasks()

            XCTAssertTrue(source.model.isCancelling)
            XCTAssertTrue(source.model.streaming, "the current owner remains live while Cancel awaits Block tools")
            XCTAssertEqual(source.model.statusLine, "正在停止…")
            XCTAssertEqual(submittedCommands.count, 1, "the next prompt must stay local while Cancel is pending")
            if case let .cancel(turnId)? = submittedCommands.first {
                XCTAssertEqual(turnId, 1)
            } else {
                XCTFail("expected the matching cancel submission")
            }
            XCTAssertTrue(source.model.messages.isEmpty, "a blocked send must not append or consume the draft")

            source.applyForTesting(.toolHeartbeat(id: "tool-a", tool: "WebSearch", elapsedMs: 2_000))
            guard case let .run(stoppingRun)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                releaseCancellation?.resume()
                return XCTFail("expected the active run")
            }
            XCTAssertEqual(stoppingRun.tools.first?.status, .running)
            XCTAssertEqual(stoppingRun.tools.first?.elapsedMs, 2_000, "heartbeat must keep advancing during safe cancellation")

            source.applyForTesting(.turnEnded(outcome: .cancelled, stopReason: nil, cost: zeroCost))
            await flushTasks()
            XCTAssertFalse(source.model.streaming, "terminal may settle the visible run")
            XCTAssertTrue(source.model.isCancelling, "terminal must not release send before Cancel FFI returns")

            releaseCancellation?.resume()
            releaseCancellation = nil
            await flushTasks(8)

            XCTAssertFalse(source.model.isCancelling)
            source.send("B prompt")
            await flushTasks()

            XCTAssertEqual(submittedCommands.count, 2, "B should submit exactly once after cancellation returns")
            if case let .sendPrompt(text, _, _, turnId)? = submittedCommands.last {
                XCTAssertEqual(text, "B prompt")
                XCTAssertEqual(turnId, 2)
            } else {
                XCTFail("expected B sendPrompt after cancellation completion")
            }
            XCTAssertEqual(source.model.messages.last?.text, "B prompt")
            XCTAssertTrue(source.model.streaming)
        }

        func testCancellingTurnKeepsRowsLiveUntilTerminalThenClosesThem() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 8, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 8))
            source.applyForTesting(.toolUseStarted(
                id: "search-1",
                tool: "WebSearch",
                inputJson: #"{"query":"Wuhan weather today"}"#
            ))
            source.applyForTesting(.toolUseStarted(
                id: "shell-1",
                tool: "Bash",
                inputJson: #"{"command":"sleep 30"}"#
            ))

            source.cancelForTesting()
            source.applyForTesting(.toolHeartbeat(id: "search-1", tool: "WebSearch", elapsedMs: 3_000))

            guard case let .run(stoppingRun)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected a run render item")
            }
            XCTAssertEqual(stoppingRun.status, .running)
            XCTAssertEqual(stoppingRun.tools.map(\.status), [.running, .running])
            XCTAssertEqual(stoppingRun.tools.first?.elapsedMs, 3_000)
            XCTAssertEqual(stoppingRun.shellCards.first?.status, .running)

            source.applyForTesting(.turnEnded(outcome: .cancelled, stopReason: "cancelled", cost: zeroCost))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected the settled run")
            }
            XCTAssertEqual(run.status, .cancelled)
            XCTAssertEqual(run.tools.map(\.status), [.cancelled, .cancelled])
            XCTAssertEqual(run.shellCards.first?.status, .cancelled)
            XCTAssertFalse(run.tools.contains(where: { $0.status == .running }))
            XCTAssertFalse(run.shellCards.contains(where: { $0.status == .running }))
        }

        func testCancellationFailureRestoresRetryableStopState() async {
            enum CancelFailure: Error { case rejected }

            let source = makeSource()
            var cancelAttempts = 0
            source.setCommandSubmitterForTesting { command in
                if case .cancel = command {
                    await MainActor.run { cancelAttempts += 1 }
                    throw CancelFailure.rejected
                }
            }
            source.beginTurnForTesting(turnId: 44, sessionId: "session-a")

            source.cancel()
            await flushTasks(8)

            XCTAssertFalse(source.model.isCancelling)
            XCTAssertTrue(source.model.streaming, "failed delivery must preserve the original turn owner")
            XCTAssertEqual(source.model.error?.kind, .host)
            XCTAssertEqual(cancelAttempts, 1)

            source.cancel()
            await flushTasks(8)
            XCTAssertEqual(cancelAttempts, 2, "Stop must become retryable after a Cancel submission failure")
        }

        func testCancellationFailureRestoresParkedPermission() async {
            enum CancelFailure: Error { case rejected }

            let source = makeSource()
            let permission = PendingPermission(request: PermissionRequest(
                requestId: 91,
                kind: .toolUseConfirm(
                    toolName: "Bash",
                    toolInputJson: #"{"command":"pwd"}"#,
                    defaultAllow: false
                ),
                worker: nil
            ))
            source.setCommandSubmitterForTesting { command in
                if case .cancel = command { throw CancelFailure.rejected }
            }
            source.beginTurnForTesting(turnId: 46, sessionId: "session-a")
            source.model.pendingPermissions = [permission]

            source.cancel()
            await flushTasks(8)

            XCTAssertFalse(source.model.isCancelling)
            XCTAssertTrue(source.model.streaming)
            XCTAssertEqual(source.model.pendingPermissions, [permission])
        }

        func testTerminalBeforeFailedCancellationClearsStoppingStatus() async {
            enum CancelFailure: Error { case rejected }

            let source = makeSource()
            var releaseCancellation: CheckedContinuation<Void, Never>?
            source.setCommandSubmitterForTesting { command in
                guard case .cancel = command else { return }
                await withCheckedContinuation { continuation in
                    releaseCancellation = continuation
                }
                throw CancelFailure.rejected
            }
            source.beginTurnForTesting(turnId: 47, sessionId: "session-a")

            source.cancel()
            await flushTasks()
            source.applyForTesting(.turnEnded(
                outcome: .cancelled,
                stopReason: "cancelled",
                cost: zeroCost
            ))
            releaseCancellation?.resume()
            releaseCancellation = nil
            await flushTasks(8)

            XCTAssertFalse(source.model.isCancelling)
            XCTAssertFalse(source.model.streaming)
            XCTAssertNil(source.model.statusLine)
        }

        func testSessionSwitchCancellationFailureKeepsOriginalTurnRetryable() async {
            enum CancelFailure: Error { case rejected }

            let source = makeSource()
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { commands.append(command) }
                if case .cancel = command { throw CancelFailure.rejected }
            }
            source.beginTurnForTesting(turnId: 45, sessionId: "session-a")
            source.applyForTesting(.textDelta(text: "still owned"))
            let originalItems = source.model.items

            source.resumeSession("session-b")
            await flushTasks(12)

            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertFalse(source.model.isCancelling)
            XCTAssertTrue(source.model.streaming)
            XCTAssertEqual(source.model.items, originalItems)
            XCTAssertEqual(commands.count, 1)
            guard let first = commands.first else { return }
            guard case let .cancel(turnId) = first else {
                return XCTFail("a failed cancellation must not submit ResumeSession")
            }
            XCTAssertEqual(turnId, 45)

            source.cancel()
            await flushTasks(8)
            XCTAssertEqual(commands.count, 2, "the same visible turn must expose a retryable Stop")
        }

        func testInterruptionMetadataMarksOnlyInterruptedToolCancelled() {
            XCTAssertTrue(ConversationExecutionParsing.isCancellationResult(
                #"{"error":"interrupted","tool_denial_kind":"interrupted"}"#
            ))
            XCTAssertFalse(ConversationExecutionParsing.isCancellationResult(
                #"{"error":"network unavailable"}"#
            ))
        }

        func testResumeSerializesCancelBeforeSessionTransition() async {
            let source = makeSource()
            var submittedCommands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { submittedCommands.append(command) }
            }
            source.beginTurnForTesting(turnId: 17, sessionId: "session-a")

            source.resumeSession("session-b")
            await waitForSubmittedCommands(2, commands: submittedCommands)

            XCTAssertEqual(submittedCommands.count, 2)
            guard submittedCommands.count == 2 else { return }
            if case let .cancel(turnId) = submittedCommands[0] {
                XCTAssertEqual(turnId, 17)
            } else {
                XCTFail("the old turn must be cancelled first")
            }
            if case let .resumeSession(sessionId, cwd) = submittedCommands[1] {
                XCTAssertEqual(sessionId, "session-b")
                XCTAssertNil(cwd)
            } else {
                XCTFail("resume must follow the awaited cancel")
            }
        }

        func testRapidResumeSubmitsOnlyLatestSessionAfterSharedCancellation() async {
            let source = makeSource()
            var submittedCommands: [ClientCommand] = []
            var releaseCancellation: CheckedContinuation<Void, Never>?
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { submittedCommands.append(command) }
                if case .cancel = command {
                    await withCheckedContinuation { continuation in
                        releaseCancellation = continuation
                    }
                }
            }
            source.beginTurnForTesting(turnId: 18, sessionId: "session-a")

            source.resumeSession("session-b")
            await waitForSubmittedCommands(1, commands: submittedCommands)
            source.resumeSession("session-c")
            await flushTasks()
            releaseCancellation?.resume()
            releaseCancellation = nil
            await waitForSubmittedCommands(2, commands: submittedCommands)
            await flushTasks(8)

            XCTAssertEqual(submittedCommands.count, 2)
            guard submittedCommands.count == 2 else { return }
            if case let .cancel(turnId) = submittedCommands[0] {
                XCTAssertEqual(turnId, 18)
            } else {
                XCTFail("the shared cancellation must be submitted first")
            }
            if case let .resumeSession(sessionId, cwd) = submittedCommands[1] {
                XCTAssertEqual(sessionId, "session-c")
                XCTAssertNil(cwd)
            } else {
                XCTFail("only the latest resume target may be submitted")
            }
        }

        func testNewSessionSerializesCancelBeforeSessionTransition() async {
            let source = makeSource()
            var submittedCommands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { submittedCommands.append(command) }
            }
            source.beginTurnForTesting(turnId: 23, sessionId: "session-a")

            source.startNewConversation()
            await waitForSubmittedCommands(2, commands: submittedCommands)

            XCTAssertEqual(submittedCommands.count, 2)
            guard submittedCommands.count == 2 else { return }
            if case let .cancel(turnId) = submittedCommands[0] {
                XCTAssertEqual(turnId, 23)
            } else {
                XCTFail("the old turn must be cancelled first")
            }
            if case let .newSession(cwd, model) = submittedCommands[1] {
                XCTAssertNil(cwd)
                XCTAssertNil(model)
            } else {
                XCTFail("newSession must follow the awaited cancel")
            }
        }
    }

#endif
