import XCTest

@testable import LingxiCode

/// "Is the reader parked at the bottom" is a measurement, not an inference. The
/// old implementation guessed it from a 1pt marker's onAppear/onDisappear inside
/// a lazy stack and from drag direction, and needed a 30-second auto-resume to
/// recover from its own false negatives.
final class TranscriptScrollTests: XCTestCase {
    private let container: CGFloat = 800

    private func atBottom(offset: CGFloat, content: CGFloat) -> Bool {
        TranscriptScrollFollowState.isAtBottom(
            contentOffset: offset,
            contentSize: content,
            containerSize: container,
            bottomInset: 0,
            slack: TranscriptScrollFollowState.bottomSlack
        )
    }

    func testExactlyAtTheBottomFollows() {
        XCTAssertTrue(atBottom(offset: 1200, content: 2000))
    }

    func testWithinSlackOfTheBottomStillFollows() {
        // 8pt short of the bottom: a rounding artefact or a growing row that has
        // not been scrolled to yet, not a reader who scrolled away.
        XCTAssertTrue(atBottom(offset: 1192, content: 2000))
    }

    func testScrolledWellAwayFromTheBottomDetaches() {
        XCTAssertFalse(atBottom(offset: 400, content: 2000))
    }

    func testContentShorterThanTheContainerAlwaysFollows() {
        // Overscrolled upward on a transcript too short to scroll. The
        // `maximumOffset > 0` guard is what makes this true; without it the
        // comparison against a NEGATIVE maximum answers false and auto-follow
        // silently switches off on short conversations.
        XCTAssertTrue(atBottom(offset: -600, content: 300))
    }

    func testRubberBandingPastTheBottomStillFollows() {
        // Overscroll drives the offset past the maximum; that is still "at the
        // bottom", and must not read as the reader detaching.
        XCTAssertTrue(atBottom(offset: 1260, content: 2000))
    }

    func testBottomInsetRaisesTheReachableMaximumOffset() {
        // A 100pt bottom inset (composer/keyboard) raises the maximum offset to
        // 1300. Sitting at that maximum is the bottom...
        XCTAssertTrue(
            TranscriptScrollFollowState.isAtBottom(
                contentOffset: 1300,
                contentSize: 2000,
                containerSize: container,
                bottomInset: 100,
                slack: TranscriptScrollFollowState.bottomSlack
            )
        )
        // ...and 100pt above it, well outside the slack, is not. This second
        // case is what discriminates the sign: an implementation that SUBTRACTED
        // the inset would put the threshold at 1076 and wrongly answer true.
        XCTAssertFalse(
            TranscriptScrollFollowState.isAtBottom(
                contentOffset: 1200,
                contentSize: 2000,
                containerSize: container,
                bottomInset: 100,
                slack: TranscriptScrollFollowState.bottomSlack
            )
        )
    }
}
