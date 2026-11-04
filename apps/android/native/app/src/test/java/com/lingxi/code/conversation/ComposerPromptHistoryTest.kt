package com.lingxi.code.conversation

import org.junit.Assert.*
import org.junit.Test

class ComposerPromptHistoryTest {
    private val messages = listOf("oldest", "line one\nline two", "latest", "")

    @Test fun recallsBothDirectionsAndClearsAfterNewest() {
        val history = ComposerPromptHistory()
        assertEquals("latest", history.navigate(messages, "", 0, 0, true))
        assertEquals("line one\nline two", history.navigate(messages, "latest", 6, 6, true))
        assertEquals("oldest", history.navigate(messages, "line one\nline two", 0, 0, true))
        assertEquals("oldest", history.navigate(messages, "oldest", 6, 6, true))
        assertEquals("line one\nline two", history.navigate(messages, "oldest", 6, 6, false))
        assertEquals("latest", history.navigate(messages, "line one\nline two", 17, 17, false))
        assertEquals("", history.navigate(messages, "latest", 6, 6, false))
        assertNull(history.navigate(messages, "", 0, 0, false))
    }

    @Test fun protectsDraftSelectionAndInteriorCursorAndResetsAfterEdit() {
        val history = ComposerPromptHistory()
        assertNull(history.navigate(messages, "draft", 0, 0, true))
        assertNull(history.navigate(messages, "", 0, 0, false))
        assertEquals("latest", history.navigate(messages, "", 0, 0, true))
        assertNull(history.navigate(messages, "latest", 2, 2, true))
        assertNull(history.navigate(messages, "latest", 0, 6, true))
        assertNull(history.navigate(messages, "latest edited", 0, 0, true))
        assertNull(history.navigate(messages, "latest", 0, 0, true))
        assertEquals("latest", history.navigate(messages, "", 0, 0, true))
        history.reset()
        assertNull(history.navigate(messages, "latest", 6, 6, false))
    }
}
