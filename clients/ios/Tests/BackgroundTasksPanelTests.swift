// BackgroundTasksPanelTests.swift — the pinned tasks panel's data feed.
//
// Drives synthetic `ClientEvent.taskRow` / `.taskStatusChanged` through
// `EngineConversationSource.apply` (the `applyForTesting` seam) and asserts
// the upsert semantics the panel renders from: out-of-turn delivery, row
// backfill of a push-first id, and status transitions in place. Hermetic —
// no engine handle is ever built.

import XCTest

@testable import LingxiCode

#if canImport(engine_mobileFFI)
    import engine_mobileFFI
#endif

#if canImport(engine_mobileFFI)

    @MainActor
    final class BackgroundTasksPanelTests: XCTestCase {
        private func makeSource() -> EngineConversationSource {
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                apiKey: "",
                model: "",
                appSandboxRoot: NSTemporaryDirectory())
            return EngineConversationSource(config: config)
        }

        private func row(
            id: String,
            status: TaskStatusDto,
            description: String = "local-app-build workflow"
        ) -> TaskRowDto {
            TaskRowDto(taskId: id, taskType: "workflow", status: status, description: description)
        }

        private func workflowProgress(
            kind: ConversationWorkflowProgressKind,
            index: UInt64? = nil,
            title: String? = nil,
            message: String? = nil,
            label: String? = nil,
            phaseIndex: UInt32? = nil,
            phaseTitle: String? = nil,
            agentId: String? = nil,
            agentType: String? = nil,
            model: String? = nil,
            fallbackModel: String? = nil,
            state: ConversationWorkflowAgentState? = nil,
            error: String? = nil,
            toolUseId: String? = nil,
            startedAtMs: UInt64? = nil,
            queuedAtMs: UInt64? = nil,
            lastProgressAtMs: UInt64? = nil,
            attempt: UInt32? = nil,
            lastAttemptReason: String? = nil,
            tokens: UInt64? = nil,
            toolCalls: UInt64? = nil,
            lastToolName: String? = nil,
            lastToolSummary: String? = nil,
            promptPreview: String? = nil
        ) -> ConversationWorkflowProgressPayload {
            ConversationWorkflowProgressPayload(
                kind: kind,
                index: index,
                title: title,
                message: message,
                label: label,
                phaseIndex: phaseIndex,
                phaseTitle: phaseTitle,
                agentId: agentId,
                agentType: agentType,
                model: model,
                fallbackModel: fallbackModel,
                state: state,
                error: error,
                toolUseId: toolUseId,
                startedAtMs: startedAtMs,
                queuedAtMs: queuedAtMs,
                lastProgressAtMs: lastProgressAtMs,
                attempt: attempt,
                lastAttemptReason: lastAttemptReason,
                tokens: tokens,
                toolCalls: toolCalls,
                lastToolName: lastToolName,
                lastToolSummary: lastToolSummary,
                promptPreview: promptPreview
            )
        }

        /// `TaskRow` replies arrive outside any turn and must seed the panel.
        func testTaskRowOutsideATurnSeedsThePanel() {
            let source = makeSource()
            XCTAssertFalse(source.model.streaming, "precondition: no turn is in flight")

            source.applyForTesting(.taskRow(task: row(id: "wmo6xnbac", status: .running)))

            XCTAssertEqual(source.model.backgroundTasks.map(\.id), ["wmo6xnbac"])
            XCTAssertEqual(source.model.backgroundTasks.first?.status, .running)
            XCTAssertEqual(
                source.model.backgroundTasks.first?.descriptionText,
                "local-app-build workflow"
            )
        }

        /// A workflow adopted after process restart is visible but not falsely
        /// animated as still running. It remains non-terminal so explicit resume
        /// can replace it with a live task.
        func testPausedWorkflowRowSurvivesSessionRestore() {
            let source = makeSource()

            source.applyForTesting(.taskRow(task: row(id: "wmo6xnbac", status: .paused)))

            XCTAssertEqual(source.model.backgroundTasks.map(\.id), ["wmo6xnbac"])
            XCTAssertEqual(source.model.backgroundTasks.first?.status, .paused)
            XCTAssertFalse(source.model.backgroundTasks.first?.status.isTerminal ?? true)
        }

        /// A status push for an id the panel has never seen inserts a
        /// placeholder row (bare id) instead of being dropped; the later
        /// `TaskRow` backfills the description WITHOUT resetting the newer
        /// status.
        func testPushFirstIdGetsAPlaceholderThenBackfills() {
            let source = makeSource()

            source.applyForTesting(.taskStatusChanged(
                taskId: "wmo6xnbac", status: .running, originSessionId: nil))
            XCTAssertEqual(source.model.backgroundTasks.map(\.id), ["wmo6xnbac"])
            XCTAssertEqual(source.model.backgroundTasks.first?.descriptionText, "")

            source.applyForTesting(.taskRow(task: row(id: "wmo6xnbac", status: .running)))
            XCTAssertEqual(source.model.backgroundTasks.count, 1, "row upserts, never duplicates")
            XCTAssertEqual(
                source.model.backgroundTasks.first?.descriptionText,
                "local-app-build workflow"
            )
        }

        func testTaskStatusPushRejectsAnotherSessionWorkflow() {
            let source = makeSource()
            source.model.activeSessionId = "session-b"

            source.applyForTesting(.taskStatusChanged(
                taskId: "workflow-a",
                status: .running,
                originSessionId: "session-a"
            ))
            XCTAssertTrue(source.model.backgroundTasks.isEmpty)

            source.applyForTesting(.taskStatusChanged(
                taskId: "workflow-b",
                status: .running,
                originSessionId: "session-b"
            ))
            XCTAssertEqual(source.model.backgroundTasks.map(\.id), ["workflow-b"])
        }

        /// Status transitions update the row in place, and the terminal
        /// transition still appends the completion notice the transcript
        /// shows today.
        func testStatusTransitionUpdatesInPlaceAndKeepsTheTerminalNotice() {
            let source = makeSource()
            source.applyForTesting(.taskRow(task: row(id: "wmo6xnbac", status: .pending)))

            source.applyForTesting(.taskStatusChanged(
                taskId: "wmo6xnbac", status: .running, originSessionId: nil))
            XCTAssertEqual(source.model.backgroundTasks.first?.status, .running)

            source.applyForTesting(.taskStatusChanged(
                taskId: "wmo6xnbac", status: .completed, originSessionId: nil))
            XCTAssertEqual(source.model.backgroundTasks.first?.status, .completed)
            XCTAssertEqual(source.model.backgroundTasks.count, 1)
            XCTAssertTrue(
                source.model.items.contains { item in
                    if case let .notice(notice) = item { return notice.text.contains("wmo6xnbac") }
                    return false
                },
                "the terminal status keeps appending the transcript notice"
            )
        }

        /// Once a task reaches a terminal state, stale non-terminal updates
        /// may still backfill the description but must not downgrade status.
        func testTerminalStatusStaysMonotonicWhileStaleUpdatesBackfillDescription() {
            let terminalCases: [(TaskStatusDto, BackgroundTaskSnapshot.Status)] = [
                (.completed, .completed),
                (.failed, .failed),
                (.cancelled, .cancelled),
            ]

            for (terminalWireStatus, terminalSnapshotStatus) in terminalCases {
                let source = makeSource()
                let taskID = "wmo6xnbac-\(terminalSnapshotStatus)"
                let description = "workflow \(terminalSnapshotStatus)"

                source.applyForTesting(.taskStatusChanged(
                    taskId: taskID, status: terminalWireStatus, originSessionId: nil))
                XCTAssertEqual(source.model.backgroundTasks.first?.status, terminalSnapshotStatus)
                XCTAssertEqual(
                    source.model.backgroundTasks.first?.descriptionText,
                    "",
                    "push-first placeholder should stay empty until a row arrives"
                )

                source.applyForTesting(.taskRow(task: row(
                    id: taskID,
                    status: .running,
                    description: description
                )))
                XCTAssertEqual(source.model.backgroundTasks.first?.status, terminalSnapshotStatus)
                XCTAssertEqual(
                    source.model.backgroundTasks.first?.descriptionText,
                    description,
                    "stale task rows should still backfill human-readable descriptions"
                )

                source.applyForTesting(.taskStatusChanged(
                    taskId: taskID, status: .running, originSessionId: nil))
                XCTAssertEqual(
                    source.model.backgroundTasks.first?.status,
                    terminalSnapshotStatus,
                    "stale running pushes must not reopen terminal tasks"
                )
                XCTAssertEqual(source.model.backgroundTasks.first?.descriptionText, description)
            }
        }

        /// The Workflow tool only launches the background run. Once that tool
        /// returns, the client must pull the registry row so the pinned panel
        /// can show the run while it continues outside the turn.
        func testSuccessfulWorkflowLaunchRefreshesTaskRows() async {
            let source = makeSource()
            let refreshed = expectation(description: "TaskList submitted")
            source.setCommandSubmitterForTesting { command in
                if case .taskList = command {
                    refreshed.fulfill()
                }
            }
            source.beginTurnForTesting()

            source.applyForTesting(.toolUseResult(
                id: "workflow-tool",
                tool: "Workflow",
                resultJson: #"{"status":"async_launched","taskId":"wmo6xnbac"}"#,
                isError: false,
                display: nil
            ))

            await fulfillment(of: [refreshed], timeout: 1)
        }

        func testWorkflowProgressSeedsTaskWithoutTranscriptLookup() {
            let source = makeSource()

            source.applyWorkflowProgressForTesting(
                taskId: "wf-task",
                runId: "run-1",
                progress: workflowProgress(
                    kind: .workflowAgent,
                    index: 0,
                    title: "Design agent",
                    phaseIndex: 0,
                    phaseTitle: "Design",
                    state: .start,
                    queuedAtMs: 1_000
                )
            )

            XCTAssertEqual(source.model.backgroundTasks.map(\.id), ["wf-task"])
            XCTAssertEqual(source.model.backgroundTasks.first?.status, .running)
            XCTAssertEqual(source.model.backgroundTasks.first?.workflow?.runId, "run-1")
            XCTAssertEqual(source.model.backgroundTasks.first?.workflow?.agents.count, 1)
            XCTAssertTrue(
                source.model.agentTranscripts.isEmpty,
                "workflow live status must not depend on transcript loading"
            )
        }

        func testWorkflowProgressIsScopedToItsOriginSession() {
            let source = makeSource()
            source.model.activeSessionId = "session-b"
            let progress = workflowProgress(
                kind: .workflowAgent,
                index: 0,
                title: "Design",
                state: .progress
            )

            source.applyWorkflowProgressForTesting(
                originSessionId: "session-a",
                taskId: "workflow-a",
                runId: "run-a",
                progress: progress
            )
            XCTAssertTrue(source.model.backgroundTasks.isEmpty)

            source.applyWorkflowProgressForTesting(
                originSessionId: "session-b",
                taskId: "workflow-b",
                runId: "run-b",
                progress: progress
            )
            XCTAssertEqual(source.model.backgroundTasks.map(\.id), ["workflow-b"])
        }

        func testWorkflowReducerPreservesTerminalAgentAgainstLateProgress() {
            let source = makeSource()

            source.applyWorkflowProgressForTesting(
                taskId: "wf-task",
                runId: "run-1",
                progress: workflowProgress(
                    kind: .workflowAgent,
                    index: 0,
                    title: "Generate",
                    phaseIndex: 2,
                    phaseTitle: "Generate",
                    state: .done,
                    startedAtMs: 1_000,
                    lastProgressAtMs: 5_000,
                    tokens: 420,
                    toolCalls: 3
                )
            )
            source.applyWorkflowProgressForTesting(
                taskId: "wf-task",
                runId: "run-1",
                progress: workflowProgress(
                    kind: .workflowAgent,
                    index: 0,
                    title: "Generate",
                    message: "stale",
                    phaseIndex: 2,
                    phaseTitle: "Generate",
                    state: .progress,
                    startedAtMs: 1_000,
                    lastProgressAtMs: 2_000,
                    tokens: 200,
                    toolCalls: 1
                )
            )

            let agent = try? XCTUnwrap(source.model.backgroundTasks.first?.workflow?.agents.first)
            XCTAssertEqual(agent?.state, .done)
            XCTAssertEqual(agent?.tokens, 420)
            XCTAssertEqual(agent?.toolCalls, 3)
            XCTAssertNotEqual(agent?.message, "stale")
        }

        func testWorkflowReducerNeverReopensTerminalAgentWithNewerProgress() {
            let source = makeSource()

            source.applyWorkflowProgressForTesting(
                taskId: "wf-task",
                runId: "run-1",
                progress: workflowProgress(
                    kind: .workflowAgent,
                    index: 0,
                    title: "Design",
                    state: .done,
                    lastProgressAtMs: 5_000,
                    tokens: 420,
                    toolCalls: 3
                )
            )
            source.applyWorkflowProgressForTesting(
                taskId: "wf-task",
                runId: "run-1",
                progress: workflowProgress(
                    kind: .workflowAgent,
                    index: 0,
                    title: "Design",
                    message: "late progress beacon",
                    state: .progress,
                    lastProgressAtMs: 6_000,
                    tokens: 421,
                    toolCalls: 4
                )
            )

            let agent = try? XCTUnwrap(source.model.backgroundTasks.first?.workflow?.agents.first)
            XCTAssertEqual(agent?.state, .done)
            XCTAssertNotEqual(agent?.message, "late progress beacon")
            XCTAssertEqual(agent?.tokens, 420)
            XCTAssertEqual(agent?.toolCalls, 3)
        }

        func testWorkflowTaskStatusStaysTerminalWhenLateProgressArrives() {
            let source = makeSource()
            source.applyForTesting(.taskStatusChanged(
                taskId: "wf-task", status: .completed, originSessionId: nil))

            source.applyWorkflowProgressForTesting(
                taskId: "wf-task",
                runId: "run-1",
                progress: workflowProgress(
                    kind: .workflowAgent,
                    index: 0,
                    title: "Verify",
                    phaseIndex: 4,
                    phaseTitle: "Verify",
                    state: .progress,
                    lastProgressAtMs: 3_000
                )
            )

            XCTAssertEqual(source.model.backgroundTasks.first?.status, .completed)
            XCTAssertNil(source.model.backgroundTasks.first?.workflow)

            source.applyWorkflowProgressForTesting(
                taskId: "wf-task",
                runId: "run-1",
                progress: workflowProgress(
                    kind: .workflowAgent,
                    index: 0,
                    title: "Verify",
                    phaseIndex: 4,
                    phaseTitle: "Verify",
                    state: .done,
                    lastProgressAtMs: 3_100
                )
            )
            XCTAssertEqual(source.model.backgroundTasks.first?.workflow?.agents.first?.state, .done)
        }

        func testWorkflowReducerDedupesRepeatedLogsAndGroupsByPhase() {
            let source = makeSource()

            source.applyWorkflowProgressForTesting(
                taskId: "wf-task",
                runId: "run-1",
                progress: workflowProgress(
                    kind: .workflowPhase,
                    message: "confirming structure",
                    phaseIndex: 0,
                    phaseTitle: "Design",
                    lastProgressAtMs: 1_000
                )
            )
            let log = workflowProgress(
                kind: .workflowLog,
                message: "reviewing hierarchy",
                label: "Progress",
                phaseIndex: 0,
                phaseTitle: "Design",
                lastProgressAtMs: 1_100
            )
            source.applyWorkflowProgressForTesting(taskId: "wf-task", runId: "run-1", progress: log)
            source.applyWorkflowProgressForTesting(taskId: "wf-task", runId: "run-1", progress: log)
            source.applyWorkflowProgressForTesting(
                taskId: "wf-task",
                runId: "run-1",
                progress: workflowProgress(
                    kind: .workflowAgent,
                    index: 0,
                    title: "Design agent",
                    phaseIndex: 0,
                    phaseTitle: "Design",
                    state: .progress,
                    lastProgressAtMs: 1_200
                )
            )

            let workflow = try? XCTUnwrap(source.model.backgroundTasks.first?.workflow)
            XCTAssertEqual(workflow?.logs.count, 1)

            let sections = TasksStatusPanel.workflowSections(for: workflow!, nowMs: 2_000)
            XCTAssertEqual(sections.map(\.title), ["Design"])
            XCTAssertEqual(sections.first?.agents.map(\.displayTitle), ["Design agent"])
            XCTAssertEqual(sections.first?.subtitle, "reviewing hierarchy")
        }

        func testWorkflowReducerReplacesSnapshotWhenRunIDChanges() {
            let source = makeSource()

            source.applyWorkflowProgressForTesting(
                taskId: "wf-task",
                runId: "run-1",
                progress: workflowProgress(
                    kind: .workflowAgent,
                    index: 0,
                    title: "Old run",
                    phaseIndex: 0,
                    phaseTitle: "Design",
                    state: .done,
                    lastProgressAtMs: 1_000
                )
            )
            source.applyWorkflowProgressForTesting(
                taskId: "wf-task",
                runId: "run-2",
                progress: workflowProgress(
                    kind: .workflowAgent,
                    index: 1,
                    title: "New run",
                    phaseIndex: 1,
                    phaseTitle: "Generate",
                    state: .progress,
                    lastProgressAtMs: 2_000
                )
            )

            let workflow = try? XCTUnwrap(source.model.backgroundTasks.first?.workflow)
            XCTAssertEqual(workflow?.runId, "run-2")
            XCTAssertEqual(workflow?.agents.map(\.displayTitle), ["New run"])
            XCTAssertEqual(workflow?.sortedPhases.map(\.title), ["Generate"])
        }

        func testWorkflowCompactSummaryDoesNotInventAgentBeforeAllocation() {
            let task = BackgroundTaskSnapshot(
                id: "wf-task",
                descriptionText: "Workflow",
                status: .running,
                workflow: ConversationWorkflowRunSnapshot(
                    taskId: "wf-task",
                    runId: "run-1",
                    phases: [
                        ConversationWorkflowPhaseSnapshot(
                            id: "phase-0",
                            index: 0,
                            title: "Design",
                            label: nil,
                            message: nil,
                            updatedAtMs: 1_000
                        ),
                    ]
                )
            )

            XCTAssertEqual(
                TasksStatusPanel.workflowCompactSummary(for: task, nowMs: 1_000),
                "Design"
            )
        }

        func testWorkflowCompactSummaryAndAgentMetricsExposeClaudeFields() {
            let task = BackgroundTaskSnapshot(
                id: "wf-task",
                descriptionText: "Workflow",
                status: .running,
                workflow: ConversationWorkflowRunSnapshot(
                    taskId: "wf-task",
                    runId: "run-1",
                    phases: [
                        ConversationWorkflowPhaseSnapshot(
                            id: "phase-1",
                            index: 1,
                            title: "Generate",
                            label: nil,
                            message: nil,
                            updatedAtMs: 2_000
                        ),
                    ],
                    logs: [],
                    agents: [
                        ConversationWorkflowAgentSnapshot(
                            id: "workflow-agent-0",
                            index: 0,
                            title: "Generate agent",
                            message: "using templates",
                            label: nil,
                            phaseIndex: 1,
                            phaseTitle: "Generate",
                            agentId: "agent:1",
                            agentType: "generator",
                            model: "gpt-5.4",
                            fallbackModel: "gpt-5.4-mini",
                            state: .progress,
                            error: nil,
                            toolUseId: nil,
                            startedAtMs: 1_000,
                            queuedAtMs: 900,
                            lastProgressAtMs: 5_000,
                            attempt: 2,
                            lastAttemptReason: "timeout",
                            tokens: 321,
                            toolCalls: 4,
                            lastToolName: "Write",
                            lastToolSummary: "generated files",
                            promptPreview: "..."
                        ),
                        ConversationWorkflowAgentSnapshot(
                            id: "workflow-agent-1",
                            index: 1,
                            title: "Verify agent",
                            message: nil,
                            label: nil,
                            phaseIndex: 1,
                            phaseTitle: "Generate",
                            agentId: "agent:2",
                            agentType: "verifier",
                            model: "gpt-5.4-mini",
                            fallbackModel: nil,
                            state: .done,
                            error: nil,
                            toolUseId: nil,
                            startedAtMs: 1_200,
                            queuedAtMs: 1_100,
                            lastProgressAtMs: 6_000,
                            attempt: 1,
                            lastAttemptReason: nil,
                            tokens: 120,
                            toolCalls: 1,
                            lastToolName: nil,
                            lastToolSummary: nil,
                            promptPreview: nil
                        ),
                    ],
                    lastUpdatedAtMs: 6_000
                )
            )

            let summary = TasksStatusPanel.workflowCompactSummary(for: task, nowMs: 6_000)
            XCTAssertEqual(summary, "1/2 done · 1 running · Generate")

            let metrics = TasksStatusPanel.workflowAgentMetrics(task.workflow!.agents[0], nowMs: 6_000)
            XCTAssertEqual(
                metrics,
                "Running · gpt-5.4 → gpt-5.4-mini · 321 tok · 4 tools · 5s · retry 2: timeout"
            )
        }

        func testWorkflowAgentCountsSeparateQueuedRunningSucceededAndFailed() {
            let states: [ConversationWorkflowAgentState] = [
                .start, .progress, .done, .cached, .error,
            ]
            let agents = states.enumerated().map { index, state in
                ConversationWorkflowAgentSnapshot(
                    id: "agent-\(index)",
                    index: UInt64(index),
                    title: nil,
                    message: nil,
                    label: nil,
                    phaseIndex: nil,
                    phaseTitle: nil,
                    agentId: nil,
                    agentType: nil,
                    model: nil,
                    fallbackModel: nil,
                    state: state,
                    error: nil,
                    toolUseId: nil,
                    startedAtMs: nil,
                    queuedAtMs: nil,
                    lastProgressAtMs: nil,
                    attempt: nil,
                    lastAttemptReason: nil,
                    tokens: nil,
                    toolCalls: nil,
                    lastToolName: nil,
                    lastToolSummary: nil,
                    promptPreview: nil
                )
            }
            let workflow = ConversationWorkflowRunSnapshot(
                taskId: "wf-task",
                runId: "run-1",
                agents: agents
            )

            XCTAssertEqual(workflow.queuedAgents, 1)
            XCTAssertEqual(workflow.runningAgents, 1)
            XCTAssertEqual(workflow.succeededAgents, 2)
            XCTAssertEqual(workflow.failedAgents, 1)
        }

        func testTaskCountsSeparatePausedAndTerminalOutcomes() {
            let statuses: [BackgroundTaskSnapshot.Status] = [
                .pending, .running, .paused, .completed, .failed, .cancelled,
            ]
            let tasks = statuses.enumerated().map { index, status in
                BackgroundTaskSnapshot(
                    id: "task-\(index)",
                    descriptionText: "",
                    status: status
                )
            }

            XCTAssertEqual(
                TasksStatusPanel.taskCounts(tasks),
                TasksStatusPanel.TaskCounts(
                    total: 6,
                    queued: 1,
                    running: 1,
                    succeeded: 1,
                    failed: 1,
                    paused: 1,
                    cancelled: 1
                )
            )
        }
    }

#endif
