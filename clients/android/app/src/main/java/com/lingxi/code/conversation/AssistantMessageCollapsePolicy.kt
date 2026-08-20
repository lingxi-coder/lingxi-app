package com.lingxi.code.conversation

/**
 * Keeps long streaming replies from taking over the transcript. The character
 * threshold covers prose with few newlines (especially CJK); the line threshold
 * covers logs and generated code made from many short lines.
 */
internal object AssistantMessageCollapsePolicy {
    private const val CHARACTER_LIMIT = 640
    private const val LINE_LIMIT = 20

    /**
     * Lines revealed when a USER bubble is collapsed. 15 lines at 15.5sp with
     * 1.5 line height is ~349dp, which lands next to the assistant branch's
     * 360dp reveal so both sides fold to about the same height.
     */
    const val COLLAPSED_LINE_LIMIT = 15

    fun shouldCollapse(text: String): Boolean =
        text.length >= CHARACTER_LIMIT || text.count { it == '\n' } >= LINE_LIMIT
}
