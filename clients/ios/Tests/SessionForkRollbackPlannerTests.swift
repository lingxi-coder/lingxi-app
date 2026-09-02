import XCTest

@testable import LingxiCode

@MainActor
final class SessionForkRollbackPlannerTests: XCTestCase {
    func testRollbackRequestRestoresOriginWhenForkMovedToAnotherWorkspace() {
        let origin = RootView.ConversationSelectionSnapshot(
            scope: .project("origin"),
            mode: .chat,
            activeSession: "session-origin",
            confirmedSession: "session-origin",
            pendingRestoreID: nil,
            draft: "draft-origin"
        )

        let request = RootView.SessionForkRollbackPlanner.rollbackRequest(
            origin: origin,
            sourceScope: .project("source"),
            sourceMode: .code,
            sourceSessionID: "session-source"
        )

        XCTAssertEqual(request?.snapshot, origin)
        XCTAssertEqual(request?.snapshot.resumeSessionID, "session-origin")
        XCTAssertFalse(request?.snapshot.startsNewConversation ?? true)
    }

    func testRollbackRequestRestoresBlankOriginAsNewConversation() {
        let origin = RootView.ConversationSelectionSnapshot(
            scope: .global,
            mode: .code,
            activeSession: "",
            confirmedSession: "",
            pendingRestoreID: nil,
            draft: "unsent draft"
        )

        let request = RootView.SessionForkRollbackPlanner.rollbackRequest(
            origin: origin,
            sourceScope: .project("source"),
            sourceMode: .code,
            sourceSessionID: "session-source"
        )

        XCTAssertEqual(request?.snapshot, origin)
        XCTAssertNil(request?.snapshot.resumeSessionID)
        XCTAssertTrue(request?.snapshot.startsNewConversation ?? false)
    }

    func testRollbackRequestRestoresOriginWhenOnlySessionChangedInSameWorkspace() {
        let origin = RootView.ConversationSelectionSnapshot(
            scope: .project("same"),
            mode: .code,
            activeSession: "session-origin",
            confirmedSession: "session-origin",
            pendingRestoreID: nil,
            draft: "draft"
        )

        let request = RootView.SessionForkRollbackPlanner.rollbackRequest(
            origin: origin,
            sourceScope: .project("same"),
            sourceMode: .code,
            sourceSessionID: "session-source"
        )

        XCTAssertEqual(request?.snapshot, origin)
        XCTAssertEqual(request?.snapshot.resumeSessionID, "session-origin")
        XCTAssertFalse(request?.snapshot.startsNewConversation ?? true)
    }

    func testRollbackRequestIsNilWhenForkNeverChangedVisibleSelection() {
        let origin = RootView.ConversationSelectionSnapshot(
            scope: .project("same"),
            mode: .code,
            activeSession: "session-a",
            confirmedSession: "session-a",
            pendingRestoreID: nil,
            draft: "draft"
        )

        let request = RootView.SessionForkRollbackPlanner.rollbackRequest(
            origin: origin,
            sourceScope: .project("same"),
            sourceMode: .code,
            sourceSessionID: "session-a"
        )

        XCTAssertNil(request)
    }
}
