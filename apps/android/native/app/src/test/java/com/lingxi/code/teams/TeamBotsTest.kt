package com.lingxi.code.teams

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files

class TeamBotsTest {
    private val key = "bot:owner:abcdefghijklmnopqrstuv:abcdefghijklmnopqrstuv"
    private fun card() = TeamBotCard(key, "owner", "Bot", "Description", 8, "all", true, true)
    private fun message() = TeamMessage("question-message", 2, "owner", "bot", "Please answer", 2, "question-change", false, "source-message", null,
        "question", key, "trigger001", "human", 4, TeamBotQuestion("question01", listOf("Which option?", "Why?")))
    private fun run() = TeamBotRun("running", "owner", key, "human", 4, 1000, "question01", false)
    private fun intent(cache: TeamMessageCache = TeamMessageCache("human", "team0001", 4)) =
        teamBotAnswerIntent(cache, "chat0001", message(), run(), mapOf("Which option?" to "A", "Why?" to "Clear"), "answer001", 100)

    @Test fun originalHumanGenerationAndLiveQuestionAreRequired() {
        assertTrue(run().canAnswer(message(), "human", 4, 100))
        assertFalse(run().canAnswer(message(), "owner", 4, 100))
        assertFalse(run().canAnswer(message(), "human", 5, 100))
        assertFalse(run().copy(originUid = "another").canAnswer(message(), "human", 4, 100))
        assertFalse(run().copy(questionId = "new-question").canAnswer(message(), "human", 4, 100))
        assertFalse(run().copy(answered = true).canAnswer(message(), "human", 4, 100))
        assertFalse(run().canAnswer(message(), "human", 4, 1000))
        listOf("completed", "revoked", "recovery", "failed", "queued").forEach { state -> assertFalse(run().copy(state = state).canAnswer(message(), "human", 4, 100)) }
    }

    @Test fun durableAnswerAndBotMetadataSurviveRestartWithoutCrossGenerationLeak() {
        val directory = Files.createTempDirectory("team-bot-answer-").toFile()
        try {
            val answer = intent()
            val conversation = TeamMessageConversation("chat0001", "bot_dm", "Bot", listOf("user:human", key), 2, 2, 0, listOf(key))
            TeamMessageStorage(directory).update("human", "team0001", 4) { it.copy(conversations = listOf(conversation), threads = mapOf("chat0001" to TeamMessageThread(2, listOf(message()))), answers = listOf(answer), outbox = listOf(JSONObject().put("targets", JSONArray(listOf(key))))) }
            val restored = TeamMessageStorage(directory).update("human", "team0001", 4) { it }
            assertEquals(answer.toString(), restored.answers.single().toString())
            assertEquals("answer001", restored.answers.single().getString("answerId"))
            assertEquals(message(), restored.threads.getValue("chat0001").messages.single())
            assertEquals(listOf(key), restored.conversations.single().botKeys)
            assertEquals(key, restored.outbox.single().getJSONArray("targets").getString(0))
            assertTrue(TeamMessageStorage(directory).update("human", "team0001", 5) { it }.answers.isEmpty())
            assertTrue(TeamMessageStorage(directory).update("other", "team0001", 4) { it }.answers.isEmpty())
        } finally { directory.deleteRecursively() }
    }

    @Test fun answerRetriesPermitOnlyCurrentUnexpiredRunOrExactAcceptedReceipt() {
        val pending = intent()
        assertTrue(run().canRetryAnswer(pending, "human", 4, 100))
        assertFalse(run().canRetryAnswer(pending, "human", 4, 1000))
        assertFalse(run().copy(state = "completed").canRetryAnswer(pending, "human", 4, 100))
        assertTrue(run().copy(state = "completed", answered = true).canRetryAnswer(pending, "human", 4, 2000))
        assertFalse(run().copy(answered = true).canRetryAnswer(pending, "other", 4, 100))
        assertFalse(run().copy(answered = true).canRetryAnswer(pending, "human", 5, 100))
        try { intent(TeamMessageCache("human", "team0001", 4, answers = listOf(pending))); fail("Duplicate question intent") } catch (expected: TeamMessageFailure) { assertEquals("ANSWER_PENDING", expected.code) }
    }

    @Test fun textMentionsNeverBecomeExecutionTargets() {
        val human = message().copy(authorKind = "user", text = "@Bot please execute", triggerId = null, question = null, targets = emptyList())
        val group = TeamMessageConversation("chat0001", "group", "Group", listOf("user:human", key), 0, 0, 0, listOf(key))
        assertTrue(teamBotTriggers(human, group).isEmpty())
        val explicit = human.copy(targets = listOf(key))
        assertEquals(teamBotTriggers(explicit, group), teamBotTriggers(human, group.copy(kind = "bot_dm")))
        assertEquals(64, teamBotTriggers(explicit, group).single().length)
        assertTrue(teamBotTriggers(explicit.copy(authorKind = "bot"), group).isEmpty())
        val wire = human.json().also { it.remove("targets"); it.put("mentions", JSONArray(listOf(key, "user:other"))) }
        assertEquals(listOf(key), TeamMessage.parse(wire).targets)
    }

    @Test fun onlyOwnerCanSettleExpiredOrRecoveryRun() {
        assertFalse(run().canRecover("owner", 100))
        assertTrue(run().canRecover("owner", 1000))
        assertTrue(run().copy(state = "claimed").canRecover("owner", 1000))
        assertFalse(run().copy(state = "claimed").canRecover("owner", 999))
        assertTrue(run().copy(state = "recovery").canRecover("owner", 100))
        assertFalse(run().copy(state = "recovery").canRecover("human", 100))
        assertFalse(run().copy(state = "completed").canRecover("owner", 1000))
    }

    @Test fun inactiveOwnerCardPreservesRevisionAndCannotBeSelectedOrUnboundAgain() {
        val wire = JSONObject().put("key", key).put("ownerUid", "owner").put("name", "Bot").put("description", "Description")
            .put("revision", 9).put("use", "all").put("permitted", true).put("online", true).put("enabled", false)
        val inactive = TeamBotCard.parse(wire)
        assertFalse(inactive.enabled)
        assertFalse(inactive.selectable)
        assertFalse(inactive.canUnbind("owner"))
        assertEquals("Inactive · revision 9. Rebind on Desktop.", inactive.inactiveLabel)
        assertTrue(card().canUnbind("owner"))
        assertFalse(card().canUnbind("other"))
        assertFalse(card().copy(permitted = false).selectable)
        wire.remove("enabled")
        assertTrue(TeamBotCard.parse(wire).selectable)
    }

    @Test fun staleBotDraftBlocksWithoutSilentlyRemovingIntendedTargets() {
        val valid = card().copy(key = "$key-valid")
        val unavailable = card().copy(enabled = false)
        val draft = setOf(key, valid.key)
        val stale = staleTeamBotSelections(draft, listOf(unavailable, valid), draft)
        assertEquals(setOf(key), stale)
        assertEquals(setOf(key, valid.key), draft)
        try { requireTeamBotSelections(draft, listOf(unavailable, valid), draft); fail("Silent downgrade") } catch (expected: TeamMessageFailure) { assertEquals("BOT_SELECTION_STALE", expected.code) }
        // This is the explicit clear action: it preserves every still-valid selection.
        val explicitlyCleared = draft - stale
        assertEquals(setOf(valid.key), explicitlyCleared)
        requireTeamBotSelections(explicitlyCleared, listOf(unavailable, valid), draft)
        assertEquals(setOf(valid.key), staleTeamBotSelections(explicitlyCleared, listOf(valid.copy(permitted = false)), draft))
        assertEquals(setOf(valid.key), staleTeamBotSelections(explicitlyCleared, listOf(valid), emptySet()))
        assertEquals(setOf(valid.key), staleTeamBotSelections(explicitlyCleared, emptyList(), draft))
    }

    @Test fun creationParticipantDraftAndBotDmRequireSelectableBots() {
        val active = card()
        requireTeamBotSelections(setOf(key), listOf(active))
        requireTeamBotSelections(setOf(key), listOf(active), setOf(key))
        assertEquals(setOf(key), staleTeamBotSelections(setOf(key), listOf(active.copy(enabled = false))))
        assertEquals(setOf(key), staleTeamBotSelections(setOf(key), listOf(active.copy(permitted = false))))
        assertEquals(setOf(key), staleTeamBotSelections(setOf(key), emptyList(), setOf(key)))
        requireTeamBotSelections(emptySet(), emptyList())
        assertTrue(staleTeamBotSelections(setOf(key), listOf(active), setOf(key)).isEmpty())
    }

    @Test fun expandedHistoryObservesOldQuestionAndRecoveryBeyondLastHundredMessages() {
        val oldQuestion = message().copy(id = "old-question", seq = 1, triggerId = "old-run")
        val oldRecovery = message().copy(id = "old-interrupted", seq = 2, triggerId = "old-recovery", question = null)
        val newer = (3L..202L).map { seq -> message().copy(id = "message-$seq", seq = seq, triggerId = null, question = null) }
        val messages = listOf(oldQuestion, oldRecovery) + newer
        assertTrue(visibleTeamBotTriggers(messages, null, 100).isEmpty())
        assertTrue(visibleTeamBotTriggers(messages, null, 200).isEmpty())
        assertEquals(listOf("old-run", "old-recovery"), visibleTeamBotTriggers(messages, null, 300))
        assertTrue(run().canAnswer(oldQuestion, "human", 4, 100))
        assertTrue(run().copy(state = "recovery").canRecover("owner", 100))
    }

    @Test fun historicalAnswerReceiptConfirmsAWhileActiveQuestionBRemainsUntouched() {
        val pendingA = intent()
        val activeB = run().copy(questionId = "question02")
        val activeStates = mapOf("trigger001" to activeB)
        assertFalse(activeB.canRetryAnswer(pendingA, "human", 4, 100))
        val observation = teamBotStateQuery("chat0001", "trigger001", pendingA.getString("questionId"))
        assertEquals("question01", observation.getString("questionId"))
        val historicalA = run().copy(answered = true)
        assertTrue(historicalA.canRetryAnswer(pendingA, "human", 4, 100))
        assertEquals("question02", activeStates.getValue("trigger001").questionId)
        assertFalse(activeStates.getValue("trigger001").answered)
        assertFalse(teamBotStateQuery("chat0001", "trigger001").has("questionId"))
    }

    @Test fun settlementExactWireBodyAndIdentitySurviveAckLossRestartAndGenerationIsolation() {
        val directory = Files.createTempDirectory("team-bot-settlement-").toFile()
        try {
            val cache = TeamMessageCache("owner", "team0001", 4)
            val pending = teamBotSettlementIntent(cache, "chat0001", "trigger001", run().copy(state = "recovery"), "Checked the interrupted effects.", "settlement001", 100)
            val exactBody = pending.getJSONObject("body").toString()
            assertFalse(pending.getJSONObject("body").has("settlementId"))
            assertEquals("recoverBot", pending.getJSONObject("body").getString("action"))
            TeamMessageStorage(directory).update("owner", "team0001", 4) { it.copy(settlements = listOf(JSONObject(pending.toString()).put("attempted", true).put("leaseUntil", 1000))) }
            val restored = TeamMessageStorage(directory).update("owner", "team0001", 4) { it }
            val retry = restored.settlements.single()
            assertEquals("settlement001", retry.getString("settlementId"))
            assertEquals(exactBody, retry.getJSONObject("body").toString())
            assertTrue(restored.answers.isEmpty())
            assertFalse(canDiscardTeamBotIntent(retry, 1001))
            assertTrue(canDiscardTeamBotIntent(pending, 100))
            assertFalse(canDiscardTeamBotIntent(JSONObject(retry.toString()).put("error", "BOT_RESULT_CONFLICT"), 999))
            assertTrue(canDiscardTeamBotIntent(JSONObject(retry.toString()).put("error", "BOT_RESULT_CONFLICT"), 1001))
            try { teamBotSettlementIntent(restored, "chat0001", "trigger001", run().copy(state = "recovery"), "Different text", "replacement", 100); fail("Duplicate settlement") } catch (expected: TeamMessageFailure) { assertEquals("MESSAGE_IN_FLIGHT", expected.code) }
            assertTrue(TeamMessageStorage(directory).update("owner", "team0001", 5) { it }.settlements.isEmpty())
            assertTrue(TeamMessageStorage(directory).update("another", "team0001", 4) { it }.settlements.isEmpty())
        } finally { directory.deleteRecursively() }
    }

    @Test fun attemptedAnswerRetriesExactReceiptAfterRevocationWithoutMutableStateAuthority() {
        val directory = Files.createTempDirectory("team-answer-revoked-receipt-").toFile()
        try {
            val original = intent()
            val attempted = JSONObject(original.toString()).put("attempted", true)
            TeamMessageStorage(directory).update("human", "team0001", 4) { it.copy(answers = listOf(attempted)) }
            val restored = TeamMessageStorage(directory).update("human", "team0001", 4) { it }
            // ACK was lost, then the owner unbound/rebound or the source was
            // recalled. Public status deliberately hides the old question.
            val revoked = run().copy(state = "revoked", questionId = null, answered = false)
            assertFalse(revoked.canRetryAnswer(attempted, "human", 4, 2000))
            assertEquals(original.toString(), teamBotAnswerRetryBody(restored, restored.answers.single(), revoked, 2000).toString())
            assertEquals(original.toString(), teamBotAnswerRetryBody(restored, restored.answers.single(), null, 2000).toString())
            try { teamBotAnswerRetryBody(restored, original, revoked, 2000); fail("A new answer cannot use revoked state") } catch (expected: TeamMessageFailure) { assertEquals("QUESTION_RECIPIENT_REQUIRED", expected.code) }
            try { teamBotAnswerRetryBody(restored.copy(generation = 5), attempted, null, 2000); fail("Generation changed") } catch (expected: TeamMessageFailure) { assertEquals("INVALID_SESSION", expected.code) }
            try { teamBotAnswerRetryBody(restored.copy(teamId = "other-team"), attempted, null, 2000); fail("Team changed") } catch (expected: TeamMessageFailure) { assertEquals("INVALID_SESSION", expected.code) }
            assertTrue(TeamMessageStorage(directory).update("other-human", "team0001", 4) { it }.answers.isEmpty())
        } finally { directory.deleteRecursively() }
    }
}
