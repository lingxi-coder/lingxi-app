import XCTest

@testable import LingxiCode

final class TranscriptScrollTests: XCTestCase {
    func testTinyUpwardDragKeepsFollowingWhenBottomStaysVisible() {
        var state = TranscriptScrollFollowState()
        var followsLatest = false

        followsLatest = state.bottomVisibilityChanged(true, followsLatest: followsLatest)
        followsLatest = state.dragChanged(translationHeight: -12, followsLatest: followsLatest)

        XCTAssertFalse(followsLatest)

        followsLatest = state.dragEnded(followsLatest: followsLatest)

        XCTAssertTrue(followsLatest)
    }

    func testUpwardDragStaysDetachedWhenBottomIsNoLongerVisible() {
        var state = TranscriptScrollFollowState()
        var followsLatest = false

        followsLatest = state.bottomVisibilityChanged(true, followsLatest: followsLatest)
        followsLatest = state.dragChanged(translationHeight: -24, followsLatest: followsLatest)
        followsLatest = state.bottomVisibilityChanged(false, followsLatest: followsLatest)
        followsLatest = state.dragEnded(followsLatest: followsLatest)

        XCTAssertFalse(followsLatest)
    }

    func testBottomVisibilityRearmsFollowing() {
        var state = TranscriptScrollFollowState()

        let followsLatest = state.bottomVisibilityChanged(false, followsLatest: false)
        let rearmed = state.bottomVisibilityChanged(true, followsLatest: followsLatest)

        XCTAssertTrue(rearmed)
    }
}
