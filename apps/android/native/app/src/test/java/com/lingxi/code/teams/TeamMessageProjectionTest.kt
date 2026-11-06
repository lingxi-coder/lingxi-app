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
    @Test fun unrelatedStoredHistoryIsPreservedWithoutDecodingItAsCommands() {
        val directory = Files.createTempDirectory("lingxi-team-history-retirement-").toFile()
        try {
            val storage = TeamCommandStorage(directory)
            storage.update("owner", "test-team", 1) { it.copy(outbox = listOf(JSONObject().put("changeId", "stable001"))) }
            val file = directory.listFiles()!!.single()
            file.writeText(JSONObject(file.readText()).put("threads", JSONObject().put("old", "plaintext")).put("conversations", "old").toString())
            assertEquals("stable001", storage.load("owner", "test-team", 1).outbox.single().getString("changeId"))
            assertTrue(file.readText().contains("plaintext")); assertTrue(JSONObject(file.readText()).has("conversations"))
        } finally { directory.deleteRecursively() }
    }
    @Test fun aMessageKeepsItsServerTimeAsIsoTextAndMilliseconds() {
        val wire = JSONObject().put("id", "m1").put("seq", 3).put("author", JSONObject().put("kind", "user").put("uid", "u2")).put("kind", "text").put("text", "hi").put("changeSeq", 3).put("changeId", "c1")
            .put("createdAt", "2026-11-02T08:00:00Z")
        val stamped = TeamMessage.parse(wire)
        assertEquals("2026-11-02T08:00:00Z", stamped.createdAt); assertEquals(1793606400000L, stamped.createdAtMs)
        // A timestamp the server has not stamped yet has no time, so the stream never groups or dates it.
        val pending = TeamMessage.parse(JSONObject(wire.toString()).put("createdAt", JSONObject.NULL))
        assertNull(pending.createdAt); assertNull(pending.createdAtMs)
        assertNull(TeamMessage.parse(JSONObject(wire.toString()).also { it.remove("createdAt") }).createdAtMs)
        assertNull(stamped.copy(createdAt = "not a time").createdAtMs)
    }

    @Test fun aBotMessageTakesItsBotKeyFromTheAuthor() {
        val wire = JSONObject().put("id", "m2").put("seq", 4).put("author", JSONObject().put("kind", "shared_bot").put("uid", "u5").put("sharedBotId", "sharedbot_b")).put("kind", "text").put("text", "done").put("changeSeq", 4).put("changeId", "c2")
        assertEquals("sharedbot_b", TeamMessage.parse(wire).sharedBotId)
        assertNull(TeamMessage.parse(JSONObject(wire.toString()).put("author", JSONObject().put("kind", "user").put("uid", "u2"))).sharedBotId)
    }

    @Test fun aConversationDocumentCarriesItsPointerBotKeysAndTheOwnWatermark() {
        val pointer = mapOf("id" to "m9", "seq" to 9L, "at" to null, "preview" to "see you", "author" to mapOf("kind" to "user", "uid" to "u2"), "recalled" to true)
        val data = mapOf("kind" to "group", "title" to "Crew", "participants" to listOf("user:u1", "user:u2", "shared_bot:sharedbot_b"), "lastSeq" to 9L, "lastChangeSeq" to 11L, "lastMessage" to pointer)
        val parsed = TeamMessageConversation.fromDocument("c9", data, 4) { null }
        assertEquals(listOf("sharedbot_b"), parsed.sharedBotIds); assertEquals(5L, parsed.unread)
        assertEquals(TeamLastMessage("m9", 9, null, "user", "u2", null, "see you", true), parsed.lastMessage)
        assertEquals(11L, parsed.lastChangeSeq)
        // Re-reading the same document never moves the watermark or the sequence numbers backwards.
        val merged = parsed.copy(lastSeq = 2, lastChangeSeq = 3, lastReadSeq = 1).merging(parsed)
        assertEquals(9L, merged.lastSeq); assertEquals(11L, merged.lastChangeSeq); assertEquals(4L, merged.lastReadSeq)
    }
}
