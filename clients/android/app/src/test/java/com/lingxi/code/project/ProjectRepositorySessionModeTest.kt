package com.lingxi.code.project

import com.lingxi.code.model.SessionMode
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
}
