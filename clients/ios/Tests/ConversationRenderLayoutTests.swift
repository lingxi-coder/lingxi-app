import XCTest
@testable import LingxiCode

final class ConversationRenderLayoutTests: XCTestCase {
    func testOnlyTheLatestRunningAgentRunIsPinnedOutsideTheTranscript() {
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
            [.run(completed), .message(message)]
        )
        XCTAssertEqual(ConversationRenderLayout.pinnedRun(items), running)
    }

    func testTerminalAgentRunStaysInTranscriptAndIsNotPinned() {
        let message = Message(role: .ai, text: "done")
        let completed = run(id: "completed", status: .completed)
        let items: [ConversationRenderItem] = [.message(message), .run(completed)]

        XCTAssertEqual(ConversationRenderLayout.transcriptItems(items), items)
        XCTAssertNil(ConversationRenderLayout.pinnedRun(items))
    }

    func testTerminalAgentRunWithAsyncWorkersRemainsPinnedUntilIdle() {
        let message = Message(role: .ai, text: "delegated")
        var delegated = run(id: "delegated", status: .completed)
        delegated.activeWorkers = 1
        let items: [ConversationRenderItem] = [.message(message), .run(delegated)]

        XCTAssertEqual(ConversationRenderLayout.transcriptItems(items), [.message(message)])
        XCTAssertEqual(ConversationRenderLayout.pinnedRun(items), delegated)
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
}
