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

    func testDetachedReaderDoesNotResumeBeforeCooldownExpires() {
        var state = TranscriptScrollFollowState()
        let start = Date(timeIntervalSince1970: 100)
        var followsLatest = state.bottomVisibilityChanged(true, followsLatest: true)
        followsLatest = state.dragChanged(
            translationHeight: -24,
            followsLatest: followsLatest,
            now: start
        )
        followsLatest = state.bottomVisibilityChanged(false, followsLatest: followsLatest)
        followsLatest = state.dragEnded(followsLatest: followsLatest, now: start)

        XCTAssertFalse(followsLatest)
        XCTAssertFalse(
            state.autoResumeIfTimedOut(
                followsLatest: followsLatest,
                now: start.addingTimeInterval(29),
                after: TranscriptScrollFollowState.automaticFollowDelay
            )
        )
    }

    func testDetachedReaderResumesOnContentAfterCooldownExpires() {
        var state = TranscriptScrollFollowState()
        let start = Date(timeIntervalSince1970: 100)
        var followsLatest = state.bottomVisibilityChanged(true, followsLatest: true)
        followsLatest = state.dragChanged(
            translationHeight: -24,
            followsLatest: followsLatest,
            now: start
        )
        followsLatest = state.bottomVisibilityChanged(false, followsLatest: followsLatest)
        followsLatest = state.dragEnded(followsLatest: followsLatest, now: start)

        XCTAssertTrue(
            state.autoResumeIfTimedOut(
                followsLatest: followsLatest,
                now: start.addingTimeInterval(30),
                after: TranscriptScrollFollowState.automaticFollowDelay
            )
        )
    }

    func testReturningToBottomCancelsDetachedCooldown() {
        var state = TranscriptScrollFollowState()
        let start = Date(timeIntervalSince1970: 100)
        var followsLatest = state.bottomVisibilityChanged(true, followsLatest: true)
        followsLatest = state.dragChanged(
            translationHeight: -24,
            followsLatest: followsLatest,
            now: start
        )
        followsLatest = state.bottomVisibilityChanged(false, followsLatest: followsLatest)
        followsLatest = state.dragEnded(followsLatest: followsLatest, now: start)
        followsLatest = state.bottomVisibilityChanged(true, followsLatest: followsLatest)

        XCTAssertTrue(followsLatest)
        XCTAssertFalse(
            state.autoResumeIfTimedOut(
                followsLatest: followsLatest,
                now: start.addingTimeInterval(60),
                after: TranscriptScrollFollowState.automaticFollowDelay
            )
        )
    }
}
