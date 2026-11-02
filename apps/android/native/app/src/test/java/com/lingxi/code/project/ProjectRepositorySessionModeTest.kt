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

    private val guestHome = "/home/test_user"
    private val testConfigDirectory = ".config"
    private val projectsGuestRoot = "$guestHome/lingxi/projects"

    @get:Rule
    val temp = TemporaryFolder()

    private fun repository() = ProjectRepository(
        projectsRoot = File(temp.root, "projects"),
        userHome = File(temp.root, "user-home"),
        projectsGuestRoot = projectsGuestRoot,
        userHomeGuestPath = guestHome,
        now = { 1000L },
    )

    @Test
    fun `empty projects persist only their creation mode and can join another space`() {
        val repository = repository()
        val project = repository.createInternal("Chat workspace", SessionMode.Chat)
        assertTrue(project.sessions.isEmpty())
        assertEquals(setOf(SessionMode.Chat), repository().project(project.record.id).record.visibleModes)
        repository.addVisibleMode(project.record.id, SessionMode.Code)
        assertEquals(setOf(SessionMode.Chat, SessionMode.Code), repository().project(project.record.id).record.visibleModes)
        assertTrue(repository().project(project.record.id).sessions.isEmpty())
    }

    @Test
    fun `project without explicit visible modes is rejected and preserved`() {
        val repository = repository()
        val project = repository.createInternal("Legacy", com.lingxi.code.model.SessionMode.Code)
        val manifest = File(temp.root, "projects/${project.record.id}/project.json")
        val json = org.json.JSONObject(manifest.readText()).also { it.remove("visibleModes") }
        manifest.writeText(json.toString())
        val original = manifest.readText()
        assertTrue(runCatching { repository().project(project.record.id) }.isFailure)
        assertTrue(repository().load().projects.isEmpty())
        assertEquals(original, manifest.readText())
    }

    @Test
    fun `authoritative session listing keeps more than the SDK default five rows`() {
        val command = completeSessionListCommand()
        assertEquals(UInt.MAX_VALUE, command.limit)
        val repository = repository()
        val rows = (1..8).map { index ->
            ProjectSessionSummary("session-$index", "Session $index", 2, "Now", index.toLong(), SessionMode.Code)
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
    fun `session index without a valid mode is rejected without rewriting it`() {
        val repository = repository()
        val row = ProjectSessionSummary("session-a", "Saved", 2, "Now", 1000L, SessionMode.Chat)
        repository.updateSessions(null, listOf(row))
        val file = File(temp.root, "projects/global-session-index.json")
        val json = org.json.JSONObject(file.readText())
        json.getJSONArray("sessions").getJSONObject(0).remove("mode")
        val invalid = json.toString()
        file.writeText(invalid)

        assertTrue(runCatching { repository().load() }.isFailure)
        assertTrue(runCatching { repository.updateSessions(null, listOf(row)) }.isFailure)
        assertEquals(invalid, file.readText())
    }

    @Test
    fun `damaged project index is never replaced with an empty index`() {
        val repository = repository()
        val project = repository.createInternal("Keep project", SessionMode.Chat)
        val root = File(temp.root, "projects")
        val index = File(root, PROJECTS_INDEX_FILE)
        index.writeText("{damaged")
        val original = index.readText()

        assertTrue(runCatching { repository.load() }.isFailure)
        assertTrue(runCatching { repository.createInternal("Another", SessionMode.Code) }.isFailure)
        assertEquals(original, index.readText())
        assertTrue(File(root, "${project.record.id}/$PROJECT_MANIFEST_FILE").isFile)
        assertEquals(1, root.listFiles().orEmpty().count { it.isDirectory && isLowercaseUuid(it.name) })
    }

    @Test
    fun `unindexed project is reported without rewriting the projects index`() {
        val repository = repository()
        val project = repository.createInternal("Orphan", SessionMode.Chat)
        val index = File(temp.root, "projects/$PROJECTS_INDEX_FILE")
        val contents = org.json.JSONObject(index.readText()).apply {
            put("projectIds", org.json.JSONArray())
            remove("activeProjectId")
        }.toString()
        index.writeText(contents)
        val failure = runCatching { repository.load() }.exceptionOrNull() as ProjectCatalogUnavailableException
        assertEquals(listOf("project.${project.record.id}"), failure.scopes)
        assertEquals(contents, index.readText())
        assertTrue(File(temp.root, "projects/${project.record.id}/$PROJECT_SESSION_INDEX_FILE").isFile)
    }

    @Test
    fun `missing global or project index freezes claims after initialization`() {
        val repository = repository()
        repository.load()
        val root = File(temp.root, "projects")
        val global = File(root, GLOBAL_SESSION_INDEX_FILE)
        val projects = File(root, PROJECTS_INDEX_FILE)
        val globalContent = global.readText()
        val projectsContent = projects.readText()

        assertTrue(global.delete())
        val noGlobal = runCatching { repository.load() }.exceptionOrNull() as ProjectCatalogUnavailableException
        assertEquals(listOf("global"), noGlobal.scopes)
        assertFalse(global.exists())
        assertEquals(projectsContent, projects.readText())
        global.writeText(globalContent)

        assertTrue(projects.delete())
        val noProjects = runCatching { repository.load() }.exceptionOrNull() as ProjectCatalogUnavailableException
        assertEquals(listOf("projects-index"), noProjects.scopes)
        assertFalse(projects.exists())
        assertEquals(globalContent, global.readText())
    }

    @Test
    fun `existing conversation state prevents replacing both missing indexes`() {
        val root = File(temp.root, "projects")
        val ownerState = File(temp.root, "scope-state.json").apply { writeText("retained owner data") }
        val repository = ProjectRepository(root, userHome = File(temp.root, "user-home"),
            projectsGuestRoot = projectsGuestRoot, userHomeGuestPath = guestHome, ownerStateFile = ownerState)

        val failure = runCatching { repository.load() }.exceptionOrNull() as ProjectCatalogUnavailableException
        assertEquals(listOf("global", "projects-index"), failure.scopes)
        assertFalse(File(root, GLOBAL_SESSION_INDEX_FILE).exists())
        assertFalse(File(root, PROJECTS_INDEX_FILE).exists())
        assertEquals("retained owner data", ownerState.readText())
    }

    @Test
    fun `a broken global index symlink is rejected without replacing it`() {
        val repository = repository()
        repository.load()
        val global = File(temp.root, "projects/$GLOBAL_SESSION_INDEX_FILE")
        assertTrue(global.delete())
        java.nio.file.Files.createSymbolicLink(global.toPath(), File(temp.root, "missing-index").toPath())
        val failure = runCatching { repository.load() }.exceptionOrNull() as ProjectCatalogUnavailableException
        assertEquals(listOf("global"), failure.scopes)
        assertTrue(java.nio.file.Files.isSymbolicLink(global.toPath()))
    }

    @Test
    fun `missing project session index freezes claims without rewriting project index`() {
        val repository = repository()
        val project = repository.createInternal("Keep project", SessionMode.Chat)
        val root = File(temp.root, "projects")
        val index = File(root, PROJECTS_INDEX_FILE)
        val original = index.readText()
        val sessions = File(root, "${project.record.id}/$PROJECT_SESSION_INDEX_FILE")
        assertTrue(sessions.delete())

        val failure = runCatching { repository.load() }.exceptionOrNull() as ProjectCatalogUnavailableException
        assertEquals(listOf("project.${project.record.id}"), failure.scopes)
        assertEquals(original, index.readText())
        assertTrue(runCatching { repository.setActiveProject(null) }.isFailure)
        assertEquals(original, index.readText())
        assertTrue(runCatching { repository.recordStartedSession(project.record.id, "session", "title", SessionMode.Chat) }.isFailure)
        assertFalse(sessions.exists())
    }

    @Test
    fun `session index missing required flags is rejected without rewriting it`() {
        val repository = repository()
        val row = ProjectSessionSummary("session-a", "Saved", 2, "Now", 1000L, SessionMode.Chat)
        repository.updateSessions(null, listOf(row))
        val file = File(temp.root, "projects/global-session-index.json")
        val json = org.json.JSONObject(file.readText())
        json.getJSONArray("sessions").getJSONObject(0).remove("isArchived")
        val original = json.toString()
        file.writeText(original)

        assertTrue(runCatching { repository.load() }.isFailure)
        assertTrue(runCatching { repository.updateSessions(null, listOf(row)) }.isFailure)
        assertEquals(original, file.readText())
    }

    @Test
    fun `catalog refresh cannot change a session's creation mode`() {
        val repository = repository()
        val row = ProjectSessionSummary("session-a", "Saved", 2, "Now", 1000L, SessionMode.Chat)
        repository.updateSessions(null, listOf(row))
        assertTrue(runCatching { repository.updateSessions(null, listOf(row.copy(mode = SessionMode.Code))) }.isFailure)
        assertTrue(runCatching { repository.recordStartedSession(null, row.sessionId, row.title, SessionMode.Code) }.isFailure)
        assertEquals(SessionMode.Chat, repository().load().globalSessions.single().mode)
    }

    @Test
    fun `conflicting duplicate catalog rows are rejected before overwriting the index`() {
        val repository = repository()
        val row = ProjectSessionSummary("session-a", "Saved", 2, "Now", 1000L, SessionMode.Chat)
        repository.updateSessions(null, listOf(row))
        val file = File(temp.root, "projects/global-session-index.json")
        val original = file.readText()
        assertTrue(runCatching { repository.updateSessions(null, listOf(row, row.copy(mode = SessionMode.Code))) }.isFailure)
        assertEquals(original, file.readText())
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
        val project = repository.createInternal("Project", com.lingxi.code.model.SessionMode.Code)
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
    fun `catalog writes reject a session owned by another workspace`() {
        val repository = repository()
        val project = repository.createInternal("Project", com.lingxi.code.model.SessionMode.Code)
        val otherProject = repository.createInternal("Other project", com.lingxi.code.model.SessionMode.Code)
        val row = ProjectSessionSummary("19587a33-0725-48db-abca-8a2aed345f6b", "Saved", 2, "Now", 1000L, SessionMode.Code)
        repository.updateSessions(null, listOf(row))
        val projectIndex = File(temp.root, "projects/${project.record.id}/$PROJECT_SESSION_INDEX_FILE")
        val original = projectIndex.readText()
        assertTrue(runCatching { repository.updateSessions(project.record.id, listOf(row)) }.isFailure)
        assertTrue(runCatching { repository.recordStartedSession(project.record.id, row.sessionId, "Saved", SessionMode.Chat) }.isFailure)
        assertEquals(original, projectIndex.readText())
        assertEquals(row.sessionId, repository.load().globalSessions.single().sessionId)

        val projectRow = row.copy(sessionId = "4190879f-18f0-4706-9f1c-c0ea8cfaac73")
        repository.updateSessions(project.record.id, listOf(projectRow))
        assertTrue(runCatching { repository.updateSessions(otherProject.record.id, listOf(projectRow)) }.isFailure)
        assertTrue(repository.project(otherProject.record.id).sessions.isEmpty())
    }

    @Test
    fun `conflicting stored workspace owners fail closed without changing any index`() {
        val repository = repository()
        val project = repository.createInternal("Project", com.lingxi.code.model.SessionMode.Code)
        val row = ProjectSessionSummary("19587a33-0725-48db-abca-8a2aed345f6b", "Saved", 2, "Now", 1000L, SessionMode.Chat)
        repository.updateSessions(null, listOf(row))
        val root = File(temp.root, "projects")
        val globalIndex = File(root, GLOBAL_SESSION_INDEX_FILE)
        val projectIndex = File(root, "${project.record.id}/$PROJECT_SESSION_INDEX_FILE")
        val projectsIndex = File(root, PROJECTS_INDEX_FILE)
        val duplicate = org.json.JSONObject(projectIndex.readText()).apply {
            getJSONArray("sessions").put(org.json.JSONObject(globalIndex.readText()).getJSONArray("sessions").getJSONObject(0))
        }.toString()
        projectIndex.writeText(duplicate)
        val originalGlobal = globalIndex.readText()
        val originalProjects = projectsIndex.readText()

        val failure = runCatching { repository.load() }.exceptionOrNull() as ProjectCatalogUnavailableException
        assertEquals(listOf("global", "project.${project.record.id}"), failure.scopes)
        assertTrue(runCatching { repository.setActiveProject(null) }.isFailure)
        assertTrue(runCatching { repository.updateSessions(project.record.id, emptyList()) }.isFailure)
        assertTrue(runCatching { repository.setSessionArchived(null, row.sessionId, true) }.isFailure)
        assertEquals(duplicate, projectIndex.readText())
        assertEquals(originalGlobal, globalIndex.readText())
        assertEquals(originalProjects, projectsIndex.readText())
    }

    @Test
    fun `unreadable project session index freezes claims but permits unrelated workspace access`() {
        val repository = repository()
        val broken = repository.createInternal("Broken", SessionMode.Chat)
        val healthy = repository.createInternal("Healthy", SessionMode.Code)
        val root = File(temp.root, "projects")
        val sessionIndex = File(root, "${broken.record.id}/$PROJECT_SESSION_INDEX_FILE")
        val projectsIndex = File(root, PROJECTS_INDEX_FILE)
        sessionIndex.writeText("{damaged")
        val originalProjects = projectsIndex.readText()

        val failure = runCatching { repository.load() }.exceptionOrNull() as ProjectCatalogUnavailableException
        assertEquals(listOf("project.${broken.record.id}"), failure.scopes)
        assertTrue(runCatching { repository.recordStartedSession(null, "new", "New", SessionMode.Chat) }.isFailure)
        assertTrue(runCatching { repository.updateSessions(healthy.record.id, emptyList()) }.isFailure)
        assertEquals(healthy.workspace.hostPath, repository.workspace(healthy.record.id).hostPath)
        assertEquals(broken.workspace.hostPath, repository.workspace(broken.record.id).hostPath)
        assertEquals("{damaged", sessionIndex.readText())
        assertEquals(originalProjects, projectsIndex.readText())
    }

    @Test
    fun `isolated project session index still prevents another workspace claiming its UUID`() {
        val repository = repository()
        val project = repository.createInternal("Project", com.lingxi.code.model.SessionMode.Code)
        val row = ProjectSessionSummary("19587a33-0725-48db-abca-8a2aed345f6b", "Saved", 2, "Now", 1000L, SessionMode.Chat)
        repository.updateSessions(project.record.id, listOf(row))
        val root = File(temp.root, "projects")
        val manifest = File(root, "${project.record.id}/$PROJECT_MANIFEST_FILE")
        val sessionIndex = File(root, "${project.record.id}/$PROJECT_SESSION_INDEX_FILE")
        val projectsIndex = File(root, PROJECTS_INDEX_FILE)
        val globalIndex = File(root, GLOBAL_SESSION_INDEX_FILE)
        manifest.writeText("{damaged")
        val originalSessions = sessionIndex.readText()
        val originalProjects = projectsIndex.readText()
        val originalGlobal = globalIndex.readText()

        val loaded = repository.load()
        assertTrue(loaded.projects.isEmpty())
        assertEquals(ScopeStateStore.SessionOwner("project.${project.record.id}", SessionMode.Chat),
            repository.indexedSessionOwners(loaded.globalSessions)[row.sessionId])
        assertTrue(runCatching { repository.updateSessions(null, listOf(row)) }.isFailure)
        assertTrue(runCatching { repository.recordStartedSession(null, row.sessionId, "Other", SessionMode.Code) }.isFailure)
        assertEquals(originalSessions, sessionIndex.readText())
        assertEquals(originalProjects, projectsIndex.readText())
        assertEquals(originalGlobal, globalIndex.readText())
    }

    @Test
    fun `first submission count is recorded without regressing existing history or archive`() {
        val repository = repository()
        repository.recordStartedSession(null, "first", "Prompt", SessionMode.Code, initialMessageCount = 1)
        assertEquals(1, repository.load().globalSessions.single().messageCount)
        repository.setSessionArchived(null, "first", true)
        repository.recordStartedSession(null, "first", "Prompt", SessionMode.Code, initialMessageCount = 0)
        val row = repository.load().globalSessions.single()
        assertEquals(1, row.messageCount)
        assertEquals(true, row.isArchived)
    }

    @Test
    fun `second brand paths drive new project identity and preserve prior-brand directories`() {
        val projectsRoot = File(temp.root, "atlas/projects")
        val userHome = File(temp.root, "atlas/home")
        val projectsGuest = "/home/atlas_guest/atlas/projects"
        val guestHome = "/home/atlas_guest"
        val repository = ProjectRepository(
            projectsRoot = projectsRoot,
            userHome = userHome,
            projectsGuestRoot = projectsGuest,
            userHomeGuestPath = guestHome,
        )
        val project = repository.createInternal("Atlas project", com.lingxi.code.model.SessionMode.Code)
        assertEquals("$projectsGuest/${project.record.id}/workspace", project.record.directory)
        assertEquals(project.record.directory, project.workspace.guestPath)

        val oldBrandGuest = "$guestHome/lingxi/projects/linked"
        val oldBrandHost = File(userHome, "lingxi/projects/linked").apply { mkdirs() }
        repository.recordStartedSession(project.record.id, "session-atlas", "Atlas session", SessionMode.Code)
        repository.recordWorkspace(
            project.record.id,
            SessionWorkspaceState("session-atlas", oldBrandGuest, project.record.directory, emptyList(), ""),
        )
        val restored = ProjectRepository(
            projectsRoot = projectsRoot,
            userHome = userHome,
            projectsGuestRoot = projectsGuest,
            userHomeGuestPath = guestHome,
        ).project(project.record.id)
        assertEquals(oldBrandHost.canonicalPath, restored.workspace.hostPath)
        assertEquals(oldBrandGuest, restored.sessions.single().workspaceDirectory)
    }

    @Test
    fun `stale zero message catalog cannot acknowledge first submission`() {
        val repository = repository()
        val stale = ProjectSessionSummary("first", "New conversation", 0, "Before", 900L, SessionMode.Code)
        repository.updateSessions(null, listOf(stale))
        repository.recordStartedSession(null, "first", "First prompt", SessionMode.Code, initialMessageCount = 1)
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
        val repository = ProjectRepository(projectsRoot = root, userHome = user, projectsGuestRoot = projectsGuestRoot, userHomeGuestPath = guestHome)
        val first = repository.createInternal("Workspace project", com.lingxi.code.model.SessionMode.Code)
        val linked = File(root, "linked").apply { mkdirs() }
        val linkedGuest = "$projectsGuestRoot/linked"
        repository.recordStartedSession(first.record.id, "session-one", "Shared session", SessionMode.Code)
        repository.recordWorkspace(first.record.id, SessionWorkspaceState("session-one", linkedGuest, first.record.directory, listOf(linkedGuest), "$guestHome/$testConfigDirectory/projects/b/session-one.jsonl"))
        val reloaded = ProjectRepository(projectsRoot = root, userHome = user, projectsGuestRoot = projectsGuestRoot, userHomeGuestPath = guestHome).project(first.record.id)
        assertEquals(linked.canonicalPath, reloaded.workspace.hostPath)
        assertEquals(first.sourceWorkspace, reloaded.sourceWorkspace)
        assertEquals(listOf("session-one"), reloaded.sessions.map { it.sessionId })
        assertEquals(linkedGuest, reloaded.sessions.single().workspaceDirectory)
        assertEquals(first.record.directory, reloaded.sessions.single().originalProjectDirectory)
        assertEquals(listOf(linkedGuest), reloaded.workspace.additionalDirectories)
        linked.deleteRecursively()
        assertEquals(first.record.id, ProjectRepository(projectsRoot = root, userHome = user, projectsGuestRoot = projectsGuestRoot, userHomeGuestPath = guestHome).load().projects.single().record.id)
        assertFalse(linked.exists())
    }

    @Test
    fun `directory navigation does not create a phantom session or escape the user home`() {
        val repository = repository()
        val project = repository.createInternal("Empty draft", com.lingxi.code.model.SessionMode.Code)
        val linked = File(temp.root, "projects/linked").apply { mkdirs() }
        val path = "$projectsGuestRoot/linked"
        repository.recordWorkspace(project.record.id, SessionWorkspaceState("draft", path, project.record.directory, listOf(path), ""))
        assertTrue(repository.project(project.record.id).sessions.isEmpty())
        val error = runCatching { repository.recordWorkspace(project.record.id, SessionWorkspaceState("draft", "/root", project.record.directory, emptyList(), "")) }.exceptionOrNull()
        assertTrue(error is IllegalArgumentException)
        assertEquals(linked.canonicalPath, repository.project(project.record.id).workspace.hostPath)
    }

    @Test
    fun `catalog in another workspace retains confirmed sessions and their exact directory`() {
        val repository = repository()
        val project = repository.createInternal("Linked directories", com.lingxi.code.model.SessionMode.Code)
        val linked = "$projectsGuestRoot/linked"
        File(temp.root, "projects/linked").mkdirs()
        val original = ProjectSessionSummary("a", "A", 2, "Now", 1000L, SessionMode.Code,
            workspaceDirectory = project.record.directory, transcriptPath = "/catalog/a.jsonl")
        repository.updateSessions(project.record.id, listOf(original))
        repository.recordWorkspace(project.record.id, SessionWorkspaceState("draft", linked,
            linked, listOf(linked), ""))
        repository.updateSessions(project.record.id, listOf(ProjectSessionSummary("b", "B", 2, "Now", 1001L, SessionMode.Code,
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
        val project = repository.createInternal("Missing linked folder", com.lingxi.code.model.SessionMode.Code)
        val missing = "$projectsGuestRoot/deleted"
        repository.recordWorkspace(project.record.id, SessionWorkspaceState("draft", project.record.directory,
            project.record.directory, listOf(missing), ""))
        assertEquals(listOf(missing), repository.project(project.record.id).record.additionalDirectories)
        assertFalse(File(temp.root, "projects/deleted").exists())
    }

    @Test
    fun `registering an application source preserves its owned directory and reuses the project`() {
        val home = temp.newFolder("linux-home")
        val projects = File(home, "lingxi/projects")
        val source = File(home, "$testConfigDirectory/.apps/counter").apply { mkdirs() }
        val guestSource = "$guestHome/$testConfigDirectory/.apps/counter"
        File(source, "index.html").writeText("unchanged")
        val repo = ProjectRepository(projects, userHome = home, projectsGuestRoot = "$guestHome/lingxi/projects", userHomeGuestPath = guestHome)
        val first = repo.registerAuthoringWorkspace("Counter", guestSource, SessionMode.Code)
        val second = repo.registerAuthoringWorkspace("Counter", guestSource, SessionMode.Code)
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
        val repo = ProjectRepository(File(home, "lingxi/projects"), userHome = home, projectsGuestRoot = "$guestHome/lingxi/projects", userHomeGuestPath = guestHome)
        org.junit.Assert.assertThrows(IllegalArgumentException::class.java) {
            repo.registerAuthoringWorkspace("Escape", "$guestHome/escape", SessionMode.Code)
        }
        assertTrue(repo.load().projects.isEmpty())
    }

}
