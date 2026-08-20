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
}
