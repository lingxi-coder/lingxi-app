package com.lingxi.code.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Mock-data integrity — counts + ids pinned against the `lingxi-iphone.html`
 * prototype / iOS `Models.swift`. The whole Android shell renders from this one
 * dataset, so a drift here (a renamed id, a dropped session, a count off by
 * one) would desync the drawer/conversation from the canonical design. Pure JVM
 * (no Android framework).
 */
class MockDataTest {

    // --- workspaces -------------------------------------------------------

    @Test
    fun workspaces_count_andExactIdOrder() {
        assertEquals(4, MockData.workspaces.size)
        assertEquals(
            listOf("personal", "work", "research", "creative"),
            MockData.workspaces.map { it.id },
        )
    }

    @Test
    fun workspaceIds_areUnique() {
        val ids = MockData.workspaces.map { it.id }
        assertEquals(ids.size, ids.toSet().size)
    }

    // --- chats ------------------------------------------------------------

    @Test
    fun chats_count_idsAndGroups() {
        assertEquals(3, MockData.chats.size)
        assertEquals(listOf("c1", "c2", "c3"), MockData.chats.map { it.id })
        // Every chat belongs to the "work" workspace (the seeded default).
        assertTrue(MockData.chats.all { it.wsId == "work" })
        assertEquals(listOf("今天", "昨天", "本周"), MockData.chats.map { it.group })
    }

    // --- projects + sessions ----------------------------------------------

    @Test
    fun projects_count_ids_andSessionTotals() {
        assertEquals(3, MockData.projects.size)
        assertEquals(listOf("p1", "p2", "p3"), MockData.projects.map { it.id })
        // Session counts per project: p1=3, p2=2, p3=2 → 7 total.
        assertEquals(3, MockData.projects[0].sessions.size)
        assertEquals(2, MockData.projects[1].sessions.size)
        assertEquals(2, MockData.projects[2].sessions.size)
        assertEquals(7, MockData.projects.sumOf { it.sessions.size })
    }

    @Test
    fun projectSessionIds_areUnique_andFirstIsPinned() {
        val sessionIds = MockData.projects.flatMap { p -> p.sessions.map { it.id } }
        assertEquals("session ids unique", sessionIds.size, sessionIds.toSet().size)
        // The prototype pins exactly the first session ("s1").
        val pinned = MockData.projects.flatMap { it.sessions }.filter { it.pinned }
        assertEquals(1, pinned.size)
        assertEquals("s1", pinned.first().id)
    }

    // --- crons ------------------------------------------------------------

    @Test
    fun crons_count_ids_andEnabledSplit() {
        assertEquals(4, MockData.crons.size)
        assertEquals(listOf("cr1", "cr2", "cr3", "cr4"), MockData.crons.map { it.id })
        // 3 enabled, 1 paused (cr4).
        assertEquals(3, MockData.crons.count { it.enabled })
        assertFalse("cr4 is paused", MockData.crons.first { it.id == "cr4" }.enabled)
    }

    // --- models -----------------------------------------------------------

    @Test
    fun models_count_ids_andShortNameStripsPrefix() {
        assertEquals(4, MockData.models.size)
        assertEquals(listOf("lx-72b", "lx-72b-r", "lx-32b", "lx-code"), MockData.models.map { it.id })
        assertEquals("72B", MockData.models.first { it.id == "lx-72b" }.shortName)
        assertEquals("Code", MockData.models.first { it.id == "lx-code" }.shortName)
    }

    // --- default conversation ---------------------------------------------

    @Test
    fun messagesDefault_count_andAlternatingRoles() {
        assertEquals(4, MockData.messagesDefault.size)
        assertEquals(
            listOf(Role.User, Role.Ai, Role.User, Role.Ai),
            MockData.messagesDefault.map { it.role },
        )
        // The two AI turns carry a "思考了…" tag; the user turns do not.
        assertEquals(2, MockData.messagesDefault.count { it.tag != null })
        MockData.messagesDefault.filter { it.role == Role.Ai }.forEach {
            assertTrue("AI msg has a 思考 tag", it.tag?.startsWith("思考了") == true)
        }
        // Each message gets a fresh, unique UUID id.
        val ids = MockData.messagesDefault.map { it.id }
        assertEquals("message ids unique", ids.size, ids.toSet().size)
    }

    // --- flattened session lookup -----------------------------------------

    @Test
    fun allSessions_flattensChatsPlusEveryProjectSession() {
        // 3 chats + 7 project sessions = 10 session refs.
        assertEquals(10, MockData.allSessions.size)
        val ids = MockData.allSessions.map { it.id }
        // chat ids come first, then each project's sessions in order.
        assertEquals(listOf("c1", "c2", "c3"), ids.take(3))
        assertTrue("contains pinned s1", "s1" in ids)
    }

    @Test
    fun session_resolvesById_andFallsBackToFirst() {
        val s1 = MockData.session("s1")
        assertEquals("s1", s1.id)
        assertEquals("设计灵犀 iPhone 版", s1.title)
        // Unknown id falls back to the first session ref (c1).
        assertEquals(MockData.allSessions.first().id, MockData.session("nope").id)
    }
}
