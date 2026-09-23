package com.lingxi.code.conversation

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The transcript's auto-follow turns on exactly this predicate, and getting it
 * wrong either drags a reader away from what they are reading or strands them
 * away from the newest output. The cases below are the ones the previous
 * row-count heuristic got wrong.
 */
class TranscriptFollowTest {
    private val slack = 24
    private val viewportEnd = 800

    private fun atBottom(
        totalItemsCount: Int,
        lastVisibleIndex: Int?,
        lastVisibleEndOffset: Int?,
        canScrollForward: Boolean = true,
    ) = TranscriptFollow.isAtBottom(
        totalItemsCount = totalItemsCount,
        lastVisibleIndex = lastVisibleIndex,
        lastVisibleEndOffset = lastVisibleEndOffset,
        viewportEndOffset = viewportEnd,
        slackPx = slack,
        canScrollForward = canScrollForward,
    )

    @Test fun emptyTranscriptCountsAsAtTheTail() {
        assertTrue(atBottom(totalItemsCount = 0, lastVisibleIndex = null, lastVisibleEndOffset = null))
    }

    @Test fun finalRowEndingAtTheViewportEdgeIsAtTheTail() {
        assertTrue(atBottom(totalItemsCount = 5, lastVisibleIndex = 4, lastVisibleEndOffset = viewportEnd))
    }

    @Test fun finalRowInsideTheBandIsAtTheTail() {
        assertTrue(atBottom(totalItemsCount = 5, lastVisibleIndex = 4, lastVisibleEndOffset = viewportEnd - slack))
    }

    @Test fun finalRowOnePixelBeyondTheBandIsDetached() {
        // The final row is on screen but its end sits past the viewport edge by
        // more than the band: the newest lines are below the fold.
        assertFalse(atBottom(totalItemsCount = 5, lastVisibleIndex = 4, lastVisibleEndOffset = viewportEnd + slack + 1))
    }

    @Test fun finalRowEndingExactlyOneBandBelowTheEdgeIsStillAtTheTail() {
        assertTrue(atBottom(totalItemsCount = 5, lastVisibleIndex = 4, lastVisibleEndOffset = viewportEnd + slack))
    }

    @Test fun aVisibleMiddleRowEndingAtTheEdgeIsNotTheTail() {
        // The last row ON SCREEN always ends at the viewport edge, whether or
        // not the transcript does. Only the final row speaks for the content.
        assertFalse(atBottom(totalItemsCount = 9, lastVisibleIndex = 6, lastVisibleEndOffset = viewportEnd))
    }

    @Test fun aFinalRowTallerThanTheViewportIsNotTheTail() {
        // Its top is aligned to the viewport top and every line it streams is
        // still below the fold. The old "within the last three rows" test called
        // this the bottom and left the newest output off screen.
        assertFalse(atBottom(totalItemsCount = 4, lastVisibleIndex = 3, lastVisibleEndOffset = 2000))
    }

    @Test fun contentTooShortToScrollIsAtTheTail() {
        // The end of the content sits above the end of the viewport, so the gap
        // is negative and falls out of the comparison with no special case.
        assertTrue(atBottom(totalItemsCount = 2, lastVisibleIndex = 1, lastVisibleEndOffset = 300))
    }

    @Test fun anUnmeasuredWindowAtTheEndIsStillTheTail() {
        // A fresh measure pass empties `visibleItemsInfo` even for a reader
        // parked at the end. Reporting "detached" there loses the follow on
        // exactly the updates that arrive with a re-measure.
        assertTrue(
            atBottom(totalItemsCount = 3, lastVisibleIndex = null, lastVisibleEndOffset = null, canScrollForward = false),
        )
    }

    @Test fun anUnmeasuredWindowMidHistoryIsDetached() {
        assertFalse(
            atBottom(totalItemsCount = 3, lastVisibleIndex = null, lastVisibleEndOffset = null, canScrollForward = true),
        )
    }
}
