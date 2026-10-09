package com.lingxi.code.drawer

import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class WorkspaceGroupsTest {

    private fun session(
        id: String,
        title: String,
        mode: SessionMode,
        modifiedAtEpochSeconds: Long,
    ) = SessionRow(
        uuid = id,
        title = title,
        messageCount = 1,
        mode = mode,
        modifiedAtEpochSeconds = modifiedAtEpochSeconds,
        relativeTime = "刚刚",
    )

    @Test
    fun `sort keeps global first then pinned then recent activity`() {
        val groups = listOf(
            WorkspaceGroup(
                stableKey = "project.b",
                scope = ConversationScope.Project("b"),
                kind = WorkspaceGroupKind.Project,
                name = "Bravo",
                sessions = listOf(session("pb", "Recent", SessionMode.Code, 20)),
            ),
            WorkspaceGroup(
                stableKey = "project.weather",
                scope = ConversationScope.Project("weather"),
                kind = WorkspaceGroupKind.Project,
                name = "Weather",
                pinnedAtEpochSeconds = 99,
                sessions = emptyList(),
            ),
            WorkspaceGroup(
                stableKey = "global",
                scope = ConversationScope.Global,
                kind = WorkspaceGroupKind.Global,
                name = "Global",
                sessions = emptyList(),
            ),
            WorkspaceGroup(
                stableKey = "project.a",
                scope = ConversationScope.Project("a"),
                kind = WorkspaceGroupKind.Project,
                name = "Alpha",
                sessions = listOf(session("pa", "Older", SessionMode.Code, 10)),
            ),
        )

        assertEquals(
            listOf("global", "project.weather", "project.b", "project.a"),
            sortWorkspaceGroups(groups).map { it.stableKey },
        )
    }

    @Test
    fun `filter matches workspace names and narrows by session title`() {
        val groups = listOf(
            WorkspaceGroup(
                stableKey = "global",
                scope = ConversationScope.Global,
                kind = WorkspaceGroupKind.Global,
                name = "Global",
                sessions = listOf(session("g1", "Release notes", SessionMode.Chat, 5)),
            ),
            WorkspaceGroup(
                stableKey = "project.finance",
                scope = ConversationScope.Project("finance"),
                kind = WorkspaceGroupKind.Project,
                name = "Finance Helper",
                sessions = listOf(
                    session("a1", "Portfolio recap", SessionMode.Chat, 4),
                    session("a2", "Build widget", SessionMode.Code, 3),
                ),
            ),
        )

        assertEquals(listOf("project.finance"), filterWorkspaceGroups(groups, "finance").map { it.stableKey })
        val narrowed = filterWorkspaceGroups(groups, "portfolio").single()
        assertEquals("project.finance", narrowed.stableKey)
        assertEquals(listOf("a1"), narrowed.sessions.map { it.uuid })
    }

    @Test
    fun `builder keeps empty chat groups and preserves project status`() {
        val groups = buildWorkspaceGroups(
            mode = SessionMode.Chat,
            globalName = "Global",
            globalSessions = listOf(session("g1", "General", SessionMode.Chat, 30)),
            projects = listOf(
                com.lingxi.code.model.Project(
                    id = "p1",
                    wsId = "project.p1",
                    name = "Workspace One",
                    icon = "◇",
                    color = androidx.compose.ui.graphics.Color.Blue,
                    desc = "External mirror · Synced",
                    sessions = listOf(
                        com.lingxi.code.model.ProjectSession(
                            id = "ps1",
                            title = "Code only",
                            activity = "刚刚",
                            preview = "",
                            msgs = 1,
                        ),
                    ),
                ),
            ),
        )

        assertEquals(listOf("global", "project.p1"), groups.map { it.stableKey })
        assertTrue(groups.first { it.stableKey == "project.p1" }.sessions.isEmpty())
        assertEquals("External mirror · Synced", groups.first { it.stableKey == "project.p1" }.status)
    }
}
