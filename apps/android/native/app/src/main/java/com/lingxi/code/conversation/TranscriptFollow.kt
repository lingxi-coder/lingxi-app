package com.lingxi.code.conversation

import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * The single rule for "the reader is parked at the tail".
 *
 * It lives outside the composable because it is the decision a transcript's
 * auto-follow turns on, and the unit tests can pin it without a device.
 */
object TranscriptFollow {
    /**
     * How far above the true end still counts as the tail. Rows are recycled and
     * a streaming row grows every frame, so an exact fit is not something a
     * reader could ever satisfy by hand. The previous "within the last three
     * rows" test was a row count rather than a distance, so it meant wildly
     * different things for short bubbles and for a three-row agent run.
     */
    val BOTTOM_SLACK: Dp = 24.dp

    /**
     * Whether the END of the content sits within [slackPx] of the end of the
     * viewport.
     *
     * Only the final row's end says anything about the end of the content: the
     * last row that happens to be on screen ends at the viewport edge whether or
     * not the transcript does. A final row taller than the viewport is the case
     * that matters most — its top can be pinned to the viewport top with its
     * newest lines still below the fold.
     */
    fun isAtBottom(
        totalItemsCount: Int,
        lastVisibleIndex: Int?,
        lastVisibleEndOffset: Int?,
        viewportEndOffset: Int,
        slackPx: Int,
        canScrollForward: Boolean = true,
        hasBottomAnchor: Boolean = false,
        afterContentPadding: Int = 0,
    ): Boolean {
        if (totalItemsCount == 0) return true
        // A fresh measure pass leaves `visibleItemsInfo` transiently empty even
        // for a reader parked at the end. Reading that as "detached" loses the
        // follow precisely on the updates that arrive with a re-measure, so fall
        // back to the list's own scroll capability, which survives the window.
        if (lastVisibleIndex == null || lastVisibleEndOffset == null) return !canScrollForward
        val finalContentIndex = totalItemsCount - if (hasBottomAnchor) 2 else 1
        if (lastVisibleIndex < finalContentIndex) return false
        // The zero-height anchor may be just outside the viewport while the
        // final real row's end is still inside the near-tail band.
        return lastVisibleEndOffset + afterContentPadding - viewportEndOffset <= slackPx
    }
}

/** Reader intent survives row growth, keyboard resizing and other layout changes. */
internal data class TranscriptFollowState(
    val followsLatest: Boolean = true,
    val pointerDown: Boolean = false,
    val userScrolled: Boolean = false,
    val explicitResume: Boolean = false,
) {
    fun pointerDown() = copy(pointerDown = true)

    fun pointerUp() = copy(pointerDown = false)

    /** Positive nested-scroll Y moves the content down, revealing older messages. */
    fun userScroll(deltaY: Float) = when {
        deltaY > 0 -> copy(followsLatest = false, userScrolled = true, explicitResume = false)
        deltaY < 0 -> copy(userScrolled = true, explicitResume = false)
        else -> this
    }

    fun measured(atBottom: Boolean, scrolling: Boolean): TranscriptFollowState {
        // Geometry alone never changes intent. Only a user's scroll can rejoin
        // the tail; in particular, collapsing a row cannot end history reading.
        if (!userScrolled) return this
        if (pointerDown || scrolling) return this
        return copy(followsLatest = atBottom, userScrolled = false)
    }

    fun resume() = copy(followsLatest = true, userScrolled = false, explicitResume = true)

    fun resumeHandled() = copy(explicitResume = false)

    fun navigateToMessage() = copy(followsLatest = false, userScrolled = false, explicitResume = false)

    fun shouldPin(scrolling: Boolean) = followsLatest && !pointerDown && (!scrolling || explicitResume)
}
