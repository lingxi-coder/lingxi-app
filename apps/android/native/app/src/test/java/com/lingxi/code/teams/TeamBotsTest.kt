package com.lingxi.code.teams

import com.lingxi.code.R
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.nio.file.Files

class TeamBotsTest {
    private fun command() = JSONObject("""{"schemaVersion":1,"commandId":"command_0001","teamId":"team_0001","generation":7,"action":"submit","payload":{"conversationId":"chat_0001","messageId":"message_0001","changeId":"change_0001","text":"Hello","targets":["sharedbot_01"]}}""")
    private fun vectors(): JSONObject {
        val root = generateSequence(File(System.getProperty("user.dir"))) { it.parentFile }.first { File(it, "packages/teams/contracts/execution-vectors.json").isFile }
        return JSONObject(File(root, "packages/teams/contracts/execution-vectors.json").readText())
    }
    @Test fun sharedCommandVectors() {
        vectors().getJSONArray("commands").objects().forEach { row ->
            val result = runCatching { TeamExecutionContract.validate(row.getJSONObject("input")) }
            assertEquals(row.getString("name"), row.getBoolean("valid"), result.isSuccess)
        }
    }
    /** Bot v3 B6: the states and codes the contract lists are the ones this app can show and read from the log (`taskStates`, `approvalOutcomes`). */
    @Test fun everyStateOfTheContractIsOneThisAppKnows() {
        val root = vectors()
        val states = root.getJSONArray("taskStates").let { array -> (0 until array.length()).map { array.getString(it) } }
        assertTrue(states.contains("awaiting_approval"))
        for (state in states) {
            assertTrue(state, TeamsBotText.runState(state).label != R.string.chat_status_unknown)
            assertTrue(state, state in TeamTriageRules.outcomes)
        }
        assertEquals("the log reads every state the contract lists, and no other", states.toSet(), TeamTriageRules.outcomes.keys)
        assertEquals(root.getJSONArray("approvalOutcomes").let { array -> (0 until array.length()).map { array.getString(it) } }.toSet(), TeamTriageRules.approvals)
        // Bot v3 B7: the rounds of several Bots: what became of a Bot, the tiers it bids, and the two settings of a Chat.
        fun codes(name: String) = root.getJSONArray(name).let { array -> (0 until array.length()).map { array.getString(it) } }
        assertEquals(codes("roundOutcomes").toSet(), TeamTriageRules.roundOutcomes)
        assertEquals(codes("bidTiers").toSet(), TeamTriageRules.bidTiers)
        assertEquals(codes("humanMentionModes").toSet(), TeamExecutionContract.humanMentionModes)
        assertEquals(root.getInt("maxRepliesLimit"), TeamExecutionContract.MAX_REPLIES_LIMIT)
    }
    /** Bot v3 B7: a member sets how many Bots answer and what a message that mentions only people does, with the Bots of the Chat, or leaves both as they are. */
    @Test fun theAutoReplyCommandCarriesTheTwoSettingsOfTheChatOnlyWhenWholeAndKnown() {
        fun send(more: Map<String, Any?> = emptyMap()) = JSONObject().put("schemaVersion", 1).put("commandId", "command_0001").put("teamId", "team_0001").put("generation", 7).put("action", "setConversationAutoReply")
            .put("payload", JSONObject().put("conversationId", "chat_0001").put("expectedRevision", 2).put("targets", JSONArray(listOf("sharedbot_01"))).also { payload -> more.forEach { (key, value) -> payload.put(key, value) } })
        fun valid(more: Map<String, Any?>) = runCatching { TeamExecutionContract.validate(send(more)) }.isSuccess
        assertTrue("a caller that changes neither leaves them as they are", valid(emptyMap()))
        for (most in 1..TeamExecutionContract.MAX_REPLIES_LIMIT) assertTrue("$most", valid(mapOf("maxReplies" to most)))
        for (mode in listOf("quiet", "judge")) assertTrue(mode, valid(mapOf("humanMentions" to mode)))
        assertTrue(valid(mapOf("maxReplies" to 3, "humanMentions" to "judge")))
        for (most in listOf<Any>(0, TeamExecutionContract.MAX_REPLIES_LIMIT + 1, -1, 1.5, "2", JSONObject.NULL)) assertFalse("$most", valid(mapOf("maxReplies" to most)))
        for (mode in listOf<Any>("ignore", "Quiet", "", 7, JSONObject.NULL)) assertFalse("$mode", valid(mapOf("humanMentions" to mode)))
        assertFalse("nothing else comes with them", valid(mapOf("extra" to true)))
        // A bid is the host's command: a member's session never sends one.
        assertTrue(vectors().getJSONArray("commands").objects().first { it.getString("name") == "reject-bidReply-member" }.let { !it.getBoolean("valid") })
        val rows = vectors().getJSONArray("commands").objects().filter { it.getJSONObject("input").optString("action") == "setConversationAutoReply" }
        assertTrue(rows.size >= 17 && rows.any { it.getBoolean("valid") } && rows.any { !it.getBoolean("valid") })
    }
    /** Bot v3 B7: how many Bots answer one message, and what a message that mentions only people does: kept when whole and known, left out when not, and the Chat still read. */
    @Test fun theTwoSettingsOfTheAutoReplyAreKeptWhenKnownAndLeftOutWhenNot() {
        fun auto(extra: Map<String, Any?>) = TeamMessageConversation.fromDocument("chat_0001", mapOf("kind" to "group", "title" to "Trip", "participants" to listOf("user:alice", "shared_bot:sharedbot_helper"),
            "lastSeq" to 0L, "lastChangeSeq" to 0L, "lastMessage" to null, "autoReply" to (mapOf("revision" to 2L, "targets" to listOf("sharedbot_helper")) + extra)), 0) { null }.autoReply
        assertEquals(TeamConversationAutoReply(2, listOf("sharedbot_helper"), 3, "judge"), auto(mapOf("maxReplies" to 3L, "humanMentions" to "judge")))
        assertEquals(TeamConversationAutoReply(2, listOf("sharedbot_helper"), 1, "quiet"), auto(mapOf("maxReplies" to 1L, "humanMentions" to "quiet")))
        assertEquals(5, auto(mapOf("maxReplies" to 5L))?.maxReplies)
        assertEquals("a whole number that came as a double is still whole", 2, auto(mapOf("maxReplies" to 2.0))?.maxReplies)
        assertEquals("never set: the Chat reads as the default", TeamConversationAutoReply(2, listOf("sharedbot_helper")), auto(emptyMap()))
        for (most in listOf<Any?>(0L, 6L, -1L, 1.5, "2", null, true)) assertNull("$most", auto(mapOf("maxReplies" to most))?.maxReplies)
        for (mode in listOf<Any?>("ignore", "Quiet", "", 7L, null)) assertNull("$mode", auto(mapOf("humanMentions" to mode))?.humanMentions)
        assertEquals("the targets are still read", TeamConversationAutoReply(2, listOf("sharedbot_helper")), auto(mapOf("maxReplies" to 99L, "humanMentions" to "a_new_mode")))
    }
    /** Bot v3 B7b, rule 6: a message sent while the Bot's Desktop was off queues for a day; the task view says so while it is queued and never started. */
    @Test fun aQueuedTaskForADesktopThatWasOffSaysSoAndNothingElseDoes() {
        fun task(state: String, hostWait: String = "") = TeamTask.parse(JSONObject("""{"taskId":"task_0001","conversationId":"chat_0001","sharedBotId":"sharedbot_01","requester":{"uid":"member","generation":3},"state":"$state","createdAt":1,"deadline":2$hostWait}"""))
        val waiting = ""","hostWait":true"""
        assertTrue(task("queued", waiting).waitsForHost)
        assertFalse("a task queued behind a Desktop that is on keeps the old line", task("queued").waitsForHost)
        assertFalse(task("queued", ""","hostWait":false""").waitsForHost)
        assertFalse("when the day is over the existing expired state stands", task("expired", waiting).waitsForHost)
        assertFalse(task("running", waiting).waitsForHost)
        assertTrue("it is still a task a worker will take", task("queued", waiting).busy)
    }
    /** A reply that waits for its maintainer can still change and can be taken back by the one who asked; no worker is changing it. */
    @Test fun aTaskWhoseReplyAwaitsApprovalIsNeitherFinishedNorBeingWorkedOn() {
        fun task(state: String) = TeamTask.parse(JSONObject("""{"taskId":"task_0001","conversationId":"chat_0001","sharedBotId":"sharedbot_01","requester":{"uid":"member","generation":3},"state":"$state","createdAt":1,"deadline":2}"""))
        val requester = TeamMemberRef("member", 3); val maintainer = TeamMemberRef("owner", 1); val other = TeamMemberRef("other", 1)
        val held = task("awaiting_approval")
        assertFalse(held.terminal); assertFalse(held.busy)
        assertTrue(TeamTaskActions.canCancel(held, requester, maintainer, null))
        assertTrue(TeamTaskActions.canCancel(held, maintainer, maintainer, null))
        assertFalse(TeamTaskActions.canCancel(held, other, maintainer, null))
        for (state in listOf("completed", "declined", "failed", "cancelled")) assertTrue(state, task(state).terminal)
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
    @Test fun theListHistorySaysWhoChangedWhatAndKeepsOlderPagesWhenTheNewestIsReadAgain() {
        val newest = TeamSharedListHistory.parse(JSONObject("""{"sharedBotId":"sharedbot_01","dataRevision":4,"hasMore":true,"revisions":[{"dataRevision":4,"author":{"kind":"shared_bot","sharedBotId":"sharedbot_01","taskId":"task_0001","requester":{"uid":"bob","generation":1}},"changes":[{"op":"update","itemId":"item_0001","text":"Whole milk","done":true,"before":{"text":"Milk","done":false}}],"createdAt":3},{"dataRevision":3,"author":{"kind":"user","uid":"alice","generation":1},"changes":[{"op":"remove","itemId":"item_0003","before":{"text":"Wine","done":false}}],"createdAt":2}]}"""))
        val older = TeamSharedListHistory.parse(JSONObject("""{"sharedBotId":"sharedbot_01","dataRevision":4,"hasMore":false,"revisions":[{"dataRevision":2,"author":{"kind":"user","uid":"carol","generation":1},"changes":[{"op":"add","itemId":"item_0002","text":"Oat milk"},{"op":"update","itemId":"item_0001","done":false,"before":{"text":"Milk","done":false}}],"createdAt":1}]}"""))
        val rules = TeamSharedListHistoryRules
        assertEquals(listOf(TeamSharedListHistoryRules.Line.Renamed("Milk", "Whole milk"), TeamSharedListHistoryRules.Line.Ticked("Whole milk")), rules.lines(newest.revisions[0].changes[0]))
        assertEquals(listOf(TeamSharedListHistoryRules.Line.Removed("Wine")), rules.lines(newest.revisions[1].changes[0]))
        assertEquals(listOf(TeamSharedListHistoryRules.Line.Added("Oat milk"), TeamSharedListHistoryRules.Line.Unchanged("Milk")), older.revisions[0].changes.flatMap(rules::lines))
        val members = listOf(TeamMember("alice", "Alice", "owner", 1), TeamMember("bob", "Bob", "member", 1))
        assertEquals(TeamSharedListHistoryRules.Author.Bot("Helper", TeamSharedListHistoryRules.Person.Member("Bob")), rules.author(newest.revisions[0].author, "alice", members, "Helper"))
        assertEquals(TeamSharedListHistoryRules.Author.Of(TeamSharedListHistoryRules.Person.You), rules.author(newest.revisions[1].author, "alice", members, "Helper"))
        assertEquals("carol is not a member", TeamSharedListHistoryRules.Author.Of(TeamSharedListHistoryRules.Person.Former), rules.author(older.revisions[0].author, "alice", members, "Helper"))
        assertEquals("an earlier membership of the same person is not the current one", TeamSharedListHistoryRules.Author.Of(TeamSharedListHistoryRules.Person.Former),
            rules.author(newest.revisions[1].author, "bob", listOf(TeamMember("alice", "Alice", "owner", 2)), "Helper"))
        val both = rules.merge(rules.merge(null, newest, true), older, false)
        assertEquals(listOf(4L, 3L, 2L), both.revisions.map { it.dataRevision }); assertFalse(both.hasMore)
        val again = rules.merge(both, newest, true)
        assertEquals("reading the newest page again keeps the older page", listOf(4L, 3L, 2L), again.revisions.map { it.dataRevision }); assertFalse(again.hasMore)
        fun history(payload: JSONObject) = command().put("action", "listHistory").put("payload", payload)
        TeamExecutionContract.validate(history(JSONObject().put("sharedBotId", "sharedbot_01").put("beforeRevision", 3)))
        assertTrue(runCatching { TeamExecutionContract.validate(history(JSONObject().put("sharedBotId", "sharedbot_01").put("beforeRevision", 0))) }.isFailure)
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

    /** Bot v3 B7b: how many Bots answer one message and whether a message to people alone is judged. Both read as their defaults until someone sets them, and are for a group or channel only. */
    @Test fun autoReplySettingsReadAsTheirDefaultsAndBelongToGroupsAndChannels() {
        fun chat(kind: String, auto: TeamConversationAutoReply?) = TeamMessageConversation("chat_0001", kind, "Trip", listOf("user:alice", "shared_bot:sharedbot_a"), 0, 0, 0, autoReply = auto)
        assertEquals(1, TeamAutoReply.maxReplies(chat("group", null))); assertFalse(TeamAutoReply.judgesHumanMentions(chat("group", null)))
        val unset = chat("channel", TeamConversationAutoReply(1, listOf("sharedbot_a")))
        assertEquals("targets alone leave both at their defaults", 1, TeamAutoReply.maxReplies(unset)); assertFalse(TeamAutoReply.judgesHumanMentions(unset))
        val set = chat("group", TeamConversationAutoReply(2, listOf("sharedbot_a"), 3, "judge"))
        assertEquals(3, TeamAutoReply.maxReplies(set)); assertTrue(TeamAutoReply.judgesHumanMentions(set))
        assertFalse(TeamAutoReply.judgesHumanMentions(chat("group", TeamConversationAutoReply(2, emptyList(), humanMentions = "quiet"))))
        assertEquals("a value past the limit shows as the limit, never a choice the chips lack", TeamExecutionContract.MAX_REPLIES_LIMIT, TeamAutoReply.maxReplies(chat("group", TeamConversationAutoReply(2, emptyList(), 9))))
        assertTrue(TeamAutoReply.hasRounds(chat("group", null))); assertTrue(TeamAutoReply.hasRounds(chat("channel", null)))
        assertFalse("a Bot DM has the one Bot, which is always the one asked", TeamAutoReply.hasRounds(chat("bot_dm", null)))
        assertFalse(TeamAutoReply.hasRounds(chat("dm", null)))
        // A change of either setting is sent with the Bots as they stand, hidden ones kept.
        val a = TeamBotCard.parse(JSONObject("""{"sharedBotId":"sharedbot_a","name":"a","description":"","maintainer":{"uid":"owner","generation":1},"revision":1,"state":"published","permitted":true,"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":1,"member":1,"bot":1}}"""))
        assertEquals(listOf("sharedbot_a", "sharedbot_x"), TeamAutoReply.targets(TeamAutoReply.Choice(listOf(a), listOf("sharedbot_a"), listOf("sharedbot_x"))))
        assertEquals(emptyList<String>(), TeamAutoReply.targets(TeamAutoReply.Choice(listOf(a), emptyList())))
    }
    @Test fun autoReplyOffersPublishedBotsOfTheChatToItsPeopleAndAlwaysLetsOneBeSwitchedOff() {
        fun bot(id: String, state: String = "published", daily: Int? = 5) = TeamBotCard.parse(JSONObject("""{"sharedBotId":"$id","name":"$id","description":"","maintainer":{"uid":"owner","generation":1},"revision":1,"state":"$state","permitted":true,"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":1,"member":1,"bot":1}""" + (daily?.let { ""","autoReply":{"daily":$it}""" } ?: "") + "}"))
        val cards = listOf(bot("sharedbot_a"), bot("sharedbot_b", daily = null), bot("sharedbot_c", state = "paused"), bot("sharedbot_d"))
        assertEquals(TeamBotAutoReply(5), cards[0].autoReply); assertNull(cards[1].autoReply)
        val chat = TeamMessageConversation.fromDocument("chat_0001", mapOf("kind" to "group", "title" to "Trip", "participants" to listOf("user:alice", "shared_bot:sharedbot_a", "shared_bot:sharedbot_b", "shared_bot:sharedbot_c"),
            "lastSeq" to 0L, "lastChangeSeq" to 0L, "lastMessage" to null, "autoReply" to mapOf("revision" to 3L, "targets" to listOf("sharedbot_c"))), 0) { null }
        assertEquals(TeamConversationAutoReply(3, listOf("sharedbot_c")), chat.autoReply)
        val offered = TeamAutoReply.bots(chat, cards, "alice")
        assertEquals(listOf("sharedbot_a", "sharedbot_c"), offered.bots.map { it.sharedBotId })
        assertEquals(listOf("sharedbot_c"), offered.on)
        assertEquals(listOf("sharedbot_a", "sharedbot_c"), TeamAutoReply.targets(offered, "sharedbot_a", true))
        assertEquals(emptyList<String>(), TeamAutoReply.targets(offered, "sharedbot_c", false))
        assertTrue("only the Chat's people change it", TeamAutoReply.bots(chat, cards, "bob").bots.isEmpty())
        // A Bot on in the Chat that this person cannot see (published to others only) stays on whatever they switch.
        val hiddenChat = TeamMessageConversation.fromDocument("chat_0003", mapOf("kind" to "group", "title" to "Trip", "participants" to listOf("user:alice", "shared_bot:sharedbot_a", "shared_bot:sharedbot_x"),
            "lastSeq" to 0L, "lastChangeSeq" to 0L, "lastMessage" to null, "autoReply" to mapOf("revision" to 4L, "targets" to listOf("sharedbot_x", "sharedbot_gone"))), 0) { null }
        val hidden = TeamAutoReply.bots(hiddenChat, cards, "alice")
        assertEquals(emptyList<String>(), hidden.on)
        assertEquals("a Bot no longer in the Chat is not kept", listOf("sharedbot_x"), hidden.kept)
        assertEquals(listOf("sharedbot_a", "sharedbot_x"), TeamAutoReply.targets(hidden, "sharedbot_a", true))
        val unreadable = TeamMessageConversation.fromDocument("chat_0002", mapOf("kind" to "group", "title" to "Trip", "participants" to listOf("user:alice"), "lastSeq" to 0L, "lastChangeSeq" to 0L, "lastMessage" to null,
            "autoReply" to mapOf("revision" to "3", "targets" to listOf("sharedbot_c"))), 0) { null }
        assertNull(unreadable.autoReply)
        assertEquals(R.string.teams_task_declined, TeamsBotText.runState("declined").label)
    }
}
