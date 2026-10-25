package com.lingxi.code.project

import com.lingxi.code.R
import com.lingxi.code.conversation.ConversationStrings
import org.junit.Assert.assertEquals
import org.junit.Test

class ProjectUiMapperTest {
    @Test
    fun `drawer hides archived sessions but retains pending sessions`() {
        val visible = ProjectSessionSummary("visible", "Visible", 1, "Now", 1L)
        val project = ProjectSnapshot(
            record = ProjectRecord("project", "Project", ProjectStorageKind.Internal, 1L, 1L),
            workspace = ProjectWorkspace("project", "/tmp/project"),
            sessions = listOf(visible, visible.copy(sessionId = "archived", isArchived = true),
                visible.copy(sessionId = "pending", pendingCatalogConfirmation = true)),
        )
        assertEquals(listOf("visible", "pending"), project.toDrawerProject().sessions.map { it.id })
        assertEquals(3, project.sessions.size)
    }

    @Test
    fun `localized descriptions retain the sync state used by project actions`() {
        val project = ProjectSnapshot(
            record = ProjectRecord("project", "Project", ProjectStorageKind.SafMirror, 1L, 1L,
                sourceDisplayName = "Photos", syncState = ProjectSyncState.AuthorizationLost),
            workspace = ProjectWorkspace("project", "/tmp/project"),
            sessions = emptyList(),
        )
        val strings = ConversationStrings { id, fallback, args ->
            when (id) {
                R.string.project_storage_external_fmt -> "External mirror · ${args[0]}"
                R.string.project_sync_state_auth_lost -> "Authorization lost"
                else -> fallback
            }
        }
        val row = project.toDrawerProject(strings)
        assertEquals("External mirror · Photos · Authorization lost", row.desc)
        assertEquals(ProjectSyncState.AuthorizationLost.label, row.syncState)
    }
}
