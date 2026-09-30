package com.lingxi.code.drawer

import com.lingxi.code.localapps.LocalAppSessionRow
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRow
import java.io.File
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
                stableKey = "app.weather",
                scope = ConversationScope.LocalApp("weather"),
                kind = WorkspaceGroupKind.LocalApp,
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
            listOf("global", "app.weather", "project.b", "project.a"),
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
                stableKey = "app.finance",
                scope = ConversationScope.LocalApp("finance"),
                kind = WorkspaceGroupKind.LocalApp,
                name = "Finance Helper",
                sessions = listOf(
                    session("a1", "Portfolio recap", SessionMode.Chat, 4),
                    session("a2", "Build widget", SessionMode.Code, 3),
                ),
            ),
        )

        assertEquals(listOf("app.finance"), filterWorkspaceGroups(groups, "finance").map { it.stableKey })
        val narrowed = filterWorkspaceGroups(groups, "portfolio").single()
        assertEquals("app.finance", narrowed.stableKey)
        assertEquals(listOf("a1"), narrowed.sessions.map { it.uuid })
    }

    @Test
    fun `builder keeps empty chat groups and preserves project plus app status`() {
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
            localApps = listOf(
                DrawerLocalAppWorkspace(
                    appId = "calendar",
                    name = "Calendar",
                    status = "Draft",
                    sessions = listOf(
                        LocalAppSessionRow(
                            uuid = "la1",
                            title = "Chat lane",
                            relativeTime = "刚刚",
                            messageCount = 1,
                            mode = SessionMode.Chat,
                            modifiedAtEpochSeconds = 7,
                            isInit = false,
                        ),
                    ),
                ),
                DrawerLocalAppWorkspace(appId = "notes", name = "Notes"),
            ),
        )

        assertEquals(listOf("global", "app.calendar", "app.notes", "project.p1"), groups.map { it.stableKey })
        assertTrue(groups.first { it.stableKey == "project.p1" }.sessions.isEmpty())
        assertEquals(listOf("la1"), groups.first { it.stableKey == "app.calendar" }.sessions.map { it.uuid })
        assertEquals("External mirror · Synced", groups.first { it.stableKey == "project.p1" }.status)
        assertEquals("Draft", groups.first { it.stableKey == "app.calendar" }.status)
    }

    /**
     * The pinned create-interview session must carry its `isInit` flag through
     * the builder into the drawer's own [SessionRow] (not the [LocalAppSessionRow]
     * it started as), and must sort first regardless of recency — a user
     * re-opening a half-finished interview should not have to hunt for it below
     * a newer, ordinary session.
     */
    @Test
    fun `the pinned init session survives into SessionRow and sorts first`() {
        val groups = buildWorkspaceGroups(
            mode = SessionMode.Code,
            globalName = "Global",
            globalSessions = emptyList(),
            projects = emptyList(),
            localApps = listOf(
                DrawerLocalAppWorkspace(
                    appId = "tracker",
                    name = "Tracker",
                    sessions = listOf(
                        // Newer, ordinary session — listed FIRST in the source
                        // to prove the sort actually reorders rather than the
                        // input already being in the right order.
                        LocalAppSessionRow(
                            uuid = "newer",
                            title = "A later message",
                            relativeTime = "刚刚",
                            messageCount = 3,
                            mode = SessionMode.Code,
                            modifiedAtEpochSeconds = 200,
                            isInit = false,
                        ),
                        LocalAppSessionRow(
                            uuid = "init",
                            title = "The interview",
                            relativeTime = "5 分钟前",
                            messageCount = 1,
                            mode = SessionMode.Code,
                            modifiedAtEpochSeconds = 100,
                            isInit = true,
                        ),
                    ),
                ),
            ),
        )

        val sessions = groups.first { it.stableKey == "app.tracker" }.sessions
        assertEquals(
            "the init session must sort first even though it is OLDER than the other session",
            listOf("init", "newer"),
            sessions.map { it.uuid },
        )
        assertTrue(
            "the drawer's own SessionRow must carry isInit through, not drop it (the field this " +
                "builder maps into used to have no such field at all)",
            sessions.single { it.uuid == "init" }.isInit,
        )
        assertTrue(
            "the non-init session must not be misreported as init",
            !sessions.single { it.uuid == "newer" }.isInit,
        )
    }

    /**
     * [WorkspaceGroupSections.WorkspaceSessionRow] has no visible marker for
     * `isInit` in this compact row (unlike the Local Apps screen's own
     * session list, which has room for a separate AssistChip) — source-level,
     * since rendering it needs a Compose test harness this module's plain-JVM
     * `test` source set does not have (see `DrawerCreateEntryTest`'s header).
     */
    @Test
    fun `the drawer session row suffixes the init badge onto the title`() {
        val source = File("src/main/java/com/lingxi/code/drawer/WorkspaceGroupSections.kt").readText()

        val start = source.indexOf("private fun WorkspaceSessionRow(")
        assertTrue("expected to find WorkspaceSessionRow in WorkspaceGroupSections.kt", start >= 0)
        val end = source.indexOf("internal fun CronWorkspaceGroupsSection(", start)
        assertTrue("expected to find the next declaration after WorkspaceSessionRow", end > start)
        val body = source.substring(start, end)

        assertTrue(
            "vacuity guard: the sliced region must still read row.isInit",
            "row.isInit" in body,
        )
        assertTrue(
            "the init session's title must be suffixed with the badge copy when row.isInit is true, " +
                "reusing the same local_apps_session_init_badge string the Local Apps screen's " +
                "AssistChip already uses",
            "R.string.local_apps_session_init_badge" in body,
        )
    }
}
