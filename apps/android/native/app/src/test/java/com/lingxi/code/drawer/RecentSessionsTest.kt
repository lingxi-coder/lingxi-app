package com.lingxi.code.drawer

import androidx.compose.ui.graphics.Color
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.Project
import com.lingxi.code.model.ProjectSession
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RecentSessionsTest {

    // 2026-10-12 18:00:00 in UTC+8 (the user's zone) = 10:00:00 UTC.
    private val zone = 8 * 3600
    private val now = 1_791_799_200L
    private val day = 86_400L

    private fun row(
        id: String,
        title: String = id,
        mode: SessionMode = SessionMode.Code,
        modified: Long? = now,
        relative: String = "",
    ) = SessionRow(
        uuid = id,
        title = title,
        messageCount = 2,
        relativeTime = relative,
        mode = mode,
        modifiedAtEpochSeconds = modified,
    )

    private fun project(id: String, name: String, vararg sessions: ProjectSession) = Project(
        id = id,
        wsId = "ws",
        name = name,
        icon = "",
        color = Color.Unspecified,
        desc = "",
        sessions = sessions.toList(),
    )

    private fun projectSession(
        id: String,
        mode: SessionMode = SessionMode.Code,
        modified: Long? = now,
    ) = ProjectSession(
        id = id,
        title = id,
        activity = "",
        preview = "",
        msgs = 3,
        mode = mode,
        updatedAtEpochMillis = modified?.times(1000L) ?: 0L,
    )

    private fun build(
        globalSessions: List<SessionRow> = emptyList(),
        projects: List<Project> = emptyList(),
        filter: RecentsFilter = RecentsFilter.All,
        query: String = "",
        pinned: Map<String, Long> = emptyMap(),
        status: (String) -> RecentStatus = { RecentStatus.Idle },
    ) = buildRecentGroups(
        globalName = "全局",
        globalSessions = globalSessions,
        projects = projects,
        filter = filter,
        query = query,
        pinnedSessions = pinned,
        statusFor = status,
        nowEpochSeconds = now,
        utcOffsetSeconds = zone,
    )

    @Test
    fun `buckets follow local calendar days and are ordered pinned first`() {
        val groups = build(
            globalSessions = listOf(
                row("today", modified = now - 3_600L),
                row("yesterday", modified = now - day),
                row("three-days", modified = now - 3 * day),
                row("six-days", modified = now - 6 * day),
                row("seven-days", modified = now - 7 * day),
                row("pinned", modified = now - 30 * day),
            ),
            pinned = mapOf("pinned" to 5L),
        )

        assertEquals(
            listOf(
                RecentBucket.Pinned to listOf("pinned"),
                RecentBucket.Today to listOf("today"),
                RecentBucket.Yesterday to listOf("yesterday"),
                RecentBucket.LastWeek to listOf("three-days", "six-days"),
                RecentBucket.Earlier to listOf("seven-days"),
            ),
            groups.map { group -> group.bucket to group.sessions.map { it.uuid } },
        )
    }

    @Test
    fun `a session just after local midnight is today and one just before is yesterday`() {
        // Local midnight of 2026-10-12 in UTC+8 is 2026-10-11 16:00:00 UTC.
        val localMidnight = now - (18 * 3600L)
        val groups = build(
            globalSessions = listOf(
                row("after", modified = localMidnight + 60L),
                row("before", modified = localMidnight - 60L),
            ),
        )

        assertEquals(
            listOf(
                RecentBucket.Today to listOf("after"),
                RecentBucket.Yesterday to listOf("before"),
            ),
            groups.map { group -> group.bucket to group.sessions.map { it.uuid } },
        )
    }

    @Test
    fun `missing and future timestamps do not break bucketing`() {
        val groups = build(
            globalSessions = listOf(
                row("unknown", modified = null),
                row("future", modified = now + 5 * day),
            ),
        )

        assertEquals(
            listOf(
                RecentBucket.Today to listOf("future"),
                RecentBucket.Earlier to listOf("unknown"),
            ),
            groups.map { group -> group.bucket to group.sessions.map { it.uuid } },
        )
    }

    @Test
    fun `mode filter keeps only that mode across global and project sessions`() {
        val projects = listOf(
            project(
                "p1",
                "lingxi-app",
                projectSession("p-code", SessionMode.Code),
                projectSession("p-chat", SessionMode.Chat),
            ),
        )
        val globalSessions = listOf(
            row("g-code", mode = SessionMode.Code),
            row("g-chat", mode = SessionMode.Chat),
        )

        fun ids(filter: RecentsFilter) = build(globalSessions, projects, filter)
            .flatMap { it.sessions }
            .map { it.uuid }
            .toSet()

        assertEquals(setOf("g-code", "g-chat", "p-code", "p-chat"), ids(RecentsFilter.All))
        assertEquals(setOf("g-chat", "p-chat"), ids(RecentsFilter.Chat))
        assertEquals(setOf("g-code", "p-code"), ids(RecentsFilter.Code))
    }

    @Test
    fun `rows carry their scope and workspace name`() {
        val groups = build(
            globalSessions = listOf(row("g", modified = now)),
            projects = listOf(project("p1", "lingxi-app", projectSession("p", modified = now - 10L))),
        )
        val byId = groups.flatMap { it.sessions }.associateBy { it.uuid }

        assertEquals(ConversationScope.Global, byId.getValue("g").scope)
        assertEquals("全局", byId.getValue("g").workspaceName)
        assertEquals(ConversationScope.Project("p1"), byId.getValue("p").scope)
        assertEquals("lingxi-app", byId.getValue("p").workspaceName)
    }

    @Test
    fun `query matches title workspace name and relative time case-insensitively`() {
        val globalSessions = listOf(
            row("a", title = "Fix Login Crash", relative = "刚刚"),
            row("b", title = "周末去哪儿玩", relative = "昨天"),
        )
        val projects = listOf(project("p1", "LingXi-App", projectSession("c")))

        fun ids(query: String) = build(globalSessions, projects, query = query)
            .flatMap { it.sessions }
            .map { it.uuid }
            .toSet()

        assertEquals(setOf("a"), ids("login"))
        assertEquals(setOf("c"), ids("lingxi-app"))
        assertEquals(setOf("b"), ids("昨天"))
        assertEquals(setOf("a", "b", "c"), ids("   "))
        assertTrue(ids("nothing-matches").isEmpty())
    }

    @Test
    fun `pinned sessions lead and the most recently pinned comes first`() {
        val groups = build(
            globalSessions = listOf(
                row("old-pin", modified = now - 40 * day),
                row("new-pin", modified = now - 50 * day),
                row("plain", modified = now),
            ),
            pinned = mapOf("old-pin" to 10L, "new-pin" to 20L),
        )

        assertEquals(RecentBucket.Pinned, groups.first().bucket)
        assertEquals(listOf("new-pin", "old-pin"), groups.first().sessions.map { it.uuid })
        assertEquals(listOf("plain"), groups.last().sessions.map { it.uuid })
    }

    @Test
    fun `a session listed in both catalogs appears once`() {
        val groups = build(
            globalSessions = listOf(row("same")),
            projects = listOf(project("p1", "lingxi-app", projectSession("same"))),
        )

        assertEquals(1, groups.sumOf { it.sessions.size })
    }

    @Test
    fun `live status is attached by session uuid`() {
        val groups = build(
            globalSessions = listOf(row("busy"), row("waiting"), row("idle")),
            status = {
                when (it) {
                    "busy" -> RecentStatus.Running
                    "waiting" -> RecentStatus.AwaitingConfirmation
                    else -> RecentStatus.Idle
                }
            },
        )
        val byId = groups.flatMap { it.sessions }.associate { it.uuid to it.status }

        assertEquals(RecentStatus.Running, byId["busy"])
        assertEquals(RecentStatus.AwaitingConfirmation, byId["waiting"])
        assertEquals(RecentStatus.Idle, byId["idle"])
    }

    @Test
    fun `empty inputs produce no groups`() {
        assertTrue(build().isEmpty())
    }

    @Test
    fun `filter key round-trips and unknown keys fall back to all`() {
        RecentsFilter.entries.forEach { assertEquals(it, RecentsFilter.fromKey(it.key)) }
        assertEquals(RecentsFilter.All, RecentsFilter.fromKey("cron"))
        assertEquals(RecentsFilter.All, RecentsFilter.fromKey(null))
    }
}
