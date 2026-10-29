package com.lingxi.code.teams

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.nio.file.Files

class TeamMessageProjectionTest {
    private fun fixture(): List<JSONObject> {
        val root = generateSequence(File(requireNotNull(System.getProperty("user.dir")))) { it.parentFile }.first { File(it, "packages/teams/fixtures/messaging.json").isFile }
        return JSONObject(File(root, "packages/teams/fixtures/messaging.json").readText()).getJSONArray("pages").objects()
    }
    @Test fun contiguousChangesRecallAndOldReplay() {
        val pages = fixture()
        var state = TeamMessageCache("owner", "test-team", 1)
        pages.forEach { state = state.apply(it) }
        state = state.apply(pages[0])
        assertEquals(3L, state.threads.getValue("test-chat").cursor)
        assertEquals("", state.threads.getValue("test-chat").messages.single().text)
        assertTrue(state.threads.getValue("test-chat").messages.single().recalled)
        assertEquals(0L, state.conversations.single().unread)
        assertEquals(3L, state.conversations.single().lastChangeSeq)
        try { TeamMessageCache("owner", "test-team", 1).apply(pages[1]); fail("Skipped change sequence") } catch (expected: TeamMessageFailure) { assertEquals("CHANGE_GAP", expected.code) }
    }
    @Test fun lateListCannotRegressWatermarks() {
        val pages = fixture()
        val latest = TeamMessageConversation.parse(pages[2].getJSONObject("conversation"))
        val old = TeamMessageConversation.parse(pages[0].getJSONObject("conversation"))
        assertEquals(latest, old.merging(latest))
    }
    @Test fun atomicFilePersistsCursorBodyAndOutboxAcrossRestartAndSeparatesGeneration() {
        val directory = Files.createTempDirectory("lingxi-team-projection-").toFile()
        try {
            val first = TeamMessageStorage(directory)
            first.update("owner", "test-team", 1) { old -> old.apply(fixture()[0]).copy(outbox = listOf(JSONObject().put("changeId", "pending001"))) }
            val recovered = TeamMessageStorage(directory).update("owner", "test-team", 1) { it }
            assertEquals(1L, recovered.threads.getValue("test-chat").cursor)
            assertEquals("Synthetic", recovered.threads.getValue("test-chat").messages.single().text)
            assertEquals("pending001", recovered.outbox.single().getString("changeId"))
            assertTrue(first.update("owner", "test-team", 2) { it }.threads.isEmpty())
            assertTrue(first.update("another", "test-team", 1) { it }.outbox.isEmpty())
            try { first.update("owner", "test-team", 1) { throw TeamMessageFailure("INJECTED_CRASH") }; fail("Expected failure") } catch (_: TeamMessageFailure) { }
            assertEquals(1L, first.update("owner", "test-team", 1) { it }.threads.getValue("test-chat").cursor)
        } finally { directory.deleteRecursively() }
    }
}
