package com.lingxi.code.project

import com.lingxi.code.model.SessionMode
import com.lingxi.code.conversation.completeSessionListCommand
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
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

    @Test
    fun `one project keeps its session and source while moving and cold reloading workspaces`() {
        val root = File(temp.root, "user/projects")
        val user = File(temp.root, "user")
        val repository = ProjectRepository(projectsRoot = root, userHome = user)
        val first = repository.createInternal("Workspace project")
        val linked = File(root, "linked").apply { mkdirs() }
        val linkedGuest = "/home/lingxi/lingxi/projects/linked"
        repository.recordStartedSession(first.record.id, "session-one", "Shared session", SessionMode.Code)
        repository.recordWorkspace(first.record.id, SessionWorkspaceState("session-one", linkedGuest, first.record.directory, listOf(linkedGuest), "/home/lingxi/.lingxi/projects/b/session-one.jsonl"))
        val reloaded = ProjectRepository(projectsRoot = root, userHome = user).project(first.record.id)
        assertEquals(linked.canonicalPath, reloaded.workspace.hostPath)
        assertEquals(first.sourceWorkspace, reloaded.sourceWorkspace)
        assertEquals(listOf("session-one"), reloaded.sessions.map { it.sessionId })
        assertEquals(linkedGuest, reloaded.sessions.single().workspaceDirectory)
        assertEquals(first.record.directory, reloaded.sessions.single().originalProjectDirectory)
        assertEquals(listOf(linkedGuest), reloaded.workspace.additionalDirectories)
        linked.deleteRecursively()
        assertEquals(first.record.id, ProjectRepository(projectsRoot = root, userHome = user).load().projects.single().record.id)
        assertFalse(linked.exists())
    }

    @Test
    fun `directory navigation does not create a phantom session or escape the user home`() {
        val repository = repository()
        val project = repository.createInternal("Empty draft")
        val linked = File(temp.root, "projects/linked").apply { mkdirs() }
        val path = "/home/lingxi/lingxi/projects/linked"
        repository.recordWorkspace(project.record.id, SessionWorkspaceState("draft", path, project.record.directory, listOf(path), ""))
        assertTrue(repository.project(project.record.id).sessions.isEmpty())
        val error = runCatching { repository.recordWorkspace(project.record.id, SessionWorkspaceState("draft", "/root", project.record.directory, emptyList(), "")) }.exceptionOrNull()
        assertTrue(error is IllegalArgumentException)
        assertEquals(linked.canonicalPath, repository.project(project.record.id).workspace.hostPath)
    }

    @Test
    fun `catalog in another workspace retains confirmed sessions and their exact directory`() {
        val repository = repository()
        val project = repository.createInternal("Linked directories")
        val linked = "/home/lingxi/lingxi/projects/linked"
        File(temp.root, "projects/linked").mkdirs()
        val original = ProjectSessionSummary("a", "A", 2, "Now", 1000L,
            workspaceDirectory = project.record.directory, transcriptPath = "/catalog/a.jsonl")
        repository.updateSessions(project.record.id, listOf(original))
        repository.recordWorkspace(project.record.id, SessionWorkspaceState("draft", linked,
            linked, listOf(linked), ""))
        repository.updateSessions(project.record.id, listOf(ProjectSessionSummary("b", "B", 2, "Now", 1001L,
            workspaceDirectory = linked, transcriptPath = "/catalog/b.jsonl")))
        val restored = repository().project(project.record.id)
        assertEquals(setOf("a", "b"), restored.sessions.map { it.sessionId }.toSet())
        assertEquals(project.record.directory, restored.sessions.first { it.sessionId == "a" }.workspaceDirectory)
        assertEquals(linked, restored.sessions.first { it.sessionId == "b" }.workspaceDirectory)
        // Deletion remains authoritative for the directory actually being listed.
        repository.updateSessions(project.record.id, emptyList())
        assertEquals(listOf("a"), repository.project(project.record.id).sessions.map { it.sessionId })
    }

    @Test
    fun `missing linked directory does not prevent saving the live workspace`() {
        val repository = repository()
        val project = repository.createInternal("Missing linked folder")
        val missing = "/home/lingxi/lingxi/projects/deleted"
        repository.recordWorkspace(project.record.id, SessionWorkspaceState("draft", project.record.directory,
            project.record.directory, listOf(missing), ""))
        assertEquals(listOf(missing), repository.project(project.record.id).record.additionalDirectories)
        assertFalse(File(temp.root, "projects/deleted").exists())
    }

    @Test
    fun `registering an application source preserves its owned directory and reuses the project`() {
        val home = temp.newFolder("linux-home")
        val projects = File(home, "lingxi/projects")
        val source = File(home, ".lingxi/.apps/counter").apply { mkdirs() }
        File(source, "index.html").writeText("unchanged")
        val repo = ProjectRepository(projects, userHome = home)
        val first = repo.registerAuthoringWorkspace("Counter", "/home/lingxi/.lingxi/.apps/counter")
        val second = repo.registerAuthoringWorkspace("Counter", "/home/lingxi/.lingxi/.apps/counter")
        assertEquals(first.record.id, second.record.id)
        assertEquals(source.canonicalPath, first.workspace.hostPath)
        assertEquals("unchanged", File(source, "index.html").readText())
        assertTrue(first.sessions.isEmpty())
    }

    @Test
    fun `application source registration rejects an escaped user directory`() {
        val home = temp.newFolder("linux-home")
        val outside = temp.newFolder("outside")
        java.nio.file.Files.createSymbolicLink(File(home, "escape").toPath(), outside.toPath())
        val repo = ProjectRepository(File(home, "lingxi/projects"), userHome = home)
        org.junit.Assert.assertThrows(IllegalArgumentException::class.java) {
            repo.registerAuthoringWorkspace("Escape", "/home/lingxi/escape")
        }
        assertTrue(repo.load().projects.isEmpty())
    }

}
