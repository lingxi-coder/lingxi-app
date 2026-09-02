package com.lingxi.code.drawer

import com.lingxi.code.model.SessionMode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DrawerUiStateTest {

    @Test
    fun `collapsed stays isolated per mode while pinned state is shared`() {
        val state = DrawerUiState(
            activeWs = "global",
            activeSession = "",
            section = DrawerSection.Chat,
            collapsedWorkspaces = emptySet(),
            pinnedWorkspaces = emptyMap(),
        )

        state.toggleWorkspaceCollapsed(SessionMode.Chat, "app.notes")
        state.toggleWorkspacePinned(SessionMode.Chat, "app.notes", nowEpochMillis = 11L)
        state.toggleWorkspacePinned(SessionMode.Code, "project.demo", nowEpochMillis = 22L)

        assertTrue(state.isWorkspaceCollapsed(SessionMode.Chat, "app.notes"))
        assertFalse(state.isWorkspaceCollapsed(SessionMode.Code, "app.notes"))
        assertEquals(11L, state.pinnedAt(SessionMode.Chat, "app.notes"))
        assertEquals(11L, state.pinnedAt(SessionMode.Code, "app.notes"))
        assertEquals(22L, state.pinnedAt(SessionMode.Code, "project.demo"))
    }

    @Test
    fun `replaceWorkspacePresentation swaps the remembered workspace chrome`() {
        val state = DrawerUiState(
            activeWs = "global",
            activeSession = "",
            section = DrawerSection.Code,
            collapsedWorkspaces = setOf("chat:global"),
            pinnedWorkspaces = mapOf("global" to 1L),
        )

        state.replaceWorkspacePresentation(
            collapsed = setOf("code:project.demo"),
            pinned = mapOf("project.demo" to 42L),
        )

        assertFalse(state.isWorkspaceCollapsed(SessionMode.Chat, "global"))
        assertTrue(state.isWorkspaceCollapsed(SessionMode.Code, "project.demo"))
        assertEquals(42L, state.pinnedAt(SessionMode.Code, "project.demo"))
        assertEquals(42L, state.pinnedAt(SessionMode.Chat, "project.demo"))
    }
}
