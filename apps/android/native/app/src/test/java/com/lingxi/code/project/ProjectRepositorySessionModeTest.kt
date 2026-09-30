package com.lingxi.code.project

import com.lingxi.code.model.SessionMode
import com.lingxi.code.conversation.completeSessionListCommand
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class ProjectRepositorySessionModeTest {

    @get:Rule
    val temp = TemporaryFolder()

    private fun repository() = ProjectRepository(
        projectsRoot = File(temp.root, "projects"),
        now = { 1000L },
    )

    @Test
    fun `authoritative session listing keeps more than the SDK default five rows`() {
        val command = completeSessionListCommand()
        assertEquals(UInt.MAX_VALUE, command.limit)
        val repository = repository()
        val rows = (1..8).map { index ->
            ProjectSessionSummary("session-$index", "Session $index", 2, "Now", index.toLong())
        }.sortedByDescending { it.updatedAtEpochMillis }
        repository.updateSessions(null, rows)

        // Model the pinned SDK's truncation by the explicitly requested limit.
        // The local index replacement must receive all confirmed rows.
        repository.updateSessions(null, rows.take(command.limit!!.toLong().coerceAtMost(rows.size.toLong()).toInt()))

        assertEquals(8, repository().load().globalSessions.size)
        assertEquals(rows.map { it.sessionId }, repository.load().globalSessions.map { it.sessionId })
    }

    @Test
    fun `global session index preserves session mode`() {
        val repository = repository()

        repository.updateSessions(
            projectId = null,
            sessions = listOf(
                ProjectSessionSummary(
                    sessionId = "session-1",
                    title = "Chat session",
                    messageCount = 2,
                    relativeTime = "刚刚",
                    updatedAtEpochMillis = 1000L,
                    mode = SessionMode.Chat,
                ),
            ),
        )

        val loaded = repository.load().globalSessions.single()
        assertEquals(SessionMode.Chat, loaded.mode)
    }

    @Test
    fun `recordStartedSession writes the mode for provisional rows`() {
        val repository = repository()

        repository.recordStartedSession(
            projectId = null,
            sessionId = "session-2",
            title = "Code session",
            mode = SessionMode.Code,
        )

        val loaded = repository.load().globalSessions.single()
        assertEquals(SessionMode.Code, loaded.mode)
    }
    @Test
    fun `pending session survives stale catalogs and acknowledges without duplication`() {
        val repository = repository()
        val project = repository.createInternal("Project")
        repository.recordStartedSession(project.record.id, "sess:20000000-0000-4000-8000-0000000000AA", "First prompt", SessionMode.Chat)
        repository.updateSessions(project.record.id, emptyList())
        val reloaded = repository().project(project.record.id)
        assertEquals("20000000-0000-4000-8000-0000000000aa", reloaded.sessions.single().sessionId)
        assertEquals("20000000-0000-4000-8000-0000000000aa", reloaded.record.lastActiveSessionId)
        assertEquals(true, reloaded.sessions.single().pendingCatalogConfirmation)
        repository.updateSessions(project.record.id, listOf(reloaded.sessions.single().copy(messageCount = 2)))
        assertEquals(false, repository.project(project.record.id).sessions.single().pendingCatalogConfirmation)
        repository.updateSessions(project.record.id, emptyList())
        assertEquals(emptyList<ProjectSessionSummary>(), repository.project(project.record.id).sessions)
    }

    @Test
    fun `archive survives refresh and reload and restores without deleting data`() {
        val repository = repository()
        val row = ProjectSessionSummary("session-a", "Saved", 2, "Now", 1000L, SessionMode.Chat)
        repository.updateSessions(null, listOf(row))
        repository.setSessionArchived(null, row.sessionId, true)
        repository.updateSessions(null, listOf(row.copy(title = "Updated")))
        assertEquals(true, repository().load().globalSessions.single().isArchived)
        repository.updateSessions(null, emptyList())
        assertEquals("Updated", repository.load().globalSessions.single().title)
        repository.setSessionArchived(null, row.sessionId, false)
        repository.updateSessions(null, emptyList())
        assertEquals(false, repository().load().globalSessions.single().isArchived)
        assertEquals(2, repository.load().globalSessions.single().messageCount)
    }

    @Test
    fun `archive state is isolated by project even for the same session id`() {
        val repository = repository()
        val project = repository.createInternal("Project")
        val row = ProjectSessionSummary("same-id", "Saved", 2, "Now", 1000L)
        repository.updateSessions(null, listOf(row))
        repository.updateSessions(project.record.id, listOf(row))
        repository.setSessionArchived(project.record.id, row.sessionId, true)
        assertEquals(false, repository.load().globalSessions.single().isArchived)
        assertEquals(true, repository.project(project.record.id).sessions.single().isArchived)
        repository.recordStartedSession(project.record.id, row.sessionId, "Saved")
        assertEquals(true, repository.project(project.record.id).sessions.single().isArchived)
    }

    @Test
    fun `first submission count is recorded without regressing existing history or archive`() {
        val repository = repository()
        repository.recordStartedSession(null, "first", "Prompt", initialMessageCount = 1)
        assertEquals(1, repository.load().globalSessions.single().messageCount)
        repository.setSessionArchived(null, "first", true)
        repository.recordStartedSession(null, "first", "Prompt", initialMessageCount = 0)
        val row = repository.load().globalSessions.single()
        assertEquals(1, row.messageCount)
        assertEquals(true, row.isArchived)
    }

    @Test
    fun `stale zero message catalog cannot acknowledge first submission`() {
        val repository = repository()
        val stale = ProjectSessionSummary("first", "New conversation", 0, "Before", 900L)
        repository.updateSessions(null, listOf(stale))
        repository.recordStartedSession(null, "first", "First prompt", initialMessageCount = 1)
        repository.updateSessions(null, listOf(stale))
        val pending = repository().load().globalSessions.single()
        assertEquals("First prompt", pending.title)
        assertEquals(1, pending.messageCount)
        assertEquals(true, pending.pendingCatalogConfirmation)
        repository.updateSessions(null, listOf(stale.copy(title = "Confirmed", messageCount = 1)))
        val confirmed = repository.load().globalSessions.single()
        assertEquals("Confirmed", confirmed.title)
        assertEquals(false, confirmed.pendingCatalogConfirmation)
    }

}
