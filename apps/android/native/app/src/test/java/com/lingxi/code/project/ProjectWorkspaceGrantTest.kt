package com.lingxi.code.project

import com.lingxi.code.model.SessionMode
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.nio.file.Files
import java.util.UUID
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.async
import kotlinx.coroutines.yield

class ProjectWorkspaceGrantTest {
    @get:Rule val temporary = TemporaryFolder()

    @Test fun explicitProjectPathsProtectMetadataAndRequireExactHostGuestCoordinates() {
        val home = temporary.root.canonicalFile
        val brand = File(home, ".brand-workspaces")
        val id = UUID.randomUUID().toString()
        val projectRoot = File(brand, "projects/$id/workspace").apply { mkdirs() }
        val roots = ProjectGrantRoots(home, "/home/user", ".brand", "/home/user/.brand",
            "/home/user/.brand-workspaces/chat", "/home/user/.brand-workspaces/code", "/home/user/.brand-workspaces/scheduled",
            "/home/user/.brand-workspaces/apps", "/home/user/.brand-workspaces/projects")
        listOf("chat", "code", "scheduled/safe", "scheduled/providers", "providers", "apps/app", "projects").forEach {
            File(brand, it).mkdirs()
        }
        File(home, "normal/folder").mkdirs()
        val projectGuest = "${roots.projectsGuestPath}/$id/workspace"
        val record = ProjectRecord(id, "Project", ProjectStorageKind.Internal, 0, 0,
            directory = projectGuest, visibleModes = setOf(SessionMode.Code))
        val project = ProjectSnapshot(record, ProjectWorkspace(id, projectRoot.canonicalPath, projectGuest), emptyList())
        fun allowed(path: String, host: String? = null, app: AuthoringSourceIdentity? = null) =
            isSafeProjectWorkspace(roots, path, host, project, app)

        assertTrue(allowed("/home/user/normal/folder"))
        assertTrue(allowed(roots.chatGuestCwd))
        assertTrue(allowed(roots.codeGuestCwd))
        assertTrue(allowed("${roots.scheduledGuestCwd}/safe"))
        assertFalse(isSafeProjectWorkspace(roots, "${roots.scheduledGuestCwd}/safe", null, null))
        assertTrue(allowed(projectGuest, projectRoot.canonicalPath))
        assertFalse(allowed("/home/user"))
        assertFalse(allowed(roots.configGuestPath))
        assertFalse(allowed(roots.scheduledGuestCwd))
        assertFalse(allowed("${roots.scheduledGuestCwd}/providers"))
        assertFalse(allowed("${roots.configGuestPath}/providers"))
        assertFalse(allowed(roots.projectsGuestPath))
        assertFalse(allowed(projectGuest, File(home, "normal/folder").canonicalPath))
        assertFalse(allowed("${roots.appsGuestPath}/app"))
        val app = AuthoringSourceIdentity("app-a", id, File(brand, "apps/app").canonicalPath, "${roots.appsGuestPath}/app")
        assertTrue(allowed(app.guestPath, app.hostPath, app))
        assertFalse(allowed(app.guestPath, app.hostPath, app.copy(projectId = UUID.randomUUID().toString())))

        Files.createSymbolicLink(File(home, "alias").toPath(), File(home, "normal/folder").toPath())
        assertFalse(allowed("/home/user/alias"))
    }

    @Test fun preparedAuthoringGrantIsExactCurrentAndSingleUse() {
        val source = AuthoringSourceIdentity("app-a", UUID.randomUUID().toString(), "/home/source", "/guest/source")
        val grant = PreparedAuthoringGrant(source, 17)
        assertNull(grant.consume(source.copy(appId = "app-b"), 17, true))
        assertNull(grant.consume(source.copy(guestPath = "/guest/other"), 17, true))
        assertNull(grant.consume(source, 18, true))
        assertNull(grant.consume(source, 17, false))
        assertNotNull(grant.consume(source, 17, true))
        assertNull(grant.consume(source, 17, true))
    }

    @Test fun scheduledPickerRequiresItsOwnVerifiedContentWorkspace() {
        val path = "/home/user/.brand-workspaces/workspaces/day/job"
        val owner = ProjectWorkspace("scheduled", "/host/day/job", path)
        assertTrue(scheduledPickerPathAllowed(true, owner, path))
        assertTrue(scheduledPickerPathAllowed(true, owner, "$path/subdir"))
        assertFalse(scheduledPickerPathAllowed(true, owner, "/home/user/.brand-workspaces/workspaces/day/other"))
        assertFalse(scheduledPickerPathAllowed(true, null, path))
        assertFalse(scheduledPickerPathAllowed(true, owner.copy(projectId = "other"), path))
        assertTrue(scheduledPickerPathAllowed(false, null, "/home/user/other"))
    }

    @Test fun pendingAdmissionBelongsToOneOwnerAndOnlyItsExactBindingUnlocksIt() {
        val source = AuthoringSourceIdentity("app-a", UUID.randomUUID().toString(), "/home/source", "/guest/source")
        val ownerA = PendingAuthoringAdmission(source)
        val ownerB = PendingAuthoringAdmission(source.copy(appId = "app-b"))
        assertFalse(ownerA.ready)
        assertFalse(ownerA.acknowledge(source.copy(projectId = UUID.randomUUID().toString()), "session-a", "session-a"))
        assertFalse(ownerA.acknowledge(source, "session-a", "session-b"))
        assertFalse(ownerB.ready)
        assertTrue(ownerA.acknowledge(source, "session-a", "session-a"))
        assertTrue(ownerA.ready)
        assertFalse(ownerB.ready)
    }

    @Test fun coldAuthoringSourceNeedsExactDurableOwnerAndRejectsIndexChangesDuringLookup() = runBlocking {
        val home = temporary.root.canonicalFile
        val projectId = UUID.randomUUID().toString()
        val sessionId = UUID.randomUUID().toString()
        val workspaceId = UUID.randomUUID().toString()
        val appHost = File(home, ".brand-workspaces/apps/app").apply { mkdirs() }.canonicalPath
        val projectHost = File(home, ".brand-workspaces/projects/$projectId/workspace").apply { mkdirs() }.canonicalPath
        val roots = ProjectGrantRoots(home, "/home/user", ".brand", "/home/user/.brand",
            "/home/user/.brand-workspaces/chat", "/home/user/.brand-workspaces/code", "/home/user/.brand-workspaces/scheduled",
            "/home/user/.brand-workspaces/apps", "/home/user/.brand-workspaces/projects")
        val guest = "${roots.appsGuestPath}/app"
        val workspace = ProjectWorkspace(projectId, appHost, guest)
        val row = ProjectSessionSummary(sessionId, "App", 0, "", 0, SessionMode.Code)
        var project = ProjectSnapshot(
            ProjectRecord(projectId, "App", ProjectStorageKind.Internal, 0, 0,
                directory = guest, visibleModes = setOf(SessionMode.Code)),
            workspace, listOf(row), ProjectWorkspace(projectId, projectHost),
        )
        val receipt = AuthoringGrantReceipt(guest, projectId, sessionId, workspaceId, null)
        suspend fun verify(
            owner: Boolean = true,
            result: suspend () -> AuthoringGrantReceipt = { receipt },
        ) = verifyProjectSource(roots, "project.$projectId", workspace, SessionMode.Code, sessionId, null,
            loadProject = { project }, ownerMatches = { id, key, mode ->
                owner && id == sessionId && key == "project.$projectId" && mode == SessionMode.Code
            }, lookupAuthoring = { source, id, session ->
                assertEquals(guest, source); assertEquals(projectId, id); assertEquals(sessionId, session)
                result()
            })

        assertEquals(AuthoringSourceIdentity(null, projectId, appHost, guest), verify())
        assertTrue(runCatching { verify(owner = false) }.isFailure)
        assertTrue(runCatching { verify { receipt.copy(sessionId = UUID.randomUUID().toString()) } }.isFailure)
        assertTrue(runCatching { verify { receipt.copy(projectId = UUID.randomUUID().toString()) } }.isFailure)
        assertTrue(runCatching { verify { error("missing receipt") } }.isFailure)
        assertTrue(runCatching { verify { project = project.copy(record = project.record.copy(directory = roots.codeGuestCwd)); receipt } }.isFailure)
    }

    @Test fun preparedSourceCannotBeTransferredOrInventedFromAnAppPath() = runBlocking {
        val home = temporary.root.canonicalFile
        val projectId = UUID.randomUUID().toString()
        val appHost = File(home, ".brand-workspaces/apps/app").apply { mkdirs() }.canonicalPath
        val projectHost = File(home, ".brand-workspaces/projects/$projectId/workspace").apply { mkdirs() }.canonicalPath
        val roots = ProjectGrantRoots(home, "/home/user", ".brand", "/home/user/.brand",
            "/home/user/.brand-workspaces/chat", "/home/user/.brand-workspaces/code", "/home/user/.brand-workspaces/scheduled",
            "/home/user/.brand-workspaces/apps", "/home/user/.brand-workspaces/projects")
        val guest = "${roots.appsGuestPath}/app"
        val workspace = ProjectWorkspace(projectId, appHost, guest)
        val project = ProjectSnapshot(ProjectRecord(projectId, "App", ProjectStorageKind.Internal, 0, 0,
            directory = guest, visibleModes = setOf(SessionMode.Code)), workspace, emptyList(), ProjectWorkspace(projectId, projectHost))
        val identity = AuthoringSourceIdentity("app-a", projectId, appHost, guest)
        val pending = checkNotNull(PreparedAuthoringGrant(identity, 3).consume(identity, 3, true))
        suspend fun verify(gate: PendingAuthoringAdmission?) = verifyProjectSource(
            roots, "project.$projectId", workspace, SessionMode.Code, null, gate, { project },
            { _, _, _ -> error("no session owner before creation") }, { _, _, _ -> error("no receipt before creation") },
        )
        assertEquals(identity, verify(pending))
        assertTrue(runCatching { verify(null) }.isFailure)
        assertTrue(runCatching { verify(PendingAuthoringAdmission(identity.copy(projectId = UUID.randomUUID().toString()))) }.isFailure)
        assertFalse(pending.ready)
    }

    @Test fun retainedAuthoringRejectsDelayedCatalogAfterTheLiveCwdChanges() {
        val id = UUID.randomUUID().toString()
        val source = AuthoringSourceIdentity("app-a", UUID.randomUUID().toString(), "/host/source", "/guest/source")
        val workspace = SessionWorkspaceState(id, source.guestPath, source.guestPath, emptyList(), "/transcript/a.jsonl")
        fun matches(active: String = id, current: SessionWorkspaceState = workspace, transcript: String = "/transcript/a.jsonl",
            host: String = source.hostPath, guest: String = source.guestPath, project: String = source.projectId,
            key: String = "project.${source.projectId}", mode: SessionMode = SessionMode.Code) =
            retainedAuthoringSourceMatches(source, id, SessionMode.Code, active, current, transcript, host, guest, project, key, mode)
        assertTrue(matches())
        assertFalse(matches(current = workspace.copy(cwd = "/guest/other")))
        assertFalse(matches(active = UUID.randomUUID().toString()))
        assertFalse(matches(transcript = "/transcript/b.jsonl"))
        assertFalse(matches(host = "/host/other"))
        assertFalse(matches(guest = "/guest/other"))
        assertFalse(matches(project = UUID.randomUUID().toString()))
        assertFalse(matches(key = "global"))
        assertFalse(matches(mode = SessionMode.Chat))
    }

    @Test fun pendingSourceBlocksRecoveryUntilExactOwnerAcknowledged() = runBlocking {
        val identity = AuthoringSourceIdentity(null, UUID.randomUUID().toString(), "/host/source", "/guest/source")
        val gate = PendingAuthoringAdmission(identity)
        val waiter = async { gate.awaitReady() }
        yield()
        assertFalse(waiter.isCompleted)
        assertFalse(gate.acknowledge(identity.copy(projectId = UUID.randomUUID().toString()), "session-a", "session-a"))
        assertFalse(gate.acknowledge(identity, "session-a", "session-b"))
        assertFalse(waiter.isCompleted)
        assertTrue(gate.acknowledge(identity.copy(appId = "metadata-only"), "session-a", "session-a"))
        waiter.await()
        assertTrue(gate.ready)
    }
}
