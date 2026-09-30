package com.lingxi.code.model

import com.lingxi.code.isBoundToScopeMode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SessionModeRoutingTest {

    @Test
    fun `in-place resume requires both workspace and mode to match`() {
        val workspace = ConversationScope.LocalApp("notes")

        assertTrue(
            isBoundToScopeMode(
                boundScope = workspace,
                boundMode = SessionMode.Chat,
                targetScope = workspace,
                targetMode = SessionMode.Chat,
            ),
        )
        assertFalse(
            isBoundToScopeMode(
                boundScope = workspace,
                boundMode = SessionMode.Code,
                targetScope = workspace,
                targetMode = SessionMode.Chat,
            ),
        )
        assertFalse(
            isBoundToScopeMode(
                boundScope = ConversationScope.Global,
                boundMode = SessionMode.Chat,
                targetScope = workspace,
                targetMode = SessionMode.Chat,
            ),
        )
    }

    @Test
    fun `persisted session stays resumable before its catalog row arrives`() {
        val target = persistedSessionTarget(
            sessionId = "session-chat",
            catalogRow = null,
        ) ?: throw AssertionError("a stored session id must remain a resume target")

        assertEquals("session-chat", target.ref.id)
        assertEquals("", target.ref.title)
        assertTrue(
            "resume-empty is safe for both empty and populated transcripts and avoids guessing",
            target.resumeEmpty,
        )
    }

    @Test
    fun `catalog metadata enriches a persisted session target`() {
        val target = persistedSessionTarget(
            sessionId = "session-code",
            catalogRow = SessionRow(
                uuid = "session-code",
                title = "Build release",
                messageCount = 4,
                relativeTime = "刚刚",
                mode = SessionMode.Code,
            ),
        ) ?: throw AssertionError("expected a resume target")

        assertEquals(SessionRef("session-code", "Build release"), target.ref)
        assertFalse(target.resumeEmpty)
    }
}
