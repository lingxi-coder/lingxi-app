// BackgroundTasksPanelTests.swift — the pinned tasks panel's data feed.
//
// Drives synthetic `ClientEvent.taskRow` / `.taskStatusChanged` through
// `EngineConversationSource.apply` (the `applyForTesting` seam) and asserts
// the upsert semantics the panel renders from: out-of-turn delivery, row
// backfill of a push-first id, and status transitions in place. Hermetic —
// no engine handle is ever built.

import XCTest

@testable import LingxiCode

#if canImport(harness_runtimeFFI)
    import harness_runtimeFFI
#endif

#if canImport(harness_runtimeFFI)

    @MainActor
    final class BackgroundTasksPanelTests: XCTestCase {
        private func makeSource() -> EngineConversationSource {
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                apiKey: "",
                model: "",
                appSandboxRoot: NSTemporaryDirectory(),
                                sessionMode: .code,
visionDelegationEnabled: true
            )
            return EngineConversationSource(config: config)
        }

        private func row(
            id: String,
            status: TaskStatusDto,
            description: String = "build workflow",
            error: String? = nil,
            canResume: Bool = false
        ) -> TaskRowDto {
            TaskRowDto(
                taskId: id,
                taskType: "local_workflow",
                status: status,
                description: description,
                canResume: canResume,
                startedAtMs: nil,
                error: error,
                stage: nil
            )
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
                "build workflow"
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

        func testTaskRowOnlyRunningAndFailedTasksRemainVisible() throws {
            let source = makeSource()
            source.applyForTesting(.taskRow(task: row(id: "workflow", status: .running)))
            var task = try XCTUnwrap(source.model.backgroundTasks.first)
            XCTAssertNil(task.workflow)
            XCTAssertTrue(TasksStatusPanel.shouldShowWorkflow([task]))
            XCTAssertEqual(TasksStatusPanel.workflowSteps(for: task).map(\.state), [.running])

            source.applyForTesting(.taskRow(task: row(
                id: "workflow", status: .failed, error: "invalid design JSON"
            )))
            task = try XCTUnwrap(source.model.backgroundTasks.first)
            XCTAssertEqual(TasksStatusPanel.visibleWorkflowTasks([task]).map(\.id), ["workflow"])
            XCTAssertEqual(TasksStatusPanel.workflowSteps(for: task).map(\.state), [.failed])
            XCTAssertEqual(task.errorText, "invalid design JSON")
            XCTAssertEqual(ExecutionStatusPanel.visibleGroups(agents: [.main], tasks: [task], todos: []), [.workflow])
        }

        func testTaskRowOnlySuccessAndCancellationAreHidden() {
            for status: BackgroundTaskSnapshot.Status in [.completed, .cancelled] {
                let task = BackgroundTaskSnapshot(id: "workflow", descriptionText: "Build", status: status)
                XCTAssertFalse(TasksStatusPanel.shouldShowWorkflow([task]))
                XCTAssertTrue(TasksStatusPanel.visibleWorkflowTasks([task]).isEmpty)
            }
        }

        func testFailedResumableTaskRestoresResumeActionWithoutLiveProgress() throws {
            let source = makeSource()
            source.applyForTesting(.taskRow(task: row(
                id: "wmo6xnbac", status: .failed, canResume: true
            )))

            let task = try XCTUnwrap(source.model.backgroundTasks.first)
            XCTAssertNil(task.workflow, "cold TaskList restore has no live progress")
            XCTAssertTrue(TasksStatusPanel.shouldShowWorkflow([task]))
            XCTAssertEqual(TasksStatusPanel.visibleWorkflowTasks([task]).map(\.id), [task.id])
            XCTAssertEqual(TasksStatusPanel.workflowSteps(for: task).map(\.canResume), [true])
        }

        func testResumeIsRejectedDuringSessionTransition() {
            let source = makeSource()
            source.setCommandSubmitterForTesting { _ in XCTFail("must not resume in another session") }
            source.applyForTesting(.taskRow(task: row(id: "workflow", status: .failed, canResume: true)))
            source.model.sessionTransitionPending = true
            source.resumeWorkflow("workflow")
            XCTAssertEqual(source.model.workflowResumeState, .idle)
        }

        func testQueuedResumeIsDroppedAfterSessionChanges() async {
            let source = makeSource()
            source.setCommandSubmitterForTesting { _ in XCTFail("must not submit stale workflow ID") }
            source.applyForTesting(.taskRow(task: row(id: "workflow", status: .failed, canResume: true)))
            source.resumeWorkflow("workflow")
            source.model.activeSessionId = "different-session"
            // Drain the MainActor task queued by resumeWorkflow.
            await Task { @MainActor in }.value
            XCTAssertEqual(source.model.workflowResumeState, .idle)
        }

        func testFailedCreateResumesDirectlyAndSuppressesDuplicateTaps() async {
            let source = makeSource()
            let submitted = expectation(description: "one direct ResumeWorkflow")
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                commands.append(command)
                submitted.fulfill()
            }
            source.applyForTesting(.taskRow(task: row(
                id: "wmo6xnbac", status: .failed, canResume: true
            )))

            source.resumeWorkflow("wmo6xnbac")
            source.resumeWorkflow("wmo6xnbac")
            await fulfillment(of: [submitted], timeout: 1)

            XCTAssertEqual(commands.count, 1)
            guard let command = commands.first, case let .resumeWorkflow(taskID) = command else {
                return XCTFail("Create recovery must submit ResumeWorkflow, never SendPrompt")
            }
            XCTAssertEqual(taskID, "wmo6xnbac")
            XCTAssertEqual(source.model.workflowResumeState, .resuming(taskID: "wmo6xnbac"))
            XCTAssertTrue(source.model.messages.isEmpty)
        }

        func testOrdinaryFailureDoesNotOfferOrSubmitResume() throws {
            let source = makeSource()
            source.setCommandSubmitterForTesting { _ in
                XCTFail("ordinary failed workflows must not be resumed")
            }
            source.applyForTesting(.taskRow(task: row(id: "wmo6xnbac", status: .failed)))

            let task = try XCTUnwrap(source.model.backgroundTasks.first)
            XCTAssertFalse(task.canResumeWorkflow)
            XCTAssertTrue(TasksStatusPanel.shouldShowWorkflow([task]), "failure remains visible without resume capability")
            XCTAssertEqual(TasksStatusPanel.workflowSteps(for: task).map(\.state), [.failed])
            XCTAssertEqual(TasksStatusPanel.workflowSteps(for: task).map(\.canResume), [false])
            source.resumeWorkflow(task.id)
            XCTAssertEqual(source.model.workflowResumeState, .idle)
        }

        func testResumingOneWorkflowDoesNotBlockAnotherTask() async {
            let source = makeSource()
            let submitted = expectation(description: "both distinct workflows resume")
            submitted.expectedFulfillmentCount = 2
            var resumedTaskIDs: [String] = []
            source.setCommandSubmitterForTesting { command in
                guard case let .resumeWorkflow(taskID) = command else {
                    return XCTFail("workflow recovery must retain its direct command")
                }
                resumedTaskIDs.append(taskID)
                submitted.fulfill()
            }
            for taskID in ["wmo6xnbac", "wmo6xnbad"] {
                source.applyForTesting(.taskRow(task: row(
                    id: taskID, status: .paused, canResume: true
                )))
                source.resumeWorkflow(taskID)
            }

            await fulfillment(of: [submitted], timeout: 1)
            XCTAssertEqual(resumedTaskIDs.sorted(), ["wmo6xnbac", "wmo6xnbad"])
        }

        func testPausedWorkflowRetainsDirectResumeCommand() async throws {
            let source = makeSource()
            let submitted = expectation(description: "paused workflow resume")
            source.setCommandSubmitterForTesting { command in
                guard case let .resumeWorkflow(taskID) = command else {
                    return XCTFail("paused workflows keep their existing command")
                }
                XCTAssertEqual(taskID, "wmo6xnbac")
                submitted.fulfill()
            }
            source.applyForTesting(.taskRow(task: row(
                id: "wmo6xnbac", status: .paused, canResume: true
            )))
            let task = try XCTUnwrap(source.model.backgroundTasks.first)
            XCTAssertTrue(task.canResumeWorkflow)

            source.resumeWorkflow(task.id)
            await fulfillment(of: [submitted], timeout: 1)
        }

        func testFullTaskRowRevokesResumeCapabilityWhileStatusPushPreservesIt() throws {
            let source = makeSource()
            source.applyForTesting(.taskRow(task: row(
                id: "wmo6xnbac", status: .failed, canResume: true
            )))
            source.applyForTesting(.taskStatusChanged(
                taskId: "wmo6xnbac", status: .failed, originSessionId: nil, error: nil
            ))
            XCTAssertTrue(try XCTUnwrap(source.model.backgroundTasks.first).canResumeWorkflow)

            source.applyForTesting(.taskRow(task: row(
                id: "wmo6xnbac", status: .failed, canResume: false
            )))
            XCTAssertFalse(try XCTUnwrap(source.model.backgroundTasks.first).canResumeWorkflow)
        }

        func testKnownTaskFailureRefreshesHostResumeCapability() async {
            let source = makeSource()
            let refreshed = expectation(description: "failed task capability refreshed")
            source.setCommandSubmitterForTesting { command in
                if case .taskList = command { refreshed.fulfill() }
            }
            source.applyForTesting(.taskRow(task: row(id: "wmo6xnbac", status: .running)))

            source.applyForTesting(.taskStatusChanged(
                taskId: "wmo6xnbac", status: .failed, originSessionId: nil, error: "invalid JSON"
            ))

            await fulfillment(of: [refreshed], timeout: 1)
        }

        func testPausedCapabilityCannotAuthorizeFailureWithoutHostRefresh() throws {
            let source = makeSource()
            source.setCommandSubmitterForTesting { _ in }
            source.applyForTesting(.taskRow(task: row(
                id: "wmo6xnbac", status: .paused, canResume: true
            )))
            source.applyForTesting(.taskStatusChanged(
                taskId: "wmo6xnbac", status: .failed, originSessionId: nil, error: nil
            ))
            XCTAssertFalse(try XCTUnwrap(source.model.backgroundTasks.first).canResumeWorkflow)

            // The terminal state remains failed when a stale paused snapshot
            // arrives; its old resume right must not be attached to that state.
            source.applyForTesting(.taskRow(task: row(
                id: "wmo6xnbac", status: .paused, canResume: true
            )))
            XCTAssertEqual(source.model.backgroundTasks.first?.status, .failed)
            XCTAssertFalse(try XCTUnwrap(source.model.backgroundTasks.first).canResumeWorkflow)
        }

        /// A status push for an id the panel has never seen inserts a
        /// placeholder row (bare id) instead of being dropped; the later
        /// `TaskRow` backfills the description WITHOUT resetting the newer
        /// status.
        func testPushFirstIdGetsAPlaceholderThenBackfills() {
            let source = makeSource()

            source.applyForTesting(.taskStatusChanged(
                taskId: "wmo6xnbac", status: .running, originSessionId: nil, error: nil
            ))
            XCTAssertEqual(source.model.backgroundTasks.map(\.id), ["wmo6xnbac"])
            XCTAssertEqual(source.model.backgroundTasks.first?.descriptionText, "")

            source.applyForTesting(.taskRow(task: row(id: "wmo6xnbac", status: .running)))
            XCTAssertEqual(source.model.backgroundTasks.count, 1, "row upserts, never duplicates")
            XCTAssertEqual(
                source.model.backgroundTasks.first?.descriptionText,
                "build workflow"
            )
        }

        func testTaskStatusPushRejectsAnotherSessionWorkflow() {
            let source = makeSource()
            source.model.activeSessionId = "session-b"

            source.applyForTesting(.taskStatusChanged(
                taskId: "workflow-a",
                status: .running,
                originSessionId: "session-a", error: nil
            ))
            XCTAssertTrue(source.model.backgroundTasks.isEmpty)

            source.applyForTesting(.taskStatusChanged(
                taskId: "workflow-b",
                status: .running,
                originSessionId: "session-b", error: nil
            ))
            XCTAssertEqual(source.model.backgroundTasks.map(\.id), ["workflow-b"])
        }

        /// r3-failure-paths-07 — a failed task must reach the user as
        /// "<what it was> failed: <why>", never as a bare 9-char id.
        func testFailedTaskNoticeNamesTheAppAndTheReason() {
            let source = makeSource()
            source.applyForTesting(.taskRow(task: row(
                id: "wmo6xnbac",
                status: .running,
                description: "Create app \u{201C}Recipe Box\u{201D}"
            )))

            source.applyForTesting(.taskStatusChanged(
                taskId: "wmo6xnbac",
                status: .failed,
                originSessionId: nil,
                error: "step 2 `build` exited 1"
            ))

            let notices: [ConversationExecutionNotice] = source.model.items.compactMap {
                if case let .notice(notice) = $0 { return notice }
                return nil
            }
            // Vacuity guard: without a notice the two assertions below would
            // pass over an empty collection.
            XCTAssertEqual(notices.count, 1, "the failure must produce exactly one notice")
            XCTAssertTrue(
                notices[0].text.contains("Create app \u{201C}Recipe Box\u{201D}"),
                "the notice names the task, not the id: \(notices[0].text)"
            )
            XCTAssertTrue(
                notices[0].text.contains("step 2 `build` exited 1"),
                "the notice names the reason: \(notices[0].text)"
            )
            XCTAssertFalse(
                notices[0].text.contains("wmo6xnbac"),
                "the bare id is the fallback, not the label: \(notices[0].text)"
            )
            XCTAssertEqual(
                source.model.backgroundTasks.first?.errorText,
                "step 2 `build` exited 1",
                "the panel row keeps the reason for a user who left the session"
            )
        }

        /// The other half of r3-failure-paths-07: a `TaskRow` refresh — the
        /// only surface reached when the push was dropped for a non-active
        /// origin session — still carries the reason onto the panel.
        func testTaskRowBackfillsTheFailureReasonWithoutAPush() {
            let source = makeSource()
            source.applyForTesting(.taskRow(task: row(
                id: "wmo6xnbac",
                status: .failed,
                description: "Create app \u{201C}Recipe Box\u{201D}",
                error: "step 2 `build` exited 1"
            )))
            XCTAssertEqual(
                source.model.backgroundTasks.first?.errorText,
                "step 2 `build` exited 1"
            )
        }

        /// Status transitions update the row in place, and the terminal
        /// transition still appends the completion notice the transcript
        /// shows today.
        func testStatusTransitionUpdatesInPlaceAndKeepsTheTerminalNotice() {
            let source = makeSource()
            source.applyForTesting(.taskRow(task: row(id: "wmo6xnbac", status: .pending)))

            source.applyForTesting(.taskStatusChanged(
                taskId: "wmo6xnbac", status: .running, originSessionId: nil, error: nil
            ))
            XCTAssertEqual(source.model.backgroundTasks.first?.status, .running)

            source.applyForTesting(.taskStatusChanged(
                taskId: "wmo6xnbac", status: .completed, originSessionId: nil, error: nil
            ))
            XCTAssertEqual(source.model.backgroundTasks.first?.status, .completed)
            XCTAssertEqual(source.model.backgroundTasks.count, 1)
            XCTAssertTrue(
                source.model.items.contains { item in
                    // The notice now names the task by its human description
                    // (the `TaskRow` above supplied one), not by the bare id.
                    if case let .notice(notice) = item {
                        return notice.text.contains("build workflow")
                    }
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
                    taskId: taskID, status: terminalWireStatus, originSessionId: nil, error: nil
                ))
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
                    taskId: taskID, status: .running, originSessionId: nil, error: nil
                ))
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
                    queuedAtMs: 1000
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
                    startedAtMs: 1000,
                    lastProgressAtMs: 5000,
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
                    startedAtMs: 1000,
                    lastProgressAtMs: 2000,
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
                    lastProgressAtMs: 5000,
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
                    lastProgressAtMs: 6000,
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
                taskId: "wf-task", status: .completed, originSessionId: nil, error: nil
            ))

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
                    lastProgressAtMs: 3000
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
                    lastProgressAtMs: 3100
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
                    lastProgressAtMs: 1000
                )
            )
            let log = workflowProgress(
                kind: .workflowLog,
                message: "reviewing hierarchy",
                label: "Progress",
                phaseIndex: 0,
                phaseTitle: "Design",
                lastProgressAtMs: 1100
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
                    lastProgressAtMs: 1200
                )
            )

            let workflow = try? XCTUnwrap(source.model.backgroundTasks.first?.workflow)
            XCTAssertEqual(workflow?.logs.count, 1)

            let sections = TasksStatusPanel.workflowSections(for: workflow!, nowMs: 2000)
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
                    lastProgressAtMs: 1000
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
                    lastProgressAtMs: 2000
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
                            updatedAtMs: 1000
                        ),
                    ]
                )
            )

            XCTAssertEqual(
                TasksStatusPanel.workflowCompactSummary(for: task, nowMs: 1000),
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
                            updatedAtMs: 2000
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
                            startedAtMs: 1000,
                            queuedAtMs: 900,
                            lastProgressAtMs: 5000,
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
                            startedAtMs: 1200,
                            queuedAtMs: 1100,
                            lastProgressAtMs: 6000,
                            attempt: 1,
                            lastAttemptReason: nil,
                            tokens: 120,
                            toolCalls: 1,
                            lastToolName: nil,
                            lastToolSummary: nil,
                            promptPreview: nil
                        ),
                    ],
                    lastUpdatedAtMs: 6000
                )
            )

            let summary = TasksStatusPanel.workflowCompactSummary(for: task, nowMs: 6000)
            XCTAssertEqual(
                summary,
                [
                    String(localized: "chat_workflow_agents_done \(1) \(2)"),
                    String(localized: "chat_workflow_agents_running \(1)"),
                    "Generate",
                ].joined(separator: " · ")
            )

            let metrics = TasksStatusPanel.workflowAgentMetrics(task.workflow!.agents[0], nowMs: 6000)
            XCTAssertEqual(
                metrics,
                [
                    String(localized: "chat_status_running"),
                    "gpt-5.4 → gpt-5.4-mini",
                    String(localized: "chat_workflow_tokens \(UInt64(321))"),
                    String(localized: "chat_workflow_tools \(UInt64(4))"),
                    "5s",
                    String(localized: "chat_workflow_retry_reason \(UInt32(2)) \("timeout")"),
                ].joined(separator: " · ")
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

        func testWorkflowStepsFlattenPhasesAndDeriveStatuses() {
            let phases = [
                ConversationWorkflowPhaseSnapshot(
                    id: "phase-0", index: 0, title: "Design", label: nil, message: nil, updatedAtMs: 1
                ),
                ConversationWorkflowPhaseSnapshot(
                    id: "phase-1", index: 1, title: "Build", label: nil, message: nil, updatedAtMs: 2
                ),
                ConversationWorkflowPhaseSnapshot(
                    id: "phase-2", index: 2, title: "Verify", label: nil, message: nil, updatedAtMs: 3
                ),
            ]
            let agents = [
                ConversationWorkflowAgentSnapshot(
                    id: "agent-0", index: 0, title: "designer", message: nil, label: nil,
                    phaseIndex: 0, phaseTitle: "Design", agentId: nil, agentType: nil,
                    model: nil, fallbackModel: nil, state: .done, error: nil, toolUseId: nil,
                    startedAtMs: nil, queuedAtMs: nil, lastProgressAtMs: nil, attempt: nil,
                    lastAttemptReason: nil, tokens: nil, toolCalls: nil, lastToolName: nil,
                    lastToolSummary: nil, promptPreview: nil
                ),
                ConversationWorkflowAgentSnapshot(
                    id: "agent-1", index: 1, title: "builder", message: nil, label: nil,
                    phaseIndex: 1, phaseTitle: "Build", agentId: nil, agentType: nil,
                    model: nil, fallbackModel: nil, state: .progress, error: nil, toolUseId: nil,
                    startedAtMs: nil, queuedAtMs: nil, lastProgressAtMs: nil, attempt: nil,
                    lastAttemptReason: nil, tokens: nil, toolCalls: nil, lastToolName: nil,
                    lastToolSummary: nil, promptPreview: nil
                ),
            ]
            let task = BackgroundTaskSnapshot(
                id: "workflow-1",
                descriptionText: "Build app",
                status: .running,
                workflow: ConversationWorkflowRunSnapshot(
                    taskId: "workflow-1", runId: "run-1", phases: phases, agents: agents
                )
            )

            let steps = TasksStatusPanel.workflowSteps(for: task, nowMs: 2)

            XCTAssertEqual(steps.map(\.title), ["Design", "Build", "Verify"])
            XCTAssertEqual(steps.map(\.state), [.completed, .running, .pending])
            XCTAssertFalse(steps.contains { $0.title == "designer" || $0.title == "builder" })
        }

        func testTerminalTaskStatusOverridesStaleAgentProgress() {
            let phases = [
                ConversationWorkflowPhaseSnapshot(
                    id: "phase-0", index: 0, title: "Design", label: nil, message: nil, updatedAtMs: 1
                ),
                ConversationWorkflowPhaseSnapshot(
                    id: "phase-1", index: 1, title: "Build", label: nil, message: nil, updatedAtMs: 2
                ),
                ConversationWorkflowPhaseSnapshot(
                    id: "phase-2", index: 2, title: "Verify", label: nil, message: nil, updatedAtMs: 3
                ),
            ]
            let staleAgent = ConversationWorkflowAgentSnapshot(
                id: "agent-1", index: 1, title: "builder", message: nil, label: nil,
                phaseIndex: 1, phaseTitle: "Build", agentId: nil, agentType: nil,
                model: nil, fallbackModel: nil, state: .progress, error: nil, toolUseId: nil,
                startedAtMs: nil, queuedAtMs: nil, lastProgressAtMs: nil, attempt: nil,
                lastAttemptReason: nil, tokens: nil, toolCalls: nil, lastToolName: nil,
                lastToolSummary: nil, promptPreview: nil
            )
            let workflow = ConversationWorkflowRunSnapshot(
                taskId: "workflow-1", runId: "run-1", phases: phases, agents: [staleAgent]
            )

            let completed = BackgroundTaskSnapshot(
                id: "workflow-1", descriptionText: "Done", status: .completed, workflow: workflow
            )
            XCTAssertTrue(TasksStatusPanel.workflowTaskIsSuccessfullyComplete(completed))
            XCTAssertFalse(TasksStatusPanel.shouldShowWorkflow([completed]))

            let paused = BackgroundTaskSnapshot(
                id: "workflow-1", descriptionText: "Paused", status: .paused,
                canResume: true, workflow: workflow
            )
            let pausedSteps = TasksStatusPanel.workflowSteps(for: paused)
            XCTAssertEqual(pausedSteps.map(\.state), [.completed, .paused, .pending])
            XCTAssertEqual(pausedSteps.map(\.canResume), [false, true, false])

            let failed = BackgroundTaskSnapshot(
                id: "workflow-1", descriptionText: "Failed", status: .failed, workflow: workflow
            )
            XCTAssertEqual(
                TasksStatusPanel.workflowSteps(for: failed).map(\.state),
                [.completed, .failed, .pending]
            )
            var resumableCreate = failed
            resumableCreate.canResume = true
            XCTAssertEqual(
                TasksStatusPanel.workflowSteps(for: resumableCreate).map(\.canResume),
                [false, true, false],
                "only the failed Create phase offers continuation"
            )

            let cancelled = BackgroundTaskSnapshot(
                id: "workflow-1", descriptionText: "Cancelled", status: .cancelled, workflow: workflow
            )
            XCTAssertEqual(
                TasksStatusPanel.workflowSteps(for: cancelled).map(\.state),
                [.completed, .cancelled, .pending]
            )
        }

        func testSuccessfulGroupsAreHiddenTogether() {
            let completedWorkflow = BackgroundTaskSnapshot(
                id: "workflow-1",
                descriptionText: "Done",
                status: .completed,
                workflow: ConversationWorkflowRunSnapshot(
                    taskId: "workflow-1",
                    runId: "run-1",
                    agents: [
                        ConversationWorkflowAgentSnapshot(
                            id: "agent-0", index: 0, title: nil, message: nil, label: nil,
                            phaseIndex: nil, phaseTitle: nil, agentId: nil, agentType: nil,
                            model: nil, fallbackModel: nil, state: .done, error: nil,
                            toolUseId: nil, startedAtMs: nil, queuedAtMs: nil,
                            lastProgressAtMs: nil, attempt: nil, lastAttemptReason: nil,
                            tokens: nil, toolCalls: nil, lastToolName: nil,
                            lastToolSummary: nil, promptPreview: nil
                        ),
                    ]
                )
            )
            let agents = [
                ConversationAgentSummary.main,
                ConversationAgentSummary(
                    id: "child-1", name: "Child", agentType: "worker", status: "completed"
                ),
            ]
            let todos = [ConversationPlanTask(
                taskId: "todo-1", subject: "Ship", activeForm: nil, state: .completed
            )]

            XCTAssertTrue(TasksStatusPanel.workflowTaskIsSuccessfullyComplete(completedWorkflow))
            XCTAssertTrue(
                ExecutionStatusPanel.visibleGroups(
                    agents: agents,
                    tasks: [completedWorkflow],
                    todos: todos,
                    selectedAgentID: ConversationModel.mainAgentID
                ).isEmpty
            )
        }

        func testAttentionGroupsRemainVisibleAndSelectedChildKeepsAgentList() {
            let failedWorkflow = BackgroundTaskSnapshot(
                id: "workflow-1", descriptionText: "Failed", status: .failed,
                workflow: ConversationWorkflowRunSnapshot(taskId: "workflow-1", runId: "run-1")
            )
            let failedAgent = ConversationAgentSummary(
                id: "child-1", name: "Child", agentType: "worker", status: "failed"
            )
            let todos = [ConversationPlanTask(
                taskId: "todo-1", subject: "Fix", activeForm: nil, state: .inProgress
            )]

            XCTAssertEqual(
                Set(ExecutionStatusPanel.visibleGroups(
                    agents: [ConversationAgentSummary.main, failedAgent],
                    tasks: [failedWorkflow],
                    todos: todos,
                    selectedAgentID: ConversationModel.mainAgentID
                )),
                Set([.agents, .workflow, .todos])
            )

            let completedChild = ConversationAgentSummary(
                id: "child-1", name: "Child", agentType: "worker", status: "completed"
            )
            XCTAssertEqual(
                ExecutionStatusPanel.visibleGroups(
                    agents: [ConversationAgentSummary.main, completedChild],
                    tasks: [],
                    todos: [],
                    selectedAgentID: "child-1"
                ),
                [.agents]
            )
            XCTAssertEqual(
                ExecutionStatusPanel.visibleGroups(
                    agents: [ConversationAgentSummary.main, completedChild],
                    tasks: [],
                    todos: [],
                    selectedAgentID: ConversationModel.mainAgentID
                ),
                []
            )
        }

        func testNonWorkflowTasksDoNotCreateAGroupAndActiveTodosDo() {
            let backgroundTask = BackgroundTaskSnapshot(
                id: "background-1", descriptionText: "Background job", status: .running
            )

            XCTAssertTrue(
                ExecutionStatusPanel.visibleGroups(
                    agents: [], tasks: [backgroundTask], todos: []
                ).isEmpty
            )

            let activeTodo = ConversationPlanTask(
                taskId: "todo-1", subject: "Ship", activeForm: nil, state: .inProgress
            )
            XCTAssertEqual(
                ExecutionStatusPanel.visibleGroups(
                    agents: [], tasks: [], todos: [activeTodo]
                ),
                [.todos]
            )
        }
    }

#endif
