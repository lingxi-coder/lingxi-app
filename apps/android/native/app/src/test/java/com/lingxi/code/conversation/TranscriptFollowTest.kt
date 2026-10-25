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

    @Test fun offscreenZeroHeightAnchorStillAllowsRejoiningInsideTheTailBand() {
        assertTrue(TranscriptFollow.isAtBottom(
            totalItemsCount = 6, lastVisibleIndex = 4, lastVisibleEndOffset = viewportEnd + 16,
            viewportEndOffset = viewportEnd, slackPx = slack, hasBottomAnchor = true,
            afterContentPadding = 8,
        ))
        assertFalse(TranscriptFollow.isAtBottom(
            totalItemsCount = 6, lastVisibleIndex = 4, lastVisibleEndOffset = viewportEnd + 17,
            viewportEndOffset = viewportEnd, slackPx = slack, hasBottomAnchor = true,
            afterContentPadding = 8,
        ))
    }

    @Test fun streamingGrowthAndKeyboardResizePreserveFollowingIntent() {
        val follow = TranscriptFollowState()
        assertTrue(follow.measured(atBottom = false, scrolling = false).shouldPin(scrolling = false))
        assertTrue(follow.measured(atBottom = true, scrolling = false).followsLatest)
    }

    @Test fun layoutGrowthAndCollapseNeverReattachAHistoryReader() {
        val reading = TranscriptFollowState().navigateToMessage()
        assertFalse(reading.measured(atBottom = false, scrolling = false).followsLatest)
        assertFalse(reading.measured(atBottom = true, scrolling = false).followsLatest)
    }

    @Test fun holdingTheTranscriptSuspendsCorrectionsWithoutDetaching() {
        val held = TranscriptFollowState().pointerDown()
        assertTrue(held.followsLatest)
        assertFalse(held.shouldPin(scrolling = false))
        assertTrue(held.pointerUp().shouldPin(scrolling = false))
    }

    @Test fun scrollingTowardHistoryCancelsFollowingThroughTheFling() {
        val dragging = TranscriptFollowState().pointerDown().userScroll(deltaY = 80f)
        assertFalse(dragging.followsLatest)
        val flinging = dragging.pointerUp().measured(atBottom = false, scrolling = true)
        assertFalse(flinging.shouldPin(scrolling = true))
        val stopped = flinging.measured(atBottom = false, scrolling = false)
        assertFalse(stopped.followsLatest)
        assertFalse(stopped.measured(atBottom = true, scrolling = false).followsLatest)
    }

    @Test fun scrollingBackIntoTheTailBandRejoinsAfterInteraction() {
        val dragging = TranscriptFollowState().navigateToMessage().pointerDown().userScroll(deltaY = -80f)
        assertFalse(dragging.measured(atBottom = true, scrolling = true).shouldPin(scrolling = true))
        val stopped = dragging.pointerUp().measured(atBottom = true, scrolling = false)
        assertTrue(stopped.shouldPin(scrolling = false))
    }

    @Test fun sendAndJumpResumeEvenWhenTheLayoutHasNotChanged() {
        val reading = TranscriptFollowState().navigateToMessage()
        assertTrue(reading.resume().shouldPin(scrolling = false))
        assertFalse(reading.resume().navigateToMessage().followsLatest)
    }

    @Test fun sendDuringMomentumCannotBeUndoneByTheOlderGestureSettling() {
        val flinging = TranscriptFollowState().userScroll(80f)
        val sent = flinging.resume().measured(atBottom = false, scrolling = true)
        assertTrue(sent.followsLatest)
        assertTrue(sent.shouldPin(scrolling = true))
        assertFalse(sent.resumeHandled().shouldPin(scrolling = true))
        assertTrue(sent.measured(atBottom = false, scrolling = false).shouldPin(scrolling = false))
    }

    @Test fun freshFingerDragCancelsAnExplicitJumpBeforeItExecutes() {
        val requested = TranscriptFollowState().navigateToMessage().resume().pointerDown()
        assertFalse(requested.shouldPin(scrolling = true))
        val reading = requested.userScroll(40f).pointerUp()
        assertFalse(reading.shouldPin(scrolling = false))
        assertFalse(reading.explicitResume)
    }

    @Test fun rejectedSendDoesNotRearmFollow() {
        var follow = TranscriptFollowState().navigateToMessage()
        sendComposerIfAdmitted(
            admitted = false,
            send = { error("Rejected send must not run") },
            clearDraft = { error("Rejected draft must survive") },
            removeAttachment = { error("Rejected attachment must survive") },
            followTail = { follow = follow.resume() },
        )
        assertFalse(follow.followsLatest)
    }

    @Test fun navigationDuringTouchDoesNotRejoinWhenThePointerLifts() {
        val linked = TranscriptFollowState().pointerDown().userScroll(-20f).navigateToMessage()
        assertFalse(linked.pointerUp().measured(atBottom = true, scrolling = false).followsLatest)
    }
}
