package com.lingxi.code.drawer

import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue

/**
 * The three lists the drawer can show — the 对话 / 项目 / 定时 tabs.
 *
 * Mirrors the iOS `Drawer.Section` enum; the raw string is the persisted key.
 */
enum class DrawerSection(val key: String) {
    Chats("chats"),
    Projects("projects"),
    Crons("crons"),
}

/**
 * Hoisted UI state for the drawer panel — the Compose analog of the iOS
 * `Drawer`'s `@Binding activeWs / activeSession` plus its local `@State section`
 * and `openProjects`.
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
    openProjects: Set<String>,
) {
    var activeWs by mutableStateOf(activeWs)
    var activeSession by mutableStateOf(activeSession)
    var section by mutableStateOf(section)
    var openProjects by mutableStateOf(openProjects)

    fun selectWorkspace(id: String) {
        activeWs = id
    }

    fun selectSession(id: String) {
        activeSession = id
    }

    fun toggleProject(id: String) {
        openProjects = if (id in openProjects) openProjects - id else openProjects + id
    }

    companion object {
        /** Default selection matches the iOS `RootView` ("work" / "s1", project p1 open). */
        val Saver: Saver<DrawerUiState, *> = listSaver(
            save = { listOf(it.activeWs, it.activeSession, it.section.key, it.openProjects.joinToString(",")) },
            restore = {
                DrawerUiState(
                    activeWs = it[0],
                    activeSession = it[1],
                    section = DrawerSection.entries.firstOrNull { s -> s.key == it[2] } ?: DrawerSection.Chats,
                    openProjects = it[3].split(",").filter(String::isNotEmpty).toSet(),
                )
            },
        )
    }
}

/** Remember a [DrawerUiState] across recomposition and process death. */
@Composable
fun rememberDrawerUiState(
    activeWs: String = "work",
    activeSession: String = "s1",
    section: DrawerSection = DrawerSection.Chats,
    openProjects: Set<String> = setOf("p1"),
): DrawerUiState = rememberSaveable(saver = DrawerUiState.Saver) {
    DrawerUiState(activeWs, activeSession, section, openProjects)
}

/** Grouping order for the chats list (matches the prototype's 今天/昨天/本周). */
internal val ChatGroupOrder = listOf("今天", "昨天", "本周")
