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

        func testCancelledTurnDefersNextSendUntilTerminalThenStartsExactlyOnce() async {
            let source = makeSource()
            var submittedCommands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                await MainActor.run {
                    submittedCommands.append(command)
                }
            }

            source.beginTurnForTesting(turnId: 1, sessionId: "session-a")
            source.applyForTesting(.turnStarted(turnId: 1))
            source.cancel()
            await flushTasks()

            source.send("B prompt")
            await flushTasks()

            XCTAssertEqual(submittedCommands.count, 1, "B must not submit before A terminal drains")
            if case let .cancel(turnId)? = submittedCommands.first {
                XCTAssertEqual(turnId, 1)
            } else {
                XCTFail("expected only cancel submission before A terminal")
            }
            XCTAssertEqual(source.model.messages.count, 1, "B user message should still render immediately")
            XCTAssertEqual(source.model.messages.first?.role, .user)
            XCTAssertEqual(source.model.messages.first?.text, "B prompt")
            XCTAssertFalse(source.model.streaming, "B must stay deferred while A is quarantined")

            source.applyForTesting(.textDelta(text: "late-a"))
            source.applyForTesting(.toolUseStarted(
                id: "late-tool",
                tool: "Read",
                inputJson: #"{"path":"/tmp/late"}"#
            ))
            source.applyForTesting(.apiRetry(message: "late retry", attempt: 1, maxRetries: 3, delayMs: 500))
            source.applyForTesting(.turnStarted(turnId: nil))

            XCTAssertEqual(source.model.messages.count, 1, "late A events must not open assistant content for B")
            XCTAssertEqual(source.model.statusLine, "正在等待上一轮取消完成…", "late A events must not replace deferred-send waiting state")
            guard case let .run(gatedRun)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected A run item to remain isolated")
            }
            XCTAssertTrue(gatedRun.tools.isEmpty, "late tool activity must stay quarantined")
            XCTAssertNil(gatedRun.retry, "late retry must stay quarantined")

            source.applyForTesting(.turnEnded(outcome: .cancelled, stopReason: nil, cost: zeroCost))
            await flushTasks()

            XCTAssertEqual(submittedCommands.count, 2, "B should submit exactly once after A terminal")
            if case let .sendPrompt(text, _, _, turnId)? = submittedCommands.last {
                XCTAssertEqual(text, "B prompt")
                XCTAssertEqual(turnId, 2)
            } else {
                XCTFail("expected deferred B sendPrompt after A terminal")
            }
            XCTAssertTrue(source.model.streaming, "B should become active only after A terminal")

            source.applyForTesting(.turnStarted(turnId: nil))
            source.applyForTesting(.textDelta(text: "reply-b"))
            source.applyForTesting(.toolUseStarted(
                id: "tool-b",
                tool: "Read",
                inputJson: #"{"path":"/tmp/current"}"#
            ))
            source.applyForTesting(.apiRetry(message: "retry-b", attempt: 2, maxRetries: 5, delayMs: 700))
            source.applyForTesting(.turnEnded(outcome: .endTurn, stopReason: "end_turn", cost: zeroCost))

            XCTAssertEqual(source.model.messages.last?.text, "reply-b", "B should accept normal uncorrelated stream after deferred start")
            XCTAssertEqual(source.model.statusLine, nil, "B terminal should clear transient status")
            guard case let .run(openRun)? = source.model.items.last(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("expected run item after quarantine drain")
            }
            XCTAssertEqual(openRun.tools.first?.tool, "Read")
            XCTAssertEqual(openRun.retry?.message, "retry-b")
            XCTAssertEqual(openRun.status, .completed)
            XCTAssertEqual(submittedCommands.count, 2, "prompt must not be submitted twice")
        }

        func testResumeSerializesCancelBeforeSessionTransition() async {
            let source = makeSource()
            var submittedCommands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { submittedCommands.append(command) }
            }
            source.beginTurnForTesting(turnId: 17, sessionId: "session-a")

            source.resumeSession("session-b")
            await flushTasks()

            XCTAssertEqual(submittedCommands.count, 2)
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

        func testNewSessionSerializesCancelBeforeSessionTransition() async {
            let source = makeSource()
            var submittedCommands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                await MainActor.run { submittedCommands.append(command) }
            }
            source.beginTurnForTesting(turnId: 23, sessionId: "session-a")

            source.startNewConversation()
            await flushTasks()

            XCTAssertEqual(submittedCommands.count, 2)
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
