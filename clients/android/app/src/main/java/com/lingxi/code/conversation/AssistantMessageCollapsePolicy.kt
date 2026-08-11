package com.lingxi.code.conversation

/**
 * Keeps long streaming replies from taking over the transcript. The character
 * threshold covers prose with few newlines (especially CJK); the line threshold
 * covers logs and generated code made from many short lines.
 */
internal object AssistantMessageCollapsePolicy {
    private const val CHARACTER_LIMIT = 640
    private const val LINE_LIMIT = 20

    fun shouldCollapse(text: String): Boolean =
        text.length >= CHARACTER_LIMIT || text.count { it == '\n' } >= LINE_LIMIT
}
