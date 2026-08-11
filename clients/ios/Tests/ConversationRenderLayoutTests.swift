import XCTest
@testable import LingxiCode

final class ConversationRenderLayoutTests: XCTestCase {
    func testAgentRunsArePinnedOutsideTheTranscriptAndLatestWins() {
        let first = run(id: "first")
        let second = run(id: "second")
        let message = Message(role: .ai, text: "done")
        let items: [ConversationRenderItem] = [
            .run(first),
            .message(message),
            .run(second),
        ]

        XCTAssertEqual(
            ConversationRenderLayout.transcriptItems(items),
            [.run(first), .message(message)]
        )
        XCTAssertEqual(ConversationRenderLayout.pinnedRun(items), second)
    }

    func testOnlyFirstPendingQuestionIsPresentedAsASheet() {
        let first = ConversationPendingQuestion(requestId: 1, questions: [], timeoutSecs: nil)
        let second = ConversationPendingQuestion(requestId: 2, questions: [], timeoutSecs: nil)

        XCTAssertEqual(ConversationRenderLayout.sheetQuestion([first, second]), first)
    }

    private func run(id: String) -> ConversationExecutionRun {
        ConversationExecutionRun(
            id: id,
            sessionId: "session",
            turnId: nil,
            status: .running
        )
    }
}
