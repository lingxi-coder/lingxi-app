package com.lingxi.code.conversation

/** Mirrors Desktop runItem.narrationShouldCollapse, including Unicode and hard lines. */
internal object AssistantMessageCollapsePolicy {
    fun shouldCollapse(text: String, isUser: Boolean = false): Boolean {
        val normalized = text.trim().replace("\r\n", "\n").replace('\r', '\n')
        if (normalized.isEmpty()) return false
        val characters = normalized.codePointCount(0, normalized.length)
        val lines = normalized.count { it == '\n' } + 1
        return characters > (if (isUser) 640 else 8_000) ||
            lines > (if (isUser) 8 else 80)
    }
}
