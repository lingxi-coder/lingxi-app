package com.lingxi.code.model

import com.lingxi.code.isBoundToScope
import com.lingxi.code.isRestorableSession
import com.lingxi.code.shouldAttemptModeRestore
import com.lingxi.code.shouldPersistActiveSession
import com.lingxi.code.sessionLinkModeMatchesOwner
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
        // Restoration uses the exact session identity and its workspace.
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
    fun `an unknown owner mode cannot be persisted as code`() {
        val key = "global#code"
        assertFalse(shouldPersistActiveSession(true, "session", null, SessionMode.Code, key, key))
        assertTrue(shouldPersistActiveSession(true, "session", SessionMode.Code, SessionMode.Code, key, key))
    }

    @Test
    fun `session links require a mode matching their known owner`() {
        assertFalse(sessionLinkModeMatchesOwner(null, SessionMode.Chat, SessionMode.Chat, true))
        assertFalse(sessionLinkModeMatchesOwner(SessionMode.Code, SessionMode.Chat, null, false))
        assertFalse(sessionLinkModeMatchesOwner(SessionMode.Chat, null, null, true))
        assertTrue(sessionLinkModeMatchesOwner(SessionMode.Chat, SessionMode.Chat, SessionMode.Chat, true))
        // Scheduled sessions without a UI index are checked by expectedMode on engine resume.
        assertTrue(sessionLinkModeMatchesOwner(SessionMode.Chat, null, null, false))
    }

}
