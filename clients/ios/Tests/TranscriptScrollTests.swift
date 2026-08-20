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
        // Nothing to scroll: the reader cannot be anywhere but the bottom.
        XCTAssertTrue(atBottom(offset: 0, content: 300))
    }

    func testRubberBandingPastTheBottomStillFollows() {
        // Overscroll drives the offset past the maximum; that is still "at the
        // bottom", and must not read as the reader detaching.
        XCTAssertTrue(atBottom(offset: 1260, content: 2000))
    }

    func testBottomInsetIsSubtractedFromTheReachableOffset() {
        // A 100pt bottom inset (composer/keyboard) lowers the maximum offset by
        // 100. Sitting at that maximum is still the bottom.
        XCTAssertTrue(
            TranscriptScrollFollowState.isAtBottom(
                contentOffset: 1300,
                contentSize: 2000,
                containerSize: container,
                bottomInset: 100,
                slack: TranscriptScrollFollowState.bottomSlack
            )
        )
    }
}
