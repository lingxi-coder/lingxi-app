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

/** The three top-level mobile drawers: Chat / Code / Cron. */
enum class DrawerSection(val key: String) {
    Chat("chat"),
    Code("code"),
    Cron("cron"),
}

/**
 * Hoisted UI state for the drawer panel — the Compose analog of the iOS
 * `Drawer`'s `@Binding activeWs / activeSession` plus its local `@State section`
 * and expanded workspace set.
 *
 * Held by [rememberDrawerUiState] so a config change / process death restores
 * the active workspace, selected session, current tab and expanded projects.
 * The conversation lives in its own `ChatViewModel`; the drawer only tracks
 * which session is *highlighted* and hands the selection back to the caller.
 */
@Stable
class DrawerUiState(
    activeWs: String,
    activeSession: String,
    section: DrawerSection,
    collapsedWorkspaces: Set<String>,
    pinnedWorkspaces: Map<String, Long>,
) {
    var activeWs by mutableStateOf(activeWs)
    var activeSession by mutableStateOf(activeSession)
    var section by mutableStateOf(section)
    var collapsedWorkspaces by mutableStateOf(collapsedWorkspaces)
    var pinnedWorkspaces by mutableStateOf(pinnedWorkspaces)

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

    companion object {
        /** Production defaults contain no prototype workspace/session/project identifiers. */
        val Saver: Saver<DrawerUiState, *> = listSaver(
            save = {
                listOf(
                    it.activeWs,
                    it.activeSession,
                    it.section.key,
                    it.collapsedWorkspaces.joinToString(","),
                    it.pinnedWorkspaces.entries.joinToString(",") { entry -> "${entry.key}:${entry.value}" },
                )
            },
            restore = {
                DrawerUiState(
                    activeWs = it[0],
                    activeSession = it[1],
                    section = DrawerSection.entries.firstOrNull { s -> s.key == it[2] } ?: DrawerSection.Chat,
                    collapsedWorkspaces = it.getOrNull(3)?.split(",")?.filter(String::isNotEmpty)?.toSet().orEmpty(),
                    pinnedWorkspaces = it.getOrNull(4)
                        ?.split(",")
                        ?.mapNotNull { entry ->
                            val index = entry.lastIndexOf(':')
                            if (index <= 0) return@mapNotNull null
                            val key = entry.substring(0, index)
                            val value = entry.substring(index + 1).toLongOrNull() ?: return@mapNotNull null
                            key to value
                        }
                        ?.toMap()
                        .orEmpty(),
                )
            },
        )
    }
}

/** Remember a [DrawerUiState] across recomposition and process death. */
@Composable
fun rememberDrawerUiState(
    activeWs: String = "",
    activeSession: String = "",
    section: DrawerSection = DrawerSection.Chat,
    collapsedWorkspaces: Set<String> = emptySet(),
    pinnedWorkspaces: Map<String, Long> = emptyMap(),
): DrawerUiState = rememberSaveable(saver = DrawerUiState.Saver) {
    DrawerUiState(activeWs, activeSession, section, collapsedWorkspaces, pinnedWorkspaces)
}

/** Legacy mock-chat grouping order retained for the still-compiled fallback section. */
internal val ChatGroupOrder = listOf("今天", "昨天", "本周")
