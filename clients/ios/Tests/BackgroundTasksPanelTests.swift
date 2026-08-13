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

        /// A status push for an id the panel has never seen inserts a
        /// placeholder row (bare id) instead of being dropped; the later
        /// `TaskRow` backfills the description WITHOUT resetting the newer
        /// status.
        func testPushFirstIdGetsAPlaceholderThenBackfills() {
            let source = makeSource()

            source.applyForTesting(.taskStatusChanged(taskId: "wmo6xnbac", status: .running))
            XCTAssertEqual(source.model.backgroundTasks.map(\.id), ["wmo6xnbac"])
            XCTAssertEqual(source.model.backgroundTasks.first?.descriptionText, "")

            source.applyForTesting(.taskRow(task: row(id: "wmo6xnbac", status: .running)))
            XCTAssertEqual(source.model.backgroundTasks.count, 1, "row upserts, never duplicates")
            XCTAssertEqual(
                source.model.backgroundTasks.first?.descriptionText,
                "local-app-build workflow"
            )
        }

        /// Status transitions update the row in place, and the terminal
        /// transition still appends the completion notice the transcript
        /// shows today.
        func testStatusTransitionUpdatesInPlaceAndKeepsTheTerminalNotice() {
            let source = makeSource()
            source.applyForTesting(.taskRow(task: row(id: "wmo6xnbac", status: .pending)))

            source.applyForTesting(.taskStatusChanged(taskId: "wmo6xnbac", status: .running))
            XCTAssertEqual(source.model.backgroundTasks.first?.status, .running)

            source.applyForTesting(.taskStatusChanged(taskId: "wmo6xnbac", status: .completed))
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

                source.applyForTesting(.taskStatusChanged(taskId: taskID, status: terminalWireStatus))
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

                source.applyForTesting(.taskStatusChanged(taskId: taskID, status: .running))
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
    }

#endif
