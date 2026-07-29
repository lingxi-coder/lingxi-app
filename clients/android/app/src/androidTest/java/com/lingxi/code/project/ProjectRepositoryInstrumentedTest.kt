package com.lingxi.code.project

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.nio.file.Files
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class ProjectRepositoryInstrumentedTest {
    private val testRoot = File(
        InstrumentationRegistry.getInstrumentation().targetContext.cacheDir,
        "project-store-${UUID.randomUUID()}",
    )

    @After
    fun tearDown() {
        testRoot.deleteRecursively()
    }

    @Test
    fun createAndReloadUsesStableUuidWorkspaceLayout() {
        val repository = ProjectRepository(testRoot)
        val created = repository.createInternal("代码项目")
        val projectDir = File(testRoot, created.record.id)

        assertTrue(isLowercaseUuid(created.record.id))
        assertEquals(
            File(projectDir, PROJECT_WORKSPACE_DIR).canonicalPath,
            created.workspace.hostPath,
        )
        assertEquals("/workspace/${created.record.id}", created.workspace.guestPath)
        assertTrue(File(projectDir, PROJECT_MANIFEST_FILE).isFile)
        assertTrue(File(projectDir, PROJECT_SESSION_INDEX_FILE).isFile)
        assertTrue(File(projectDir, PROJECT_SYNC_BASELINE_FILE).isFile)
        assertFalse(File(created.workspace.hostDirectory, PROJECT_MANIFEST_FILE).exists())

        val restored = ProjectRepository(testRoot).load()
        assertEquals(listOf(created.record.id), restored.projects.map { it.record.id })
    }

    @Test
    fun oneCorruptManifestDoesNotHideOtherProjects() {
        var counter = 0
        val ids = listOf(
            "10000000-0000-4000-8000-000000000001",
            "10000000-0000-4000-8000-000000000002",
        )
        val repository = ProjectRepository(testRoot, newId = { ids[counter++] })
        val first = repository.createInternal("损坏项目")
        val second = repository.createInternal("正常项目")
        File(testRoot, "${first.record.id}/$PROJECT_MANIFEST_FILE").writeText("{broken")

        val restored = repository.load()
        assertEquals(listOf(second.record.id), restored.projects.map { it.record.id })
    }

    @Test
    fun projectAndGlobalSessionIndexesStayIsolated() {
        val repository = ProjectRepository(testRoot)
        val first = repository.createInternal("一")
        val second = repository.createInternal("二")
        val firstRows = listOf(session("session-a"), session("session-b"))
        val secondRows = listOf(session("session-c"))
        val globalRows = listOf(session("legacy-global"))

        repository.updateSessions(first.record.id, firstRows)
        repository.updateSessions(second.record.id, secondRows)
        repository.updateSessions(null, globalRows)
        val restored = repository.load()

        assertEquals(
            setOf("session-a", "session-b"),
            restored.projects.first { it.record.id == first.record.id }
                .sessions.map { it.sessionId }.toSet(),
        )
        assertEquals(
            listOf("session-c"),
            restored.projects.first { it.record.id == second.record.id }
                .sessions.map { it.sessionId },
        )
        assertEquals(listOf("legacy-global"), restored.globalSessions.map { it.sessionId })
        assertNotEquals(first.workspace.hostPath, second.workspace.hostPath)
    }

    @Test
    fun sessionStartedIsIndexedBeforeEngineCatalogContainsIt() {
        val repository = ProjectRepository(testRoot)
        val project = repository.createInternal("Started")

        val state = repository.recordStartedSession(
            projectId = project.record.id,
            sessionId = "engine-session-1",
            title = "新对话",
        )

        val reloaded = state.projects.single { it.record.id == project.record.id }
        assertEquals("engine-session-1", reloaded.sessions.single().sessionId)
        assertEquals("engine-session-1", reloaded.record.lastActiveSessionId)
        assertEquals(0, reloaded.sessions.single().messageCount)
    }

    @Test
    fun globalSessionStartedIsIndexedBeforeEngineCatalogContainsIt() {
        val repository = ProjectRepository(testRoot)
        val uuid = "19587a33-0725-48db-abca-8a2aed345f6b"

        val state = repository.recordStartedSession(
            projectId = null,
            sessionId = "sess:$uuid",
            title = "新对话",
        )

        assertEquals(uuid, state.globalSessions.single().sessionId)
        assertEquals(0, state.globalSessions.single().messageCount)
        assertEquals(
            uuid,
            ProjectRepository(testRoot).load().globalSessions.single().sessionId,
        )
    }

    @Test
    fun multipleEmptySessionsRemainIndexedAndKeepTheirStableIds() {
        val repository = ProjectRepository(testRoot)
        val project = repository.createInternal("空会话清理")
        val first = "19587a33-0725-48db-abca-8a2aed345f6b"
        val second = "29587a33-0725-48db-abca-8a2aed345f6b"

        repository.recordStartedSession(project.record.id, first, "旧空会话")
        val state = repository.recordStartedSession(project.record.id, second, "新会话")

        val restored = state.projects.single()
        assertEquals(listOf(second, first), restored.sessions.map { it.sessionId })
        assertEquals(listOf(0, 0), restored.sessions.map { it.messageCount })
        assertEquals(second, restored.record.lastActiveSessionId)
    }

    @Test
    fun legacyPrefixedSessionIndexLoadsAsBareUuid() {
        val repository = ProjectRepository(testRoot)
        val project = repository.createInternal("旧版索引")
        val uuid = "19587a33-0725-48db-abca-8a2aed345f6b"

        repository.recordStartedSession(
            projectId = project.record.id,
            sessionId = "sess:$uuid",
            title = "旧版会话",
        )

        val restored = ProjectRepository(testRoot).load().projects.single()
        assertEquals(uuid, restored.sessions.single().sessionId)
        assertEquals(uuid, restored.record.lastActiveSessionId)
    }

    @Test
    fun failedAtomicManifestWriteRollsBackNewProjectDirectory() {
        val id = "20000000-0000-4000-8000-000000000001"
        val writer = ProjectAtomicWriter { target, _, _ ->
            if (target.name == PROJECT_MANIFEST_FILE) error("injected write failure")
        }
        val repository = ProjectRepository(testRoot, newId = { id }, writer = writer)

        val failed = runCatching { repository.createInternal("失败") }

        assertTrue(failed.isFailure)
        assertFalse(File(testRoot, id).exists())
    }

    @Test
    fun onlyCanonicalLowercaseUuidIsAccepted() {
        assertTrue(isLowercaseUuid("12345678-1234-4abc-8def-1234567890ab"))
        assertFalse(isLowercaseUuid("12345678-1234-4ABC-8def-1234567890ab"))
        assertFalse(isLowercaseUuid("1-1-1-1-1"))
    }

    @Test
    fun workspaceSymlinkIsQuarantinedAndNestedLinksAreUnsafe() {
        val repository = ProjectRepository(testRoot)
        val project = repository.createInternal("链接安全")
        val workspace = project.workspace.hostDirectory
        val outside = File(testRoot, "outside-workspace").also { assertTrue(it.mkdirs()) }
        val secret = File(outside, "secret.txt").also { it.writeText("secret") }
        val nestedLink = File(workspace, "secret-link")
        Files.createSymbolicLink(nestedLink.toPath(), secret.toPath())

        assertFalse(isSafeWorkspacePath(workspace, nestedLink))
        val regular = File(workspace, "regular.txt").also { it.writeText("ok") }
        assertTrue(isSafeWorkspacePath(workspace, regular))

        assertTrue(workspace.deleteRecursively())
        Files.createSymbolicLink(workspace.toPath(), outside.toPath())
        val reloaded = repository.load()
        assertTrue(reloaded.projects.isEmpty())
        assertTrue(reloaded.errorMessage?.contains("已隔离") == true)
    }

    private fun session(id: String) = ProjectSessionSummary(
        sessionId = id,
        title = id,
        messageCount = 1,
        relativeTime = "刚刚",
        updatedAtEpochMillis = System.currentTimeMillis(),
    )
}
