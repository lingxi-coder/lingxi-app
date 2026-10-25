import SwiftUI
import XCTest

@testable import LingxiCode

/// "Is the reader parked at the bottom" is a measurement, not an inference. The
/// old implementation guessed it from a 1pt marker's onAppear/onDisappear inside
/// a lazy stack and from drag direction, and needed a 30-second auto-resume to
/// recover from its own false negatives.
///
/// A later implementation replaced that guess with a reconstruction —
/// `contentSize - containerSize + bottomInset` — that looked measured but
/// wasn't: device measurement (iPhone 11, iOS 18) showed `containerSize`
/// EXCLUDES the content insets while `visibleRect` includes them, and that
/// this surface's only inset is `contentInsets.top`, never `.bottom`. The
/// reconstructed maximum came out 92pt too high, and the predicate answered
/// false in every hold at the real bottom. These tests pin the CURRENT
/// contract — comparing `contentSize` directly against `visibleMaxY` — so a
/// future edit can't silently reintroduce a reconstructed maximum.
final class TranscriptScrollTests: XCTestCase {
    private let slack = TranscriptScrollFollowState.bottomSlack

    private func atBottom(contentSize: CGFloat, visibleMaxY: CGFloat) -> Bool {
        TranscriptScrollFollowState.isAtBottom(
            contentSize: contentSize,
            visibleMaxY: visibleMaxY,
            slack: slack
        )
    }

    func testExactlyAtTheBottomFollows() {
        // gap == 0.
        XCTAssertTrue(atBottom(contentSize: 2000, visibleMaxY: 2000))
    }

    func testWithinSlackOfTheBottomStillFollows() {
        // gap == 20, inside the 24pt slack.
        XCTAssertTrue(atBottom(contentSize: 2000, visibleMaxY: 1980))
    }

    func testBeyondSlackDetaches() {
        // gap == 100, well outside the slack. This is what discriminates a
        // reversed subtraction: `visibleMaxY - contentSize <= slack` would
        // evaluate -100 <= 24 and wrongly answer true here.
        XCTAssertFalse(atBottom(contentSize: 2000, visibleMaxY: 1900))
    }

    func testRubberBandingPastTheBottomStillFollows() {
        // gap == -100: overscroll drove visibleMaxY past contentSize. Still
        // "at the bottom" — must not read as the reader detaching.
        XCTAssertTrue(atBottom(contentSize: 2000, visibleMaxY: 2100))
    }

    func testContentTooShortToScrollFollows() {
        // Content and visible bottom coincide because there is nothing to
        // scroll, not because the reader is pinned. Still "at the bottom".
        XCTAssertTrue(atBottom(contentSize: 300, visibleMaxY: 300))
    }

    func testJustOutsideSlackDetaches() {
        // gap == 24.1, one tenth of a point past the slack boundary. This
        // discriminates a dropped-slack implementation
        // (`contentSize - visibleMaxY <= 0`), which would already read false
        // well before this point, and a boundary that used `<` instead of
        // `<=` no differently — this case is strictly beyond either boundary.
        XCTAssertFalse(atBottom(contentSize: 2000, visibleMaxY: 1975.9))
    }

    private func metrics(content: CGFloat = 2000, visibleBottom: CGFloat = 2000, height: CGFloat = 600) -> TranscriptScrollMetrics {
        TranscriptScrollMetrics(ScrollGeometry(
            contentOffset: CGPoint(x: 0, y: visibleBottom - height),
            contentSize: CGSize(width: 390, height: content),
            contentInsets: .init(),
            containerSize: CGSize(width: 390, height: height)
        ))
    }

    func testLargeContentGrowthKeepsFollowingAndRequestsCorrection() {
        var state = TranscriptScrollFollowState()
        XCTAssertTrue(state.geometryChanged(from: metrics(), to: metrics(content: 2500)))
        XCTAssertTrue(state.followsLatest)
        XCTAssertTrue(state.canFollow)
    }

    func testKeyboardAndViewportResizeKeepsAttachedReaderAtTail() {
        var state = TranscriptScrollFollowState()
        let keyboardOpen = metrics(visibleBottom: 1700, height: 300)
        XCTAssertTrue(state.geometryChanged(from: metrics(), to: keyboardOpen))
        XCTAssertTrue(state.geometryChanged(from: keyboardOpen, to: metrics()))
        XCTAssertTrue(state.canFollow)
    }

    func testDetachedReaderNeverRejoinsFromContentOrViewportLayout() {
        var state = TranscriptScrollFollowState()
        state.requestRow()
        XCTAssertFalse(state.geometryChanged(from: metrics(visibleBottom: 1200), to: metrics(content: 2500, visibleBottom: 1200)))
        XCTAssertFalse(state.geometryChanged(from: metrics(visibleBottom: 1200), to: metrics(content: 1000, visibleBottom: 1200)))
        XCTAssertFalse(state.geometryChanged(from: metrics(visibleBottom: 1200), to: metrics(height: 300)))
        XCTAssertFalse(state.followsLatest)
    }

    func testTouchPausesFollowWithoutMistakingConcurrentGrowthForAScroll() {
        var state = TranscriptScrollFollowState()
        state.phaseChanged(to: .tracking, isAtBottom: true)
        XCTAssertFalse(state.canFollow)
        XCTAssertFalse(state.geometryChanged(from: metrics(), to: metrics(content: 2500)))
        state.phaseChanged(to: .idle, isAtBottom: false)
        XCTAssertTrue(state.followsLatest)
        XCTAssertTrue(state.canFollow)
    }

    func testUserDragDetachesAndMomentumDoesNotFightReader() {
        var state = TranscriptScrollFollowState()
        state.phaseChanged(to: .interacting, isAtBottom: true)
        XCTAssertFalse(state.geometryChanged(from: metrics(), to: metrics(visibleBottom: 1800)))
        XCTAssertFalse(state.followsLatest)
        state.phaseChanged(to: .decelerating, isAtBottom: false)
        XCTAssertFalse(state.canFollow)
        state.phaseChanged(to: .idle, isAtBottom: false)
        XCTAssertFalse(state.canFollow)
    }

    func testUserReturnsWithinSlackAndRejoinsWhenGestureEnds() {
        var state = TranscriptScrollFollowState()
        state.requestRow()
        state.phaseChanged(to: .interacting, isAtBottom: false)
        XCTAssertFalse(state.geometryChanged(from: metrics(visibleBottom: 1200), to: metrics(visibleBottom: 1980)))
        XCTAssertTrue(state.followsLatest)
        XCTAssertFalse(state.canFollow)
        state.phaseChanged(to: .idle, isAtBottom: true)
        XCTAssertTrue(state.canFollow)
    }

    func testLayoutWhileDraggingDoesNotPretendToBeUserOffsetChange() {
        var state = TranscriptScrollFollowState()
        state.phaseChanged(to: .interacting, isAtBottom: true)
        XCTAssertFalse(state.geometryChanged(from: metrics(), to: metrics(content: 2500, visibleBottom: 1900)))
        XCTAssertTrue(state.followsLatest)
        state.phaseChanged(to: .idle, isAtBottom: false)
        XCTAssertFalse(state.followsLatest)
    }

    func testProgrammaticNavigationStaysDetachedEvenAtBottom() {
        var state = TranscriptScrollFollowState()
        state.requestRow()
        state.phaseChanged(to: .animating, isAtBottom: false)
        XCTAssertFalse(state.geometryChanged(from: metrics(visibleBottom: 1000), to: metrics()))
        state.phaseChanged(to: .idle, isAtBottom: true)
        XCTAssertFalse(state.followsLatest)
    }

    func testAcceptedSendRejoinsWithoutWaitingForContentChange() {
        var state = TranscriptScrollFollowState()
        state.requestRow()
        state.requestLatest()
        XCTAssertTrue(state.canFollow)
        XCTAssertFalse(state.geometryChanged(from: metrics(), to: metrics()))
    }

    func testMessageTargetWinsOverNewScopeAndPreviouslyQueuedFollow() {
        var state = TranscriptScrollFollowState()
        XCTAssertTrue(state.canFollow)
        state.requestRow()
        XCTAssertFalse(state.canFollow)
        state = TranscriptScrollFollowState()
        state.requestRow()
        XCTAssertFalse(state.canFollow)
    }


    func testAcceptedSendDuringDecelerationWinsOverOldOffBottomGeometry() {
        var state = TranscriptScrollFollowState()
        state.phaseChanged(to: .interacting, isAtBottom: true)
        state.phaseChanged(to: .decelerating, isAtBottom: false)
        state.requestLatest()
        XCTAssertFalse(state.canFollow)
        XCTAssertTrue(state.canApplyExplicitRequest)
        XCTAssertFalse(state.geometryChanged(from: metrics(visibleBottom: 1800), to: metrics(visibleBottom: 1600)))
        state.phaseChanged(to: .idle, isAtBottom: false)
        XCTAssertTrue(state.canFollow)
    }

    func testRowNavigationDuringDecelerationDoesNotRejoinAtOldTail() {
        var state = TranscriptScrollFollowState()
        state.phaseChanged(to: .decelerating, isAtBottom: false)
        state.requestRow()
        XCTAssertTrue(state.canApplyExplicitRequest)
        XCTAssertFalse(state.geometryChanged(from: metrics(visibleBottom: 1800), to: metrics()))
        state.phaseChanged(to: .idle, isAtBottom: true)
        XCTAssertFalse(state.canFollow)
    }

    func testNewGestureCanSupersedeAnExplicitSend() {
        var state = TranscriptScrollFollowState()
        state.phaseChanged(to: .decelerating, isAtBottom: false)
        state.requestLatest()
        state.phaseChanged(to: .tracking, isAtBottom: false)
        state.phaseChanged(to: .interacting, isAtBottom: false)
        state.phaseChanged(to: .idle, isAtBottom: false)
        XCTAssertFalse(state.canFollow)
    }


    func testExplicitRequestWaitsWhileFingerIsDownThenCanInterruptMomentum() {
        var state = TranscriptScrollFollowState()
        state.phaseChanged(to: .tracking, isAtBottom: false)
        state.requestLatest()
        XCTAssertFalse(state.canApplyExplicitRequest)
        state.phaseChanged(to: .interacting, isAtBottom: false)
        XCTAssertFalse(state.canApplyExplicitRequest)
        state.phaseChanged(to: .decelerating, isAtBottom: false)
        XCTAssertTrue(state.canApplyExplicitRequest)
    }

    func testDisappearanceClearsInteractionWithoutChangingReadingIntent() {
        var state = TranscriptScrollFollowState()
        state.requestRow()
        state.phaseChanged(to: .interacting, isAtBottom: false)
        state.suspendInteraction()
        XCTAssertFalse(state.followsLatest)
        XCTAssertFalse(state.isUserInteracting)
    }

}
