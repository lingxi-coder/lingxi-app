import XCTest

@testable import LingxiCode

/// The timeline projection walks every transcript item. `ChatView.body` reads it
/// more than once per evaluation and re-evaluates on every streamed token, so it
/// must be built once per mutation, not once per read.
@MainActor
final class ConversationTimelineCacheTests: XCTestCase {
    func testRepeatedReadsRebuildTheProjectionOnlyOnce() {
        let model = ConversationModel()
        model.items = []

        let baseline = model.timelineGroupsRebuildCount
        _ = model.visibleTimelineGroups
        _ = model.visibleTimelineGroups
        _ = model.visibleTimelineGroups

        XCTAssertEqual(
            model.timelineGroupsRebuildCount - baseline,
            1,
            "Three reads with no mutation in between must rebuild the projection once"
        )
    }

    func testMutatingItemsInvalidatesTheProjection() {
        let model = ConversationModel()
        model.items = []
        _ = model.visibleTimelineGroups
        let afterFirstRead = model.timelineGroupsRebuildCount

        model.items = []
        _ = model.visibleTimelineGroups

        XCTAssertEqual(
            model.timelineGroupsRebuildCount - afterFirstRead,
            1,
            "Assigning items must invalidate the memo even when the new value is equal"
        )
    }

    func testSwitchingAgentInvalidatesTheProjection() {
        let model = ConversationModel()
        model.items = []
        _ = model.visibleTimelineGroups
        let afterFirstRead = model.timelineGroupsRebuildCount

        model.selectedAgentID = "child-agent"
        _ = model.visibleTimelineGroups

        XCTAssertEqual(
            model.timelineGroupsRebuildCount - afterFirstRead,
            1,
            "Switching the selected agent changes which items project, so it must invalidate"
        )
    }

    func testUpdatingAgentTranscriptsInvalidatesTheProjection() {
        let model = ConversationModel()
        model.items = []
        model.activeSessionId = "session-1"
        _ = model.visibleTimelineGroups
        let afterFirstRead = model.timelineGroupsRebuildCount

        model.setAgentTranscript(
            "child-agent",
            transcript: ConversationAgentTranscript(),
            sessionID: "session-1"
        )
        _ = model.visibleTimelineGroups

        XCTAssertEqual(
            model.timelineGroupsRebuildCount - afterFirstRead,
            1,
            "Updating a child agent's transcript changes what may project, so it must invalidate"
        )
    }
}
