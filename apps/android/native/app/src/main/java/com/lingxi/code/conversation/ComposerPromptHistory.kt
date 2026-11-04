package com.lingxi.code.conversation

/** Shell-style recall: start empty, continue from an unedited entry at either text boundary. */
internal class ComposerPromptHistory {
    private var entries = emptyList<String>()
    private var index: Int? = null
    private var recalledText: String? = null

    fun reset() {
        entries = emptyList()
        index = null
        recalledText = null
    }

    fun navigate(messages: List<String>, text: String, start: Int, end: Int, older: Boolean): String? {
        if (start != end) return null
        if (text != recalledText) reset()
        if (text.isNotEmpty() && (index == null || (start != 0 && start != text.length))) return null
        if (index == null) {
            if (!older || text.isNotEmpty()) return null
            entries = messages.filter { it.isNotBlank() }
            if (entries.isEmpty()) return null
            index = entries.size
        }
        val next = (index!! + if (older) -1 else 1).coerceIn(0, entries.size)
        if (next == entries.size) {
            reset()
            return ""
        }
        index = next
        return entries[next].also { recalledText = it }
    }
}
