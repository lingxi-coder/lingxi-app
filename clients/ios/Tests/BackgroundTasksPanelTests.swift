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
    }

#endif
