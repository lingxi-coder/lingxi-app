package com.lingxi.code.project

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
}
