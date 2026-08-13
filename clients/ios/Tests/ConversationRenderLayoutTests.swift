import XCTest
@testable import LingxiCode

final class ConversationRenderLayoutTests: XCTestCase {
    func testRunCardsNeverEnterDurableTranscript() {
        let completed = run(id: "completed", status: .completed)
        let running = run(id: "running", status: .running)
        let message = Message(role: .ai, text: "done")
        let items: [ConversationRenderItem] = [
            .run(completed),
            .message(message),
            .run(running),
        ]

        XCTAssertEqual(
            ConversationRenderLayout.transcriptItems(items),
            [.message(message)]
        )
        XCTAssertEqual(ConversationRenderLayout.pinnedRun(items), running)
    }

    func testTerminalAgentRunIsProjectedToTimelineInsteadOfTranscript() {
        let message = Message(role: .ai, text: "done")
        let completed = run(id: "completed", status: .completed)
        let items: [ConversationRenderItem] = [.message(message), .run(completed)]

        XCTAssertEqual(ConversationRenderLayout.transcriptItems(items), [.message(message)])
        XCTAssertNil(ConversationRenderLayout.pinnedRun(items))
    }

    func testTerminalAgentRunWithAsyncWorkersIsStillPinnedForLegacySurface() {
        let message = Message(role: .ai, text: "delegated")
        var delegated = run(id: "delegated", status: .completed)
        delegated.activeWorkers = 1
        let items: [ConversationRenderItem] = [.message(message), .run(delegated)]

        XCTAssertEqual(ConversationRenderLayout.transcriptItems(items), [.message(message)])
        XCTAssertEqual(ConversationRenderLayout.pinnedRun(items), delegated)
    }

    func testTimelineGroupsKeepReasoningToolsAndNoticesAtBoundaries() {
        var active = run(id: "run-1", status: .running)
        active.reasoning = "inspect workspace"
        active.tools = [tool(id: "read-1", name: "Read"), tool(id: "read-2", name: "Read")]
        active.notices = [ConversationExecutionNotice(id: "notice-1", kind: .info, text: "waiting")]
        let items: [ConversationRenderItem] = [
            .message(Message(role: .ai, text: "before")),
            .run(active),
            .message(Message(role: .ai, text: "after")),
        ]

        let groups = ConversationRenderLayout.timelineGroups(items)
        XCTAssertEqual(groups.count, 5)
        XCTAssertEqual(groups.map(\.runID), [nil, "run-1", "run-1", "run-1", nil])
        XCTAssertEqual(groups[1].rows, [.reasoning(runID: "run-1", activityID: "run:run-1:reasoning", text: "inspect workspace")])
        XCTAssertEqual(groups[1].status, .running)
        XCTAssertTrue(groups[2].isToolGroup)
        XCTAssertEqual(groups[2].rows.count, 2, "continuous tools from one run share one group")
        XCTAssertEqual(groups[2].status, .running)
        XCTAssertEqual(groups[3].rows, [.notice(runID: "run-1", notice: active.notices[0])])
        XCTAssertEqual(groups.map(\.id), [
            items[0].id,
            "run:run-1:reasoning",
            "run:run-1:tools:read-1",
            "run:run-1:notice:notice-1",
            items[2].id,
        ])
    }

    func testGrowingToolBatchKeepsStableGroupIdentity() {
        var active = run(id: "run-batch", status: .running)
        active.tools = [tool(id: "read-1", name: "Read"), tool(id: "read-2", name: "Read")]
        active.activities = [.tool(id: "read-1"), .tool(id: "read-2")]

        let initial = try! XCTUnwrap(ConversationRenderLayout.timelineGroups([.run(active)]).first)
        active.tools.append(tool(id: "read-3", name: "Read"))
        active.activities.append(.tool(id: "read-3"))
        let grown = try! XCTUnwrap(ConversationRenderLayout.timelineGroups([.run(active)]).first)

        XCTAssertEqual(initial.id, grown.id)
        XCTAssertEqual(grown.rows.count, 3)
    }

    func testTimelineGroupIDsStayStableWhenEarlierRowsChange() {
        let active = run(id: "run-1", status: .running)
        let first = ConversationRenderLayout.timelineGroups([.run(active)])
        let second = ConversationRenderLayout.timelineGroups([
            .message(Message(role: .ai, text: "new narrative")),
            .run(active),
        ])

        XCTAssertEqual(first.map(\.id), second.dropFirst().map(\.id))
    }

    func testToolIconUsesHeaderVerbAndLegacyRawToolFallback() {
        XCTAssertEqual(ConversationToolIcon.resolve(verb: .update, tool: "unknown"), .edit)
        XCTAssertEqual(ConversationToolIcon.resolve(verb: .generic, tool: "bash"), .terminal)
        XCTAssertEqual(ConversationToolIcon.resolve(verb: nil, tool: "WebSearch"), .globe)
        XCTAssertEqual(ConversationToolIcon.resolve(verb: nil, tool: "unknown"), .wrench)
        XCTAssertEqual(ConversationToolIcon.resolve(verb: .read, tool: "Read"), .read)
        XCTAssertEqual(ConversationToolIcon.resolve(verb: .search, tool: "Search"), .search)
        XCTAssertEqual(ConversationToolIcon.resolve(verb: .task, tool: "Task"), .workflow)
        XCTAssertEqual(ConversationToolIcon.resolve(verb: .todo, tool: "Todo"), .listChecks)
        XCTAssertEqual(ConversationToolIcon.resolve(verb: .skill, tool: "Skill"), .sparkles)
        XCTAssertEqual(ConversationToolIcon.resolve(verb: .output, tool: "Output"), .output)
        XCTAssertEqual(ConversationToolIcon.resolve(verb: .kill, tool: "Kill"), .stop)
    }

    func testTimelineActivityLedgerPreservesTextBoundaries() {
        let message = Message(role: .ai, text: "between tools")
        var active = run(id: "run-ordered", status: .running)
        active.tools = [tool(id: "one", name: "Read"), tool(id: "two", name: "Edit")]
        active.activities = [
            .reasoning(id: "think", text: "inspect"),
            .tool(id: "one"),
            .textBoundary(id: "text-1", messageID: message.id),
            .tool(id: "two"),
            .notice(id: "done"),
        ]
        active.notices = [ConversationExecutionNotice(id: "done", kind: .info, text: "finished")]

        let groups = ConversationRenderLayout.timelineGroups([.message(message), .run(active)])
        XCTAssertEqual(groups.map(\.id), [
            "think",
            "run:run-ordered:tools:one",
            "message:\(message.id.uuidString)",
            "run:run-ordered:tools:two",
            "run:run-ordered:notice:done",
        ])
        XCTAssertEqual(groups[1].rows.count, 1)
        XCTAssertEqual(groups[3].rows.count, 1)
    }

    func testEveryTerminalOutcomeHasItsOwnVisualTone() {
        let tones = [
            ConversationExecutionStatus.running.tone,
            ConversationExecutionStatus.completed.tone,
            ConversationExecutionStatus.failed.tone,
            ConversationExecutionStatus.cancelled.tone,
            ConversationExecutionStatus.maxTurns.tone,
            ConversationExecutionStatus.restored.tone,
        ]
        XCTAssertEqual(Set(tones).count, tones.count)
    }

    func testOnlyFirstPendingQuestionIsPresentedAsASheet() {
        let first = ConversationPendingQuestion(requestId: 1, questions: [], timeoutSecs: nil)
        let second = ConversationPendingQuestion(requestId: 2, questions: [], timeoutSecs: nil)

        XCTAssertEqual(ConversationRenderLayout.sheetQuestion([first, second]), first)
    }

    private func run(
        id: String,
        status: ConversationExecutionStatus
        ) -> ConversationExecutionRun {
        ConversationExecutionRun(
            id: id,
            sessionId: "session",
            turnId: nil,
            status: status
        )
    }

    private func tool(id: String, name: String) -> ConversationToolTrace {
        ConversationToolTrace(
            id: id,
            tool: name,
            status: .completed,
            inputSummary: nil,
            outputSummary: nil,
            elapsedMs: nil
        )
    }
}
