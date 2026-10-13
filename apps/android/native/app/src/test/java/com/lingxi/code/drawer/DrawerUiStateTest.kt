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
            filter = RecentsFilter.All,
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
            filter = RecentsFilter.Code,
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

    @Test
    fun `session pins toggle on and off independent of workspace pins`() {
        val state = DrawerUiState(
            activeWs = "global",
            activeSession = "",
            filter = RecentsFilter.All,
            collapsedWorkspaces = emptySet(),
            pinnedWorkspaces = mapOf("global" to 1L),
        )

        state.toggleSessionPinned("s1", nowEpochMillis = 50L)
        assertEquals(50L, state.sessionPinnedAt("s1"))
        assertEquals(1L, state.pinnedAt(SessionMode.Chat, "global"))

        state.toggleSessionPinned("s1", nowEpochMillis = 60L)
        assertEquals(null, state.sessionPinnedAt("s1"))
    }

    @Test
    fun `filter keys saved by the old three tabs restore to the nearest filter`() {
        assertEquals(RecentsFilter.Chat, RecentsFilter.fromKey("chat"))
        assertEquals(RecentsFilter.Code, RecentsFilter.fromKey("code"))
        assertEquals(RecentsFilter.All, RecentsFilter.fromKey("cron"))
    }
}
