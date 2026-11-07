package com.lingxi.code.project

import com.lingxi.code.R
import com.lingxi.code.bindings.runtime.MobileUserDirectories
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.defaultNewChatTarget
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.assertFalse
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.UUID

class MobileUserStorageTest {
    @get:Rule val temporary = TemporaryFolder()

    private fun directories(): MobileUserDirectories {
        val home = temporary.root.canonicalFile
        val config = File(home, ".brand")
        val workspace = File(home, ".brand-workspaces")
        return MobileUserDirectories(
            brandId = "brand", linuxUser = "user", configDirName = ".brand",
            home = home.path, guestHome = "/home/user", configHome = config.path,
            configGuestPath = "/home/user/.brand", projects = File(workspace, "projects").path,
            projectsGuestPath = "/home/user/.brand-workspaces/projects",
            chatCwd = File(workspace, "chat").path, chatGuestCwd = "/home/user/.brand-workspaces/chat",
            codeCwd = File(workspace, "code").path, codeGuestCwd = "/home/user/.brand-workspaces/code",
            scheduledCwd = File(workspace, "scheduled").path, scheduledGuestCwd = "/home/user/.brand-workspaces/scheduled",
            apps = File(workspace, "apps").path, appsGuestPath = "/home/user/.brand-workspaces/apps",
        )
    }

    @Test fun globalModeChoosesItsWorkspaceWithoutChangingLogicalScope() {
        val paths = directories()
        assertEquals("/home/user/.brand-workspaces/chat", paths.globalGuestCwd(SessionMode.Chat))
        assertEquals("/home/user/.brand-workspaces/code", paths.globalGuestCwd(SessionMode.Code))
        assertEquals(ConversationScope.Global to SessionMode.Chat, defaultNewChatTarget(SessionMode.Chat))
        assertEquals(ConversationScope.Global to SessionMode.Code, defaultNewChatTarget(SessionMode.Code))
        assertEquals("/home/user/.brand", paths.configGuestPath)
    }

    @Test fun onlyVerifiedProjectContentAndScheduledOwnerAreGrantedInsideTheContainer() {
        val paths = directories()
        listOf(paths.chatCwd, paths.codeCwd, paths.scheduledCwd, paths.projects).forEach { File(it).mkdirs() }
        val id = UUID.randomUUID().toString()
        val source = File(paths.projects, "$id/workspace").apply { mkdirs() }
        val guestSource = "${paths.projectsGuestPath}/$id/workspace"
        val manifest = File(source.parentFile, "project.json").apply { writeText("keep") }
        val record = ProjectRecord(id = id, name = "Project", storageKind = ProjectStorageKind.Internal,
            createdAtEpochMillis = 0, updatedAtEpochMillis = 0, directory = guestSource,
            visibleModes = setOf(SessionMode.Chat))
        val workspace = ProjectWorkspace(id, source.canonicalPath, guestSource)
        val project = ProjectSnapshot(record, workspace, emptyList(), workspace)
        val healthy = ProjectStoreState(projects = listOf(project), loading = false)
        assertTrue(workspaceOwnerStoreMatches(true, healthy, listOf(project)))
        assertFalse(workspaceOwnerStoreMatches(true, healthy.copy(storageUnavailable = true), listOf(project)))
        assertFalse(workspaceOwnerStoreMatches(false, healthy, listOf(project)))
        assertFalse(workspaceOwnerStoreMatches(true, healthy.copy(projects = emptyList()), listOf(project)))
        val task = File(paths.home, paths.configDirName + "-workspaces/cron/$id/runs/day/run").apply { mkdirs() }
        val taskGuest = "${paths.guestHome}/${paths.configDirName}-workspaces/cron/$id/runs/day/run"
        val scheduled = ProjectWorkspace("scheduled", task.canonicalPath, taskGuest)

        val withoutOwners = paths.workspaceDirectoryRepository(paths.chatGuestCwd)
        denied { withoutOwners.create(guestSource, "not-authorized") }
        denied { withoutOwners.create(taskGuest, "not-authorized") }
        denied { withoutOwners.create(paths.scheduledGuestCwd, "not-authorized") }
        val withOwners = paths.workspaceDirectoryRepository(paths.chatGuestCwd, listOf(project), scheduled)
        assertTrue(withOwners.resolve(withOwners.create(guestSource, "project-content")).isDirectory)
        assertTrue(withOwners.resolve(withOwners.create(taskGuest, "task-content")).isDirectory)
        denied { withOwners.delete("${paths.projectsGuestPath}/$id/project.json", emptyList()) }
        denied { withOwners.delete(guestSource, emptyList()) }
        denied { withOwners.delete(taskGuest, emptyList()) }
        denied { withOwners.create(paths.scheduledGuestCwd, "not-authorized") }
        assertEquals("keep", manifest.readText())

        val privatePath = File(paths.configHome, "provider").apply { mkdirs() }
        val forged = project.copy(sourceWorkspace = ProjectWorkspace(id, privatePath.canonicalPath))
        val rejected = paths.workspaceDirectoryRepository(paths.chatGuestCwd, listOf(forged))
        denied { rejected.create("${paths.configGuestPath}/provider", "not-authorized") }
    }

    private fun denied(action: () -> Unit) {
        try {
            action()
            throw AssertionError("Expected protected operation to fail")
        } catch (error: WorkspaceDirectoryException) {
            assertEquals(R.string.workspace_browser_in_use, error.messageResource)
        }
    }
}
