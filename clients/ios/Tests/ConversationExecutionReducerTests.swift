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
                    inputJson: #"{"command":"echo ok","cwd":"/workspace"}"#,
                    header: nil
                ))
            source.applyForTesting(.toolHeartbeat(id: "task-1", tool: "bash", elapsedMs: 320))
            source.applyForTesting(
                .toolUseResult(
                    id: "task-1",
                    tool: "bash",
                    resultJson: #"{"data":{"stdout":"ok\n","stderr":"","exit_code":0,"duration_ms":480,"truncated":false}}"#,
                    isError: false,
                    display: nil
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

        func testTerminalRunStaysAnchoredBeforeLaterOutOfBandNotice() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 33, sessionId: "session-a")

            source.applyForTesting(.turnStarted(turnId: 33))
            source.applyForTesting(.messageComplete(
                stopReason: "end_turn",
                message: MessageDto(role: "assistant", blocks: [.text(text: "完成")])
            ))
            source.applyForTesting(.taskStatusChanged(taskId: "late-task", status: .completed))
            source.applyForTesting(
                .turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            let kinds = source.model.items.map { item -> String in
                switch item {
                case .message: return "message"
                case .run: return "run"
                case .notice: return "notice"
                case .toolCall: return "tool"
                }
            }
            XCTAssertEqual(
                kinds,
                ["message", "run", "notice"],
                "the terminal result belongs to its assistant message, not the list tail")
        }

        func testCoordinatorWorkersKeepBackgroundExecutionAfterTurnEndsUntilIdlePush() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 34, sessionId: "session-a")

            source.applyForTesting(.turnStarted(turnId: 34))
            source.applyForTesting(.coordinatorStatus(activeWorkers: 1, team: "review"))
            source.applyForTesting(.messageComplete(
                stopReason: "end_turn",
                message: MessageDto(role: "assistant", blocks: [.text(text: "Delegated")])
            ))
            source.applyForTesting(
                .turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            XCTAssertTrue(
                source.model.requiresBackgroundExecution,
                "a worker can outlive the assistant turn that started it")

            source.applyForTesting(.coordinatorStatus(activeWorkers: 0, team: "review"))
            XCTAssertFalse(source.model.requiresBackgroundExecution)
        }

        func testCoordinatorIdlePushClearsWorkersFromAnEarlierTurn() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 35, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 35))
            source.applyForTesting(.coordinatorStatus(activeWorkers: 1, team: "review"))
            source.applyForTesting(
                .turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            source.beginTurnForTesting(turnId: 36, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 36))
            source.applyForTesting(.coordinatorStatus(activeWorkers: 0, team: "review"))
            source.applyForTesting(
                .turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            let workerCounts = source.model.items.compactMap { item -> UInt32? in
                guard case let .run(run) = item else { return nil }
                return run.activeWorkers
            }
            XCTAssertTrue(workerCounts.allSatisfy { $0 == 0 })
            XCTAssertFalse(source.model.requiresBackgroundExecution)
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
                    .toolUse(id: "t1", tool: "Read", inputJson: #"{"path":"/tmp/a"}"#, header: nil),
                    .toolResult(id: "t1", tool: "Read", resultJson: #"{"result":"ok"}"#, isError: false, oldString: nil, newString: nil, filePath: nil, display: nil),
                ])))

            XCTAssertEqual(source.model.messages.count, 1)
            let message = source.model.messages[0]
            let detail = source.model.messageDetails[message.id]

            XCTAssertNotNil(detail)
            XCTAssertEqual(detail?.blocks.count, 4)
            XCTAssertTrue(message.text.contains("正文"))
            XCTAssertTrue(message.text.contains("推理"))
            if case let .toolUse(_, tool, inputSummary, _, _)? = detail?.blocks[2] {
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
                    inputJson: #"{"command":"pwd"}"#,
                    header: nil
                ))
            source.applyForTesting(.sessionResumed(sessionId: "new-session", messages: []))
            source.applyForTesting(
                .toolUseResult(
                    id: "task-2",
                    tool: "bash",
                    resultJson: #"{"data":{"stdout":"stale","stderr":"","exit_code":0}}"#,
                    isError: false,
                    display: nil
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
                inputJson: #"{"query":"weather"}"#,
                header: nil
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
                inputJson: #"{"query":"Wuhan weather today"}"#,
                header: nil
            ))
            source.applyForTesting(.toolUseStarted(
                id: "shell-1",
                tool: "Bash",
                inputJson: #"{"command":"sleep 30"}"#,
                header: nil
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

        // MARK: - Engine-derived tool presentation
        //
        // The engine derives the header / result block ONCE and ships them on
        // additive wire fields. These assert the reducer CARRIES them instead of
        // re-deriving anything from `input_json` / `result_json`.

        private func sampleHeader() -> ToolHeaderDto {
            ToolHeaderDto(
                verb: .update,
                label: "Update",
                primary: "src/host.rs",
                qualifier: " (3 edits)",
                count: nil,
                subLine: nil,
                title: "Update(src/host.rs) (3 edits)"
            )
        }

        private func sampleDiff() -> StructuredDiffDto {
            StructuredDiffDto(
                filePath: "src/host.rs",
                language: "rust",
                gutterWidth: 3,
                additions: 1,
                removals: 1,
                truncatedRows: 2,
                rows: [
                    DiffRowDto(
                        kind: .remove, lineNo: 41, hunk: 0, wordDiffed: true,
                        segments: [
                            CodeSegmentDto(text: "let ", class: .keyword, rgb: nil,
                                           bold: false, italic: false, underline: false, emph: false),
                            CodeSegmentDto(text: "old", class: .variable, rgb: 0x00FF_0000,
                                           bold: false, italic: false, underline: false, emph: true),
                        ]
                    ),
                    DiffRowDto(
                        kind: .add, lineNo: 41, hunk: 1, wordDiffed: false,
                        segments: [
                            CodeSegmentDto(text: "let new", class: .plain, rgb: nil,
                                           bold: true, italic: false, underline: false, emph: false),
                        ]
                    ),
                ]
            )
        }

        func testToolUseStartedCarriesTheEngineDerivedHeader() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 60, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 60))

            source.applyForTesting(.toolUseStarted(
                id: "edit-1",
                tool: "MultiEdit",
                inputJson: #"{"file_path":"src/host.rs"}"#,
                header: sampleHeader()
            ))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected a run render item")
            }
            guard let header = run.tools.first?.header else {
                return XCTFail("the derived header must survive onto the trace")
            }
            XCTAssertEqual(header.verb, .update)
            XCTAssertEqual(header.primary, "src/host.rs")
            XCTAssertEqual(header.qualifier, " (3 edits)")
            XCTAssertEqual(header.title, "Update(src/host.rs) (3 edits)")
            // The verb is what a localizing client renders — never the English
            // `label` that rides along for non-localizing surfaces.
            XCTAssertEqual(
                ToolDisplayText.verbLabel(header),
                String(localized: "chat_tool_verb_update"))
        }

        func testToolUseResultCarriesTheEngineDerivedDisplay() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 61, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 61))
            source.applyForTesting(.toolUseStarted(
                id: "edit-1", tool: "Edit",
                inputJson: #"{"file_path":"src/host.rs"}"#, header: sampleHeader()))
            source.applyForTesting(.toolUseResult(
                id: "edit-1",
                tool: "Edit",
                resultJson: #"{"result":"ok"}"#,
                isError: false,
                display: ToolResultDisplayDto(
                    headline: "Added 18 lines, removed 4 lines",
                    headlineKind: .addedRemoved,
                    headlineArgs: [18, 4],
                    diff: sampleDiff(),
                    body: "line a\nline b",
                    bodyLines: 42,
                    bodyTruncated: true,
                    collapsed: true
                )
            ))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected a run render item")
            }
            guard let display = run.tools.first?.display else {
                return XCTFail("the result display must survive onto the trace")
            }
            XCTAssertEqual(display.headlineKind, .addedRemoved)
            XCTAssertEqual(display.headlineArgs, [18, 4])
            XCTAssertEqual(display.bodyLines, 42)
            XCTAssertTrue(display.bodyTruncated)
            XCTAssertTrue(display.collapsed, "an over-budget body renders collapsed")

            // Localized from the KIND + args, not from the English headline.
            XCTAssertEqual(
                ToolDisplayText.headline(display),
                String(localized: "chat_result_added_removed \(18) \(4)"))

            // The structured diff arrives PRE-SPLIT: concatenating a row's
            // segments reproduces the line exactly, with no offsets on the wire.
            guard let diff = display.diff else { return XCTFail("expected a diff") }
            XCTAssertEqual(diff.gutterWidth, 3)
            XCTAssertEqual(diff.truncatedRows, 2)
            XCTAssertEqual(diff.rows.count, 2)
            XCTAssertEqual(diff.rows[0].segments.map(\.text).joined(), "let old")
            XCTAssertEqual(diff.rows[0].kind, .remove)
            XCTAssertEqual(diff.rows[0].segments[0].syntax, .keyword)
            XCTAssertTrue(diff.rows[0].segments[1].emph,
                          "a word-diffed run keeps its emphasis flag")
            XCTAssertEqual(diff.rows[0].segments[1].rgb, 0x00FF_0000)
            // A hunk index CHANGE between consecutive rows is where the `⋯`
            // separator belongs; there is no separator row kind.
            XCTAssertNotEqual(diff.rows[0].hunk, diff.rows[1].hunk)
            XCTAssertTrue(diff.rows[1].segments[0].bold)
        }

        func testEngineOverriddenLabelSurvivesLocalization() {
            // `REPL` and a named subagent share a verb with a canonical English
            // label but override it with a proper noun. That override must be
            // rendered verbatim — the localized verb would erase it.
            let repl = ConversationToolHeader(
                verb: .shell, label: "REPL", primary: nil, qualifier: nil,
                count: nil, subLine: nil, title: "REPL")
            XCTAssertEqual(ToolDisplayText.verbLabel(repl), "REPL")

            let subagent = ConversationToolHeader(
                verb: .task, label: "code-reviewer", primary: "review the diff",
                qualifier: nil, count: nil, subLine: nil,
                title: "code-reviewer(review the diff)")
            XCTAssertEqual(ToolDisplayText.verbLabel(subagent), "code-reviewer")

            // A shell call that DID count is localized with its count.
            let bash = ConversationToolHeader(
                verb: .shell, label: "Running 1 shell command…", primary: nil,
                qualifier: nil, count: 1,
                subLine: ConversationToolSubLine(prefix: "$", text: "cargo test"),
                title: "Running 1 shell command…")
            XCTAssertEqual(
                ToolDisplayText.verbLabel(bash),
                String(localized: "chat_tool_verb_shell \(1)"))
            XCTAssertNotEqual(ToolDisplayText.verbLabel(bash), bash.label)
        }

        func testPlanUpdatedReplacesTheWholeListAndAnEmptyListClearsIt() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 62, sessionId: "session-a")

            source.applyForTesting(.planUpdated(tasks: [
                PlanTaskDto(id: "1", subject: "写代码", activeForm: "正在写代码", state: .inProgress),
                PlanTaskDto(id: nil, subject: "跑测试", activeForm: nil, state: .pending),
            ]))
            XCTAssertEqual(source.model.planTasks.count, 2)
            XCTAssertEqual(source.model.planTasks[0].state, .inProgress)
            XCTAssertEqual(source.model.planTasks[0].taskId, "1")
            XCTAssertEqual(source.model.planTasks[1].id, "跑测试",
                           "a TodoWrite V1 item has no id and falls back to its subject")

            // FULL-LIST replace, not a merge.
            source.applyForTesting(.planUpdated(tasks: [
                PlanTaskDto(id: "1", subject: "写代码", activeForm: nil, state: .completed),
            ]))
            XCTAssertEqual(source.model.planTasks.count, 1)
            XCTAssertEqual(source.model.planTasks[0].state, .completed)

            // An empty list CLEARS the plan — it is never "no news".
            source.applyForTesting(.planUpdated(tasks: []))
            XCTAssertTrue(source.model.planTasks.isEmpty)
        }

        func testPlanUpdatedIsNotDroppedOutsideATurn() {
            // The plan block outlives the turn that wrote it; the turn gate must
            // allowlist it exactly like TaskRow / AskUserQuestion.
            let source = makeSource()
            source.applyForTesting(.planUpdated(tasks: [
                PlanTaskDto(id: nil, subject: "落库", activeForm: nil, state: .pending),
            ]))
            XCTAssertEqual(source.model.planTasks.count, 1)
        }

        func testSessionResumeClearsThePlanAndTheExpandedRows() {
            let source = makeSource()
            source.applyForTesting(.planUpdated(tasks: [
                PlanTaskDto(id: nil, subject: "旧计划", activeForm: nil, state: .pending),
            ]))
            source.model.expandedToolCalls = ["edit-1"]

            source.applyForTesting(.sessionResumed(sessionId: "session-z", messages: []))

            XCTAssertTrue(source.model.planTasks.isEmpty,
                          "the plan belonged to the transcript we just replaced")
            XCTAssertTrue(source.model.expandedToolCalls.isEmpty)
        }

        func testOlderEngineWithoutHeaderOrDisplayKeepsTheLegacySummaries() {
            let source = makeSource()
            source.beginTurnForTesting(turnId: 63, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 63))
            source.applyForTesting(.toolUseStarted(
                id: "old-1", tool: "Read",
                inputJson: #"{"file_path":"/tmp/a"}"#, header: nil))
            source.applyForTesting(.toolUseResult(
                id: "old-1", tool: "Read",
                resultJson: #"{"message":"read 3 lines"}"#, isError: false, display: nil))

            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected a run render item")
            }
            XCTAssertNil(run.tools.first?.header)
            XCTAssertNil(run.tools.first?.display)
            XCTAssertEqual(run.tools.first?.inputSummary, "/tmp/a")
            XCTAssertEqual(run.tools.first?.outputSummary, "read 3 lines")
        }
    }

#endif
