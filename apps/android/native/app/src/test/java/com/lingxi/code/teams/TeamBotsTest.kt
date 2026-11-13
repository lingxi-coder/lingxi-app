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
    private fun card(extra: String = "", permitted: Boolean = true, state: String = "published") = TeamBotCard.parse(JSONObject(
        """{"sharedBotId":"sharedbot_01","name":"Helper","description":"","maintainer":{"uid":"owner","generation":3},"revision":2,"state":"$state","permitted":$permitted,"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":1,"member":1,"bot":1}$extra}"""))
    @Test fun theListIsOfferedOnlyToThoseWhoMayReadIt() {
        val listed = ""","sharedList":{"title":"Groceries","approval":"requester"}"""
        assertNull(card().sharedList)
        assertEquals(TeamBotListInfo("Groceries", "requester"), card(listed).sharedList)
        assertFalse(TeamSharedListRules.canOpen(card(), "member", 3))
        assertTrue(TeamSharedListRules.canOpen(card(listed), "member", 3))
        // Not allowed to use the Bot: only its maintainer still reads the list.
        assertFalse(TeamSharedListRules.canOpen(card(listed, permitted = false), "member", 3))
        assertTrue(TeamSharedListRules.canOpen(card(listed, permitted = false), "owner", 3))
        assertFalse(TeamSharedListRules.canOpen(card(listed, state = "paused"), "owner", 3))
    }
    @Test fun aRepeatedListSaveKeepsItsOperationAndAnyOtherChangeGetsANewOne() {
        var made = 0
        val id = { made += 1; "operation_000$made" }
        val bread = listOf<TeamSharedListChange>(TeamSharedListChange.Add("Bread"))
        val first = TeamSharedListOperation.of(null, 4, bread, id)
        assertEquals("operation_0001", first.id)
        // The answer was lost and the person saves again: the same request.
        assertEquals(first, TeamSharedListOperation.of(first, 4, listOf(TeamSharedListChange.Add("Bread")), id))
        // Other text, or the same text on a newer list, is another change.
        assertEquals("operation_0002", TeamSharedListOperation.of(first, 4, listOf(TeamSharedListChange.Add("Milk")), id).id)
        assertEquals("operation_0003", TeamSharedListOperation.of(first, 5, bread, id).id)
    }
    @Test fun everyListChangeIsACommandTheServiceAccepts() {
        fun write(changes: List<TeamSharedListChange>) = command().put("action", "writeList")
            .put("payload", JSONObject().put("sharedBotId", "sharedbot_01").put("operationId", "operation_0001").put("expectedDataRevision", 0).put("changes", changes.json()))
        TeamExecutionContract.validate(write(listOf(TeamSharedListChange.Add("Bread"), TeamSharedListChange.SetText("item_0001", "Rye bread"),
            TeamSharedListChange.SetDone("item_0001", true), TeamSharedListChange.Remove("item_0002"))))
        assertTrue(TeamSharedListRules.usable(" Bread "))
        assertFalse(TeamSharedListRules.usable("  \n"))
        // 500 bytes fit; one more does not, and multi-byte text is counted in bytes.
        assertTrue(TeamSharedListRules.usable("a".repeat(500)))
        assertFalse(TeamSharedListRules.usable("a".repeat(501)))
        assertFalse(TeamSharedListRules.fits("\u6e05".repeat(167)))
        assertTrue(runCatching { TeamExecutionContract.validate(write(listOf(TeamSharedListChange.Add("a".repeat(501))))) }.isFailure)
    }
    @Test fun aListThatMovedIsReadAgain() {
        assertTrue(TeamSharedListRules.moved("LIST_REVISION_CONFLICT"))
        assertTrue(TeamSharedListRules.moved("LIST_ITEM_REQUIRED"))
        assertFalse(TeamSharedListRules.moved("LIST_LIMIT"))
        val list = TeamSharedList.parse(JSONObject("""{"sharedBotId":"sharedbot_01","title":"Groceries","description":"","dataRevision":2,"items":[{"itemId":"item_0001","text":"Bread","done":true}],"updatedAt":5}"""))
        assertEquals(listOf(TeamSharedListItem("item_0001", "Bread", true)), list.items)
        assertEquals(2L, list.dataRevision)
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
    @Test fun aDocumentEditIsValidOnlyAsTheServiceStoresIt() {
        val base = TeamArtifact.parse(JSONObject("""{"artifactId":"artifact_0001","revision":2,"digest":"b","document":{"title":"Plan","body":"One"},"currentRevision":3}"""))
        assertTrue(base.old)
        assertFalse(TeamDocumentDraft(base).changed(base))
        assertFalse("Spaces around the title are not a change", TeamDocumentDraft(" Plan ", "One").changed(base))
        assertTrue(TeamDocumentDraft("Plan", "Two").changed(base))
        assertTrue(TeamDocumentDraft("Plan", "Two").valid)
        assertFalse(TeamDocumentDraft("  ", "Two").valid)
        assertFalse(TeamDocumentDraft("x".repeat(121), "Two").valid)
        assertFalse(TeamDocumentDraft("Plan", "").valid)
        val long = TeamDocumentDraft("Plan", "é".repeat(TeamExecutionContract.DOCUMENT_BYTES / 2 + 1))
        assertTrue(long.tooLong); assertFalse(long.valid)
        assertEquals("""{"title":"Plan","body":"Two"}""", TeamDocumentDraft(" Plan ", "Two").json().toString())
    }
    @Test fun aRepeatedDocumentSaveKeepsItsOperationAndAnyOtherEditGetsANewOne() {
        var made = 0
        val id = { made += 1; "operation_000$made" }
        val draft = TeamDocumentDraft("Plan ", "Two")
        val first = TeamDocumentOperation.of(null, 3, draft, id)
        assertEquals(first, TeamDocumentOperation.of(first, 3, TeamDocumentDraft("Plan", "Two"), id))
        assertEquals("operation_0002", TeamDocumentOperation.of(first, 3, TeamDocumentDraft("Plan", "Three"), id).id)
        // Saving the same text over a newer revision is another decision.
        assertEquals("operation_0003", TeamDocumentOperation.of(first, 4, draft, id).id)
    }
    @Test fun theHistoryNamesMembersAndBotsAndKeepsAGenericNameForThoseWhoLeft() {
        val history = TeamArtifactRevisions.parse(JSONObject("""{"artifactId":"artifact_0001","currentRevision":2,"revisions":[{"revision":2,"parentRevision":1,"digest":"b","title":"Plan","author":{"kind":"user","uid":"member","generation":3},"createdAt":2000},{"revision":1,"parentRevision":null,"digest":"a","title":"Plan","author":{"kind":"shared_bot","sharedBotId":"sharedbot_01"},"createdAt":1000,"sourceTaskId":"task_0001"}]}"""))
        val members = listOf(TeamMember("member", "Ada", "member", 3))
        val strings = TeamsListStrings.ENGLISH
        assertEquals(listOf(2L, 1L), history.revisions.map { it.revision })
        assertNull(history.revisions[1].parentRevision)
        assertEquals("task_0001", history.revisions[1].sourceTaskId)
        assertEquals("Ada", TeamDocumentRules.authorName(history.revisions[0].author, members, listOf(card()), strings))
        assertEquals("Helper", TeamDocumentRules.authorName(history.revisions[1].author, members, listOf(card()), strings))
        assertEquals(strings.member, TeamDocumentRules.authorName(history.revisions[0].author, emptyList(), emptyList(), strings))
        assertEquals(strings.bot, TeamDocumentRules.authorName(history.revisions[1].author, emptyList(), emptyList(), strings))
    }
    @Test fun onlyAnActionNobodyCouldVerifyAsksForADecision() {
        val listed = TeamTaskOperation.list(JSONObject("""{"taskId":"task_0001","operations":[{"operationId":"a","logicalStepId":"s1","category":"queryable","tool":{"name":"send_mail","version":"1"},"state":"dispatched","epoch":1,"dispatchedAt":1000},{"operationId":"b","logicalStepId":"s2","category":"unqueryable","tool":{"name":"post","version":"1"},"state":"unknown","epoch":1,"dispatchedAt":1000},{"operationId":"c","logicalStepId":"s3","category":"unqueryable","tool":{"name":"post","version":"1"},"state":"succeeded","epoch":1,"dispatchedAt":1000,"settledAt":2000,"resolvedBy":{"uid":"owner","generation":1},"resolvedAt":2000},{"operationId":"d","logicalStepId":"s4","category":"queryable","tool":{"name":"post","version":"1"},"state":"failed","epoch":1,"dispatchedAt":1000},{"operationId":"e","logicalStepId":"s5","category":"store_idempotent","tool":{"name":"read","version":"1"},"state":"dispatched","epoch":1,"dispatchedAt":1000}]}"""))
        assertEquals(listOf(true, true, false, false, false), listed.map { it.unverified })
        assertEquals(listOf(com.lingxi.code.R.string.teams_ops_state_unreported, com.lingxi.code.R.string.teams_ops_state_unknown, com.lingxi.code.R.string.teams_ops_state_happened,
            com.lingxi.code.R.string.teams_ops_state_not_happened, com.lingxi.code.R.string.teams_ops_state_repeatable), listed.map(TeamOperationRules::stateRes))
        assertEquals(TeamMemberRef("owner", 1), listed[2].resolvedBy)
        assertEquals("send_mail", listed[0].toolName); assertNull(listed[0].settledAt)
    }
    @Test fun theRequesterSeesTheActionsAndOnlyTheMaintainerConfirmsThem() {
        fun task(state: String) = TeamTask.parse(JSONObject("""{"taskId":"task_0001","conversationId":"chat_0001","sharedBotId":"sharedbot_01","requester":{"uid":"member","generation":3},"state":"$state","createdAt":1,"deadline":2,"replyId":null,"artifactId":null}"""))
        val requester = TeamMemberRef("member", 3); val maintainer = TeamMemberRef("owner", 3); val other = TeamMemberRef("other", 1)
        val unchecked = task("needs_reconciliation")
        assertTrue(TeamOperationRules.canReview(unchecked, requester, maintainer))
        assertTrue(TeamOperationRules.canReview(unchecked, maintainer, maintainer))
        assertFalse(TeamOperationRules.canReview(unchecked, other, maintainer))
        assertFalse("A member who rejoined is another member", TeamOperationRules.canReview(unchecked, TeamMemberRef("member", 4), maintainer))
        assertFalse(TeamOperationRules.canReview(unchecked, null, maintainer))
        assertFalse(TeamOperationRules.canReview(task("failed"), maintainer, maintainer))
        assertTrue(TeamOperationRules.canResolve(maintainer, maintainer))
        assertFalse(TeamOperationRules.canResolve(requester, maintainer))
        assertFalse(TeamOperationRules.canResolve(null, null))
        assertTrue(TeamOperationRules.maintains("task_0001", listOf(unchecked), listOf(card()), maintainer))
        assertFalse(TeamOperationRules.maintains("task_0001", listOf(unchecked), listOf(card()), requester))
        assertFalse("An unknown task or Bot gives nobody the choice", TeamOperationRules.maintains("task_0002", listOf(unchecked), listOf(card()), maintainer))
        assertEquals(com.lingxi.code.R.string.teams_error_ops_maintainer, TeamOperationRules.errorRes("MAINTAINER_REQUIRED"))
        assertEquals(com.lingxi.code.R.string.teams_error_ops_running, TeamOperationRules.errorRes("TASK_RUNNING"))
        assertEquals(teamsErrorRes("MEMBERSHIP_REQUIRED"), TeamOperationRules.errorRes("MEMBERSHIP_REQUIRED"))
    }
    @Test fun offlineBotQueuesWithoutAnyMemberModelConfiguration() {
        val card = TeamBotCard.parse(JSONObject("""{"sharedBotId":"sharedbot_01","name":"Helper","description":"","maintainer":{"uid":"owner","generation":1},"revision":1,"state":"published","permitted":true,"ready":false,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":100,"member":10,"bot":50}}"""))
        assertTrue(card.selectable)
        requireTeamBotSelections(listOf(card.sharedBotId), listOf(card), listOf("shared_bot:${card.sharedBotId}"))
        assertEquals(setOf(card.sharedBotId), staleTeamBotSelections(listOf(card.sharedBotId), listOf(card), listOf(card.sharedBotId)))
        assertFalse(card.canPause("owner", 2))
    }

    private fun groupBot(permitted: Boolean = true) = TeamBotCard.parse(JSONObject("""{"sharedBotId":"sharedbot_01","name":"Helper","description":"","maintainer":{"uid":"owner","generation":1},"revision":1,"state":"published","permitted":$permitted,"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":100,"member":10,"bot":50}}"""))
    private fun groupConversation(participants: List<String>, sharedBotIds: List<String>) =
        TeamMessageConversation("chat_0001", "group", "Group", participants, 0, 0, 0, sharedBotIds)

    @Test fun composerAcceptsAPublishedBotThatIsAGroupParticipant() {
        val conversation = groupConversation(listOf("user:alice", "shared_bot:sharedbot_01"), listOf("sharedbot_01"))
        assertEquals(emptySet<String>(), composerStaleTargets(setOf("sharedbot_01"), listOf(groupBot()), conversation))
    }

    @Test fun composerFlagsABotThatIsNotPermittedOrNotAParticipant() {
        val member = groupConversation(listOf("user:alice", "shared_bot:sharedbot_01"), listOf("sharedbot_01"))
        assertEquals(setOf("sharedbot_01"), composerStaleTargets(setOf("sharedbot_01"), listOf(groupBot(permitted = false)), member))
        val other = groupConversation(listOf("user:alice"), emptyList())
        assertEquals(setOf("sharedbot_01"), composerStaleTargets(setOf("sharedbot_01"), listOf(groupBot()), other))
    }
}
