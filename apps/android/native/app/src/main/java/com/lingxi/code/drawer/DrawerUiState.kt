package com.lingxi.code.drawer

import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import com.lingxi.code.model.SessionMode

/**
 * Hoisted UI state for the drawer panel — the Compose analog of the iOS
 * `Drawer`'s `@Binding activeWs / activeSession` plus its local filter and pins.
 *
 * Held by [rememberDrawerUiState] so a config change / process death restores
 * the active workspace, selected session, mode filter and pins.
 * The conversation lives in its own `ChatViewModel`; the drawer only tracks
 * which session is *highlighted* and hands the selection back to the caller.
 * [filter] is a pure list filter — it never decides the conversation's mode.
 */
@Stable
class DrawerUiState(
    activeWs: String,
    activeSession: String,
    filter: RecentsFilter,
    collapsedWorkspaces: Set<String>,
    pinnedWorkspaces: Map<String, Long>,
    pinnedSessions: Map<String, Long> = emptyMap(),
) {
    var activeWs by mutableStateOf(activeWs)
    var activeSession by mutableStateOf(activeSession)
    var filter by mutableStateOf(filter)
    var collapsedWorkspaces by mutableStateOf(collapsedWorkspaces)
    var pinnedWorkspaces by mutableStateOf(pinnedWorkspaces)
    var pinnedSessions by mutableStateOf(pinnedSessions)

    fun selectWorkspace(id: String) {
        activeWs = id
    }

    fun selectSession(id: String) {
        activeSession = id
    }

    private fun workspaceModeKey(mode: SessionMode, workspaceKey: String): String =
        "${mode.wireKey}:$workspaceKey"

    fun isWorkspaceCollapsed(mode: SessionMode, workspaceKey: String): Boolean =
        workspaceModeKey(mode, workspaceKey) in collapsedWorkspaces

    fun toggleWorkspaceCollapsed(mode: SessionMode, workspaceKey: String) {
        val key = workspaceModeKey(mode, workspaceKey)
        collapsedWorkspaces = if (key in collapsedWorkspaces) {
            collapsedWorkspaces - key
        } else {
            collapsedWorkspaces + key
        }
    }

    fun setWorkspaceCollapsed(mode: SessionMode, workspaceKey: String, collapsed: Boolean) {
        val key = workspaceModeKey(mode, workspaceKey)
        collapsedWorkspaces = if (collapsed) collapsedWorkspaces + key else collapsedWorkspaces - key
    }

    fun pinnedAt(mode: SessionMode, workspaceKey: String): Long? =
        pinnedWorkspaces[workspaceKey]

    fun toggleWorkspacePinned(
        mode: SessionMode,
        workspaceKey: String,
        nowEpochMillis: Long = System.currentTimeMillis(),
    ) {
        pinnedWorkspaces = if (workspaceKey in pinnedWorkspaces) {
            pinnedWorkspaces - workspaceKey
        } else {
            pinnedWorkspaces + (workspaceKey to nowEpochMillis)
        }
    }

    fun replaceWorkspacePresentation(
        collapsed: Set<String>,
        pinned: Map<String, Long>,
    ) {
        collapsedWorkspaces = collapsed
        pinnedWorkspaces = pinned
    }

    fun sessionPinnedAt(sessionId: String): Long? = pinnedSessions[sessionId]

    fun toggleSessionPinned(
        sessionId: String,
        nowEpochMillis: Long = System.currentTimeMillis(),
    ) {
        pinnedSessions = if (sessionId in pinnedSessions) {
            pinnedSessions - sessionId
        } else {
            pinnedSessions + (sessionId to nowEpochMillis)
        }
    }

    fun replaceSessionPins(pinned: Map<String, Long>) {
        pinnedSessions = pinned
    }

    companion object {
        /** Production defaults contain no prototype workspace/session/project identifiers. */
        val Saver: Saver<DrawerUiState, *> = listSaver(
            save = {
                listOf(
                    it.activeWs,
                    it.activeSession,
                    it.filter.key,
                    it.collapsedWorkspaces.joinToString(","),
                    it.pinnedWorkspaces.encodePins(),
                    it.pinnedSessions.encodePins(),
                )
            },
            restore = {
                DrawerUiState(
                    activeWs = it[0],
                    activeSession = it[1],
                    // A key saved by the old Chat/Code/Cron tabs maps to the
                    // nearest filter ("chat"/"code") or to "all" ("cron").
                    filter = RecentsFilter.fromKey(it[2]),
                    collapsedWorkspaces = it.getOrNull(3)?.split(",")?.filter(String::isNotEmpty)?.toSet().orEmpty(),
                    pinnedWorkspaces = it.getOrNull(4).decodePins(),
                    pinnedSessions = it.getOrNull(5).decodePins(),
                )
            },
        )

        private fun Map<String, Long>.encodePins(): String =
            entries.joinToString(",") { entry -> "${entry.key}:${entry.value}" }

        private fun String?.decodePins(): Map<String, Long> = this
            ?.split(",")
            ?.mapNotNull { entry ->
                val index = entry.lastIndexOf(':')
                if (index <= 0) return@mapNotNull null
                val key = entry.substring(0, index)
                val value = entry.substring(index + 1).toLongOrNull() ?: return@mapNotNull null
                key to value
            }
            ?.toMap()
            .orEmpty()
    }
}

/** Remember a [DrawerUiState] across recomposition and process death. */
@Composable
fun rememberDrawerUiState(
    activeWs: String = "",
    activeSession: String = "",
    filter: RecentsFilter = RecentsFilter.All,
    collapsedWorkspaces: Set<String> = emptySet(),
    pinnedWorkspaces: Map<String, Long> = emptyMap(),
    pinnedSessions: Map<String, Long> = emptyMap(),
): DrawerUiState = rememberSaveable(saver = DrawerUiState.Saver) {
    DrawerUiState(activeWs, activeSession, filter, collapsedWorkspaces, pinnedWorkspaces, pinnedSessions)
}

/** Legacy mock-chat grouping order retained for the still-compiled fallback section. */
internal val ChatGroupOrder = listOf("今天", "昨天", "本周")
