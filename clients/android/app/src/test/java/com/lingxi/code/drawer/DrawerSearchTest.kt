package com.lingxi.code.drawer

import com.lingxi.code.model.MockData
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pure-logic checks for the drawer search filters ([filterChats] /
 * [filterProjects] / [filterCrons]) that back the editable SearchBar. The
 * composable only hoists `query` and renders the filtered lists, so asserting
 * the filters here pins the real behavior without Compose. Pure JVM.
 *
 * Fixtures are the canonical "work" workspace slice (the drawer's default
 * `activeWs`), matching what `DrawerContent` feeds the filters at runtime.
 */
class DrawerSearchTest {

    private val chats = MockData.chats.filter { it.wsId == "work" }
    private val projects = MockData.projects.filter { it.wsId == "work" }
    private val crons = MockData.crons.filter { it.wsId == "work" }

    // --- empty / blank query is a pass-through -----------------------------

    @Test
    fun emptyQuery_returnsAllRows() {
        assertEquals(chats, filterChats(chats, ""))
        assertEquals(projects, filterProjects(projects, ""))
        assertEquals(crons, filterCrons(crons, ""))
    }

    @Test
    fun blankQuery_returnsAllRows() {
        assertEquals(chats, filterChats(chats, "   "))
        assertEquals(projects, filterProjects(projects, "   "))
        assertEquals(crons, filterCrons(crons, "\t "))
    }

    // --- chats -------------------------------------------------------------

    @Test
    fun chats_matchOnTitle_caseInsensitive() {
        // "重装 Claude Code" — match the embedded "claude" (lowercased).
        val out = filterChats(chats, "claude")
        assertEquals(1, out.size)
        assertEquals("c1", out.first().id)
    }

    @Test
    fun chats_matchOnPreview() {
        // c2 preview: "已生成 4 套话术".
        val out = filterChats(chats, "话术")
        assertEquals(listOf("c2"), out.map { it.id })
    }

    @Test
    fun chats_noMatch_returnsEmpty() {
        assertTrue(filterChats(chats, "zzz-没有-zzz").isEmpty())
    }

    // --- crons -------------------------------------------------------------

    @Test
    fun crons_matchOnTitleOrDesc() {
        // cr1 title "每日晨报".
        assertEquals(listOf("cr1"), filterCrons(crons, "晨报").map { it.id })
        // cr3 desc mentions "情感分析".
        assertEquals(listOf("cr3"), filterCrons(crons, "情感").map { it.id })
    }

    @Test
    fun crons_matchOnCronExpression() {
        // Several crons share "每周"; cr2 + cr3 are weekly.
        val out = filterCrons(crons, "每周")
        assertTrue(out.map { it.id }.containsAll(listOf("cr2", "cr3")))
    }

    // --- projects ----------------------------------------------------------

    @Test
    fun projects_matchOnProjectName_keepsAllSessions() {
        // p2 name "Q2 OKR & 周报".
        val out = filterProjects(projects, "OKR")
        assertEquals(1, out.size)
        val p2 = out.first()
        assertEquals("p2", p2.id)
        // Whole-project match keeps every session.
        assertEquals(
            projects.first { it.id == "p2" }.sessions.size,
            p2.sessions.size,
        )
    }

    @Test
    fun projects_matchOnSessionTitle_narrowsToMatchingSessions() {
        // p1 session "深色色板校准" matches "色板", but the project name does not.
        val out = filterProjects(projects, "色板")
        assertEquals(1, out.size)
        val p1 = out.first()
        assertEquals("p1", p1.id)
        // Only the matching session is retained.
        assertEquals(listOf("p1s3"), p1.sessions.map { it.id })
    }

    @Test
    fun projects_noMatchAnywhere_dropsProject() {
        assertTrue(filterProjects(projects, "zzz-没有-zzz").isEmpty())
    }
}
