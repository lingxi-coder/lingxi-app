package com.lingxi.code.model

import com.lingxi.code.isBoundToScope
import com.lingxi.code.isRestorableSession
import com.lingxi.code.shouldAttemptModeRestore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SessionModeRoutingTest {

    @Test
    fun `in-place resume requires the same workspace regardless of mode`() {
        val workspace = ConversationScope.Project("notes")

        assertTrue(
            isBoundToScope(
                boundScope = workspace,
                targetScope = workspace,
            ),
        )
        assertFalse(
            isBoundToScope(
                boundScope = ConversationScope.Global,
                targetScope = workspace,
            ),
        )
    }

    @Test
    fun `pending mode survives cold restore by session and workspace identity`() {
        val sessionId = "19587a33-0725-48db-abca-8a2aed345f6b"
        val row = SessionRow(
            uuid = sessionId, title = "Chat", messageCount = 1,
            relativeTime = "", mode = SessionMode.Chat,
        )
        // The saved current mode may still be Code while the catalog already
        // shows the pending Chat selection. Mode must not disqualify the row.
        assertTrue(isRestorableSession("sess:$sessionId", row.uuid, "/work/project", "/work/project"))
        assertFalse(isRestorableSession(sessionId, row.uuid, "/work/other", "/work/project"))
        assertFalse(isRestorableSession("session-b", row.uuid, "/work/project", "/work/project"))
    }

    @Test
    fun `new session is not replaced while its selected mode is being confirmed`() {
        val key = "project.notes#chat"
        assertFalse(shouldAttemptModeRestore(false, false, false, SessionMode.Code, SessionMode.Chat, key, null))
        assertFalse(shouldAttemptModeRestore(true, false, false, SessionMode.Chat, SessionMode.Code, key, null))
        assertFalse(shouldAttemptModeRestore(true, false, false, SessionMode.Chat, SessionMode.Chat, key, key))
        assertTrue(shouldAttemptModeRestore(true, false, false, SessionMode.Chat, SessionMode.Chat, key, null))
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
