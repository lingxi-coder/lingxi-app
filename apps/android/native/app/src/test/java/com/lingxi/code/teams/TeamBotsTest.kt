package com.lingxi.code.teams

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.nio.file.Files

class TeamBotsTest {
    private fun command() = JSONObject("""{"schemaVersion":1,"commandId":"command_0001","teamId":"team_0001","generation":7,"action":"submit","payload":{"conversationId":"chat_0001","messageId":"message_0001","changeId":"change_0001","text":"Hello","targets":["sharedbot_01"]}}""")
    @Test fun sharedCommandVectors() {
        val root = generateSequence(File(System.getProperty("user.dir"))) { it.parentFile }.first { File(it, "packages/teams/contracts/execution-vectors.json").isFile }
        JSONObject(File(root, "packages/teams/contracts/execution-vectors.json").readText()).getJSONArray("commands").objects().forEach { row ->
            val result = runCatching { TeamExecutionContract.validate(row.getJSONObject("input")) }
            assertEquals(row.getString("name"), row.getBoolean("valid"), result.isSuccess)
        }
    }
    @Test fun durableCommandsSurviveRestartAndStayInTheirAccountTeamGeneration() {
        val directory = Files.createTempDirectory("team-execution-").toFile()
        try {
            val intent = command()
            TeamCommandStorage(directory).update("member", "team_0001", 7) { it.copy(outbox = listOf(intent)) }
            val restored = TeamCommandStorage(directory).load("member", "team_0001", 7)
            assertEquals(intent.toString(), restored.outbox.single().toString())
            TeamExecutionContract.validate(restored.outbox.single())
            assertTrue(TeamCommandStorage(directory).load("other", "team_0001", 7).outbox.isEmpty())
            assertTrue(TeamCommandStorage(directory).load("member", "team_0002", 7).outbox.isEmpty())
            assertTrue(TeamCommandStorage(directory).load("member", "team_0001", 8).outbox.isEmpty())
        } finally { directory.deleteRecursively() }
    }
    @Test fun retiredBotAuthorsAreRejectedAndSharedBotAuthorsNeedNoOwnerUid() {
        val v = JSONObject("""{"id":"message_0001","seq":1,"author":{"kind":"bot","uid":"owner","botKey":"bot:o:h:x"},"kind":"text","text":"Hello","changeSeq":1,"changeId":"change_0001"}""")
        assertTrue(runCatching { TeamMessage.parse(v) }.isFailure)
        v.put("author", JSONObject().put("kind", "shared_bot").put("sharedBotId", "sharedbot_01"))
        assertEquals("sharedbot_01", TeamMessage.parse(v).sharedBotId)
        assertEquals("", TeamMessage.parse(v).authorUid)
    }
    @Test fun retiredBotCommandCleanupPreservesHumanCommandsAndUnrelatedData() {
        val directory = Files.createTempDirectory("team-retired-bot-cleanup-").toFile()
        try {
            val human = JSONObject().put("changeId", "change_0001").put("text", "Human draft")
            val storage = TeamCommandStorage(directory)
            storage.update("member", "team_0001", 7) { it.copy(outbox = listOf(human)) }
            val file = directory.listFiles()!!.single()
            file.writeText(JSONObject(file.readText()).put("answers", JSONArray().put(JSONObject().put("action", "botAnswer"))).put("settlements", JSONArray()).put("businessSentinel", "human history is preserved").toString())
            val restored = storage.load("member", "team_0001", 7)
            assertEquals(human.toString(), restored.outbox.single().toString())
            storage.update("member", "team_0001", 7) { it.copy(outbox = emptyList()) }
            val cleaned = JSONObject(file.readText())
            assertFalse(cleaned.has("answers")); assertFalse(cleaned.has("settlements")); assertEquals("human history is preserved", cleaned.getString("businessSentinel"))
        } finally { directory.deleteRecursively() }
    }
    @Test fun offlineBotQueuesWithoutAnyMemberModelConfiguration() {
        val card = TeamBotCard.parse(JSONObject("""{"sharedBotId":"sharedbot_01","name":"Helper","description":"","maintainer":{"uid":"owner","generation":1},"revision":1,"state":"published","permitted":true,"ready":false,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":100,"member":10,"bot":50}}"""))
        assertTrue(card.selectable)
        requireTeamBotSelections(listOf(card.sharedBotId), listOf(card), listOf("shared_bot:${card.sharedBotId}"))
        assertEquals(setOf(card.sharedBotId), staleTeamBotSelections(listOf(card.sharedBotId), listOf(card), listOf(card.sharedBotId)))
        assertFalse(card.canPause("owner", 2))
    }
}
