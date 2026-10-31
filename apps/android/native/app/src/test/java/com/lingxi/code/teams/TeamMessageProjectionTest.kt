package com.lingxi.code.teams

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files

class TeamMessageProjectionTest {
    @Test fun journalRetainsCommandsWithoutMessageHistoryAcrossRestart() {
        val directory = Files.createTempDirectory("lingxi-team-commands-").toFile()
        try {
            val first = TeamCommandStorage(directory)
            val intent = JSONObject().put("changeId", "pending001").put("text", "Pending text").put("attempted", true)
            first.update("owner", "test-team", 1) { old -> old.copy(outbox = listOf(intent), conversations = listOf(TeamMessageConversation("test-chat", "dm", "Private history", listOf("user:owner"), 1, 1, 0))) }
            val recovered = TeamCommandStorage(directory).load("owner", "test-team", 1)
            assertTrue(recovered.threads.isEmpty()); assertTrue(recovered.conversations.isEmpty())
            assertEquals(intent.toString(), recovered.outbox.single().toString())
            val json = JSONObject(directory.listFiles()!!.single().readText())
            assertFalse(json.has("threads")); assertFalse(json.has("conversations"))
            assertTrue(first.load("owner", "test-team", 2).outbox.isEmpty())
            assertTrue(first.load("another", "test-team", 1).outbox.isEmpty())
            try { first.update("owner", "test-team", 1) { throw TeamMessageFailure("INJECTED_CRASH") }; fail("Expected failure") } catch (_: TeamMessageFailure) { }
            assertEquals(intent.toString(), first.load("owner", "test-team", 1).outbox.single().toString())
        } finally { directory.deleteRecursively() }
    }
    @Test fun obsoleteHistoryIsRemovedWithoutLosingPendingIdentity() {
        val directory = Files.createTempDirectory("lingxi-team-history-retirement-").toFile()
        try {
            val storage = TeamCommandStorage(directory)
            storage.update("owner", "test-team", 1) { it.copy(outbox = listOf(JSONObject().put("changeId", "stable001"))) }
            val file = directory.listFiles()!!.single()
            file.writeText(JSONObject(file.readText()).put("threads", JSONObject().put("old", "plaintext")).put("conversations", "old").toString())
            assertEquals("stable001", storage.load("owner", "test-team", 1).outbox.single().getString("changeId"))
            assertFalse(file.readText().contains("plaintext")); assertFalse(JSONObject(file.readText()).has("conversations"))
        } finally { directory.deleteRecursively() }
    }
}
