import XCTest

@testable import LingxiCode

/// The transcript's row identity must not change while a turn is streaming.
/// A tool group grows from one tool to two mid-turn; if that flips the row's
/// `ForEach` id, SwiftUI tears the row down and rebuilds it, which moves the
/// scroll position under the reader.
final class ConversationTimelineSegmentsTests: XCTestCase {
    private func trace(_ id: String) -> ConversationToolTrace {
        ConversationToolTrace(id: id, tool: "Bash", status: .completed)
    }

    private func toolGroup(_ traces: [ConversationToolTrace]) -> ConversationTimelineGroup {
        ConversationTimelineGroup(
            id: "run:R1:tools:\(traces[0].id)",
            runID: "R1",
            rows: traces.map { .tool(runID: "R1", trace: $0) },
            status: .running
        )
    }

    func testToolGroupKeepsItsRowIdentityWhenASecondToolArrives() {
        let oneTool = ConversationTimelineView.segmentIDs(in: toolGroup([trace("t1")]))
        let twoTools = ConversationTimelineView.segmentIDs(in: toolGroup([trace("t1"), trace("t2")]))

        XCTAssertEqual(oneTool.count, 1)
        XCTAssertEqual(twoTools.count, 1)
        XCTAssertEqual(
            oneTool,
            twoTools,
            "A tool group must keep one stable ForEach id as it grows from one tool to two"
        )
    }

    func testTwoToolGroupsInTheSameRunDoNotCollide() {
        let first = ConversationTimelineView.segmentIDs(in: toolGroup([trace("t1"), trace("t2")]))
        let second = ConversationTimelineView.segmentIDs(in: toolGroup([trace("t3"), trace("t4")]))

        XCTAssertNotEqual(first, second)
    }

    func testTimelineChevronKeepsATouchVisibleRestingAffordance() {
        XCTAssertGreaterThan(ConversationTimelineChevronPresentation.opacity(isHighlighted: false), 0)
        XCTAssertGreaterThan(ConversationTimelineChevronPresentation.scale(isHighlighted: false), 0)
    }

    func testTimelineChevronHighlightOnlyStrengthensTheAffordance() {
        XCTAssertGreaterThan(
            ConversationTimelineChevronPresentation.opacity(isHighlighted: true),
            ConversationTimelineChevronPresentation.opacity(isHighlighted: false)
        )
        XCTAssertGreaterThan(
            ConversationTimelineChevronPresentation.scale(isHighlighted: true),
            ConversationTimelineChevronPresentation.scale(isHighlighted: false)
        )
    }

    func testRuntimeFooterMotionPolicyHasAnExactTruthTable() {
        XCTAssertEqual(
            RuntimeFooterMotionPolicy.indicatorPresentation(isAnimated: true, hasIcon: false, reduceMotion: false),
            .hidden
        )
        XCTAssertTrue(
            RuntimeFooterMotionPolicy.textSweepIsActive(
                isAnimated: true,
                reduceMotion: false
            )
        )

        XCTAssertEqual(
            RuntimeFooterMotionPolicy.indicatorPresentation(isAnimated: true, hasIcon: false, reduceMotion: true),
            .activeIndicator
        )
        XCTAssertFalse(
            RuntimeFooterMotionPolicy.textSweepIsActive(
                isAnimated: true,
                reduceMotion: true
            )
        )

        for reduceMotion in [false, true] {
            XCTAssertEqual(
                RuntimeFooterMotionPolicy.indicatorPresentation(
                    isAnimated: false,
                    hasIcon: false,
                    reduceMotion: reduceMotion
                ),
                .staticIndicator
            )
            XCTAssertFalse(
                RuntimeFooterMotionPolicy.textSweepIsActive(
                    isAnimated: false,
                    reduceMotion: reduceMotion
                )
            )
            XCTAssertEqual(
                RuntimeFooterMotionPolicy.indicatorPresentation(
                    isAnimated: true,
                    hasIcon: true,
                    reduceMotion: reduceMotion
                ),
                .hidden
            )
            XCTAssertEqual(
                RuntimeFooterMotionPolicy.textSweepIsActive(
                    isAnimated: true,
                    reduceMotion: reduceMotion
                ),
                !reduceMotion
            )
        }
    }
}
