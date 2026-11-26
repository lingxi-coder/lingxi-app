package com.lingxi.code.teams

import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.yield
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.File

/**
 * The replies of a Bot that wait for its maintainer (Bot v3 B6b): the replies as the service writes them, what the maintainer may send, the commands, whose replies a person
 * decides on and where they are shown, the list and the decisions on it, the time, and the words in every language. No UI, no network: the screens are `TeamHeldViews.kt`.
 */
class TeamHeldRepliesTest {
    private val viewer = TeamMemberRef("alice", 1)
    private val minute = 60_000L
    private val start = 1_800_000_000_000L

    /** One held reply as the service writes it; [n] is its number, newer replies have the larger one. */
    private fun wire(n: Int = 1, unasked: Boolean = false, extra: Map<String, Any?> = emptyMap()): JSONObject {
        val value = JSONObject().put("taskId", "task-%04d".format(n)).put("conversationId", "chat_0001").put("messageId", "message-$n").put("requester", JSONObject().put("uid", "bob").put("generation", 1))
            .put("unasked", unasked).put("heldAt", start + n * 1000L).put("expiresAt", start + n * 1000L + 10 * minute).put("text", "Dinner at 7 works for me.").put("conversationTitle", "Planning")
        for ((key, replacement) in extra) value.put(key, replacement ?: JSONObject.NULL)
        return value
    }
    private fun reply(n: Int = 1, unasked: Boolean = false, bot: String = "sharedbot_01", extra: Map<String, Any?> = emptyMap()): TeamHeldReply =
        checkNotNull(TeamHeldReply.parse(wire(n, unasked, extra))).copy(sharedBotId = bot)
    private fun card(id: String, maintainer: String = "alice", generation: Long = 1, daily: Int? = null): TeamBotCard {
        val auto = if (daily != null) ",\"autoReply\":{\"daily\":$daily}" else ""
        return TeamBotCard.parse(JSONObject(
            """{"sharedBotId":"$id","name":"$id","description":"","maintainer":{"uid":"$maintainer","generation":$generation},"revision":1,"state":"published","permitted":true,"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":1,"member":1,"bot":1}$auto}"""))
    }
    private fun chat(participants: List<String>, kind: String = "group") = TeamMessageConversation(
        id = "chat_0001", kind = kind, title = "Trip", participants = participants, lastSeq = 0, lastChangeSeq = 0, lastReadSeq = 0,
        sharedBotIds = participants.filter { it.startsWith("shared_bot:") }.map { it.removePrefix("shared_bot:") },
    )
    private fun row(n: Int, unasked: Boolean = false, bot: String = "sharedbot_01", chat: String = "chat_0001", title: String? = "Planning") =
        TeamHeldRow(reply(n, unasked, bot, mapOf("conversationId" to chat, "conversationTitle" to title)))

    // The replies as the service writes them

    @Test fun aReplyIsReadAsTheServiceWroteIt() {
        val full = checkNotNull(TeamHeldReply.parse(wire(7, unasked = true, extra = mapOf("document" to JSONObject().put("title", "Dinner plan").put("body", "1. Book a table")))))
        assertEquals(TeamHeldReply("task-0007", "chat_0001", "message-7", TeamMemberRef("bob", 1), true, start + 7000, start + 7000 + 10 * minute, "Dinner at 7 works for me.",
            TeamHeldDocument("Dinner plan", "1. Book a table"), "Planning"), full)
        assertEquals("the service does not say whose reply it is: the screen that asked tags it", "", full.sharedBotId)
        val plain = checkNotNull(TeamHeldReply.parse(wire(8).also { it.remove("conversationTitle") }))
        assertNull(plain.document); assertNull(plain.conversationTitle); assertFalse(plain.unasked)
        assertNull("an empty title is no title", TeamHeldReply.parse(wire(9, extra = mapOf("conversationTitle" to "")))?.conversationTitle)
        assertNull("a title that is not text is no title; the reply is still one", checkNotNull(TeamHeldReply.parse(wire(9, extra = mapOf("conversationTitle" to 7)))).conversationTitle)
        assertEquals("a document may have no body", "", TeamHeldReply.parse(wire(10, extra = mapOf("document" to JSONObject().put("title", "T").put("body", "")))) ?.document?.body)
    }

    @Test fun aReplyThisVersionCannotReadIsNotOneAndAReplyIsNeverHalfShown() {
        val unreadable = listOf(
            wire(1, extra = mapOf("taskId" to "")), wire(1, extra = mapOf("conversationId" to "")), wire(1, extra = mapOf("messageId" to "")), wire(1, extra = mapOf("text" to "")), wire(1, extra = mapOf("text" to 5)),
            wire(1, extra = mapOf("unasked" to "no")), wire(1, extra = mapOf("unasked" to null)), wire(1, extra = mapOf("heldAt" to "1")), wire(1, extra = mapOf("expiresAt" to 9.0e15)), wire(1, extra = mapOf("heldAt" to null)),
            wire(1, extra = mapOf("requester" to JSONObject().put("uid", "").put("generation", 1))), wire(1, extra = mapOf("requester" to JSONObject().put("uid", "bob").put("generation", -1))),
            wire(1, extra = mapOf("requester" to JSONObject().put("uid", "bob").put("generation", 1.5))), wire(1, extra = mapOf("requester" to "bob")),
            // The maintainer would be letting go of something they did not see.
            wire(1, extra = mapOf("document" to null)), wire(1, extra = mapOf("document" to "text")), wire(1, extra = mapOf("document" to JSONObject().put("title", "").put("body", "x"))),
            wire(1, extra = mapOf("document" to JSONObject().put("title", "T"))), wire(1, extra = mapOf("document" to JSONObject().put("title", "T").put("body", 5))), JSONObject().put("nonsense", true),
            wire(1).also { it.remove("heldAt") },
        )
        for (value in unreadable) assertNull("$value", TeamHeldReply.parse(value))
        assertNull(TeamHeldReply.parse(null))
    }

    @Test fun theAnswerKeepsTheRepliesItCanReadAndAnAnswerThatIsNoListIsNotAnEmptyOne() {
        val answer = JSONObject().put("replies", JSONArray().put(wire(3)).put(JSONObject().put("nonsense", true)).put(7).put(JSONObject.NULL).put(wire(1, extra = mapOf("text" to ""))).put(wire(2, unasked = true)))
        assertEquals("the replies around one that cannot be read are still shown", listOf("task-0003", "task-0002"), TeamHeldAnswer.parse(answer).map { it.taskId })
        assertTrue(TeamHeldAnswer.parse(JSONObject().put("replies", JSONArray())).isEmpty())
        for (broken in listOf(JSONObject().put("nope", true), JSONObject().put("replies", "x"), JSONObject().put("replies", JSONObject.NULL), JSONObject())) {
            try { TeamHeldAnswer.parse(broken); fail("$broken is not a list") } catch (failure: TeamMessageFailure) { assertEquals("INVALID_DATA", failure.code) }
        }
    }

    @Test fun aDecisionIsReadAsTheServiceCameToItAndAnAnswerThatSaysNothingOfTheTaskIsNone() {
        fun decision(vararg pairs: Pair<String, Any?>) = TeamHeldDecision.parse(JSONObject().also { value -> for ((key, item) in pairs) value.put(key, item ?: JSONObject.NULL) })
        assertEquals(TeamHeldDecision("task-0001", "completed", replyId = "reply-0001"), decision("taskId" to "task-0001", "state" to "completed", "replyId" to "reply-0001", "artifactId" to null))
        assertEquals("approval_rejected", decision("taskId" to "task-0001", "state" to "declined", "declineReason" to "approval_rejected").declineReason)
        assertEquals("revoked", decision("taskId" to "task-0001", "state" to "declined", "declineReason" to "revoked").declineReason)
        assertNull("a reason this version does not know is left out", decision("taskId" to "task-0001", "state" to "declined", "declineReason" to "a_new_reason").declineReason)
        assertEquals("artifact-1", decision("taskId" to "task-0001", "state" to "completed", "artifactId" to "artifact-1", "replyId" to "").artifactId)
        assertNull(decision("taskId" to "task-0001", "state" to "completed", "artifactId" to "artifact-1", "replyId" to "").replyId)
        for (nonsense in listOf(JSONObject(), JSONObject().put("taskId", "task-0001"), JSONObject().put("state", "completed"), JSONObject().put("taskId", "").put("state", "completed"), JSONObject().put("taskId", "task-0001").put("state", ""))) {
            try { TeamHeldDecision.parse(nonsense); fail("$nonsense is no decision") } catch (failure: TeamMessageFailure) { assertEquals("INVALID_DATA", failure.code) }
        }
    }

    // What may be sent

    @Test fun theTextTheMaintainerSendsIsTrimmedAtLeastACharacterAndEightKilobytes() {
        assertEquals("trimmed", TeamHeldEdit.Send("Dinner at 8."), TeamHeldRules.edit("Dinner at 7.", "  Dinner at 8.  "))
        assertEquals("nothing changed: the reply goes as the Bot made it, and is not recorded as changed", TeamHeldEdit.Send(null), TeamHeldRules.edit("Dinner at 7.", "Dinner at 7."))
        assertEquals("only spaces and lines at the ends changed", TeamHeldEdit.Send(null), TeamHeldRules.edit("Dinner at 7.\n", "\n  Dinner at 7.  "))
        for (edited in listOf("", "   ", "\n\t ")) assertEquals(edited, TeamHeldEdit.Empty, TeamHeldRules.edit("Dinner", edited))
        assertEquals("2731 euro signs are 8193 bytes", TeamHeldEdit.TooLong, TeamHeldRules.edit("x", "€".repeat(2731)))
        assertEquals("2730 are 8190", TeamHeldEdit.Send("€".repeat(2730)), TeamHeldRules.edit("x", "€".repeat(2730)))
        assertEquals("the spaces around are not counted", TeamHeldEdit.Send("x".repeat(8192)), TeamHeldRules.edit("x", "  " + "x".repeat(8192) + "  "))
        assertEquals(TeamHeldEdit.TooLong, TeamHeldRules.edit("x", "x".repeat(8193)))
        assertEquals(8192, TeamExecutionContract.HELD_REPLY_BYTES)
        // What cannot be sent says why; text that can says nothing.
        assertNull(TeamHeldRules.problemRes(TeamHeldEdit.Send("x"))); assertNull(TeamHeldRules.problemRes(TeamHeldEdit.Send(null)))
        assertEquals(R.string.teams_held_empty_text, TeamHeldRules.problemRes(TeamHeldEdit.Empty)); assertEquals(R.string.teams_held_too_long, TeamHeldRules.problemRes(TeamHeldEdit.TooLong))
    }

    @Test fun theCommandsAreTheMemberCommandsTheServiceTakes() {
        fun command(action: String, payload: JSONObject) = JSONObject().put("schemaVersion", 1).put("commandId", "command_0001").put("teamId", "team_0001").put("generation", 1).put("action", action).put("payload", payload)
        val bot = "sharedbot_01"; val task = "task-0001"; val chatId = "chat_0001"
        fun JSONObject.fields(): Map<String, Any?> = keys().asSequence().associateWith { get(it) }
        assertEquals("without a Chat, every Chat", mapOf("sharedBotId" to bot), TeamHeldRules.listPayload(bot).fields())
        assertEquals(mapOf("sharedBotId" to bot, "conversationId" to chatId), TeamHeldRules.listPayload(bot, chatId).fields())
        assertEquals("as the Bot made it: no text", mapOf("taskId" to task), TeamHeldRules.approvePayload(task).fields())
        assertEquals(mapOf("taskId" to task, "text" to "Dinner at 8."), TeamHeldRules.approvePayload(task, "Dinner at 8.").fields())
        assertEquals(mapOf("taskId" to task), TeamHeldRules.rejectPayload(task).fields())
        for (payload in listOf(TeamHeldRules.listPayload(bot), TeamHeldRules.listPayload(bot, chatId))) TeamExecutionContract.validate(command("listHeldReplies", payload))
        for (payload in listOf(TeamHeldRules.approvePayload(task), TeamHeldRules.approvePayload(task, "Dinner at 8."), TeamHeldRules.approvePayload(task, "€".repeat(2730)))) TeamExecutionContract.validate(command("approveReply", payload))
        TeamExecutionContract.validate(command("rejectReply", TeamHeldRules.rejectPayload(task)))
        // What the service would refuse is refused before it is sent.
        val refused = listOf(
            "listHeldReplies" to JSONObject(), "listHeldReplies" to JSONObject().put("conversationId", chatId), "listHeldReplies" to JSONObject().put("sharedBotId", "x"),
            "listHeldReplies" to JSONObject().put("sharedBotId", bot).put("conversationId", "x"), "listHeldReplies" to JSONObject().put("sharedBotId", bot).put("limit", 5),
            "approveReply" to JSONObject(), "approveReply" to JSONObject().put("taskId", "x"), "approveReply" to JSONObject().put("taskId", task).put("text", ""),
            "approveReply" to JSONObject().put("taskId", task).put("text", "  \n "), "approveReply" to TeamHeldRules.approvePayload(task, "€".repeat(2731)), "approveReply" to JSONObject().put("taskId", task).put("text", 5),
            "approveReply" to JSONObject().put("taskId", task).put("commandId", "command_0002"), "rejectReply" to JSONObject(), "rejectReply" to JSONObject().put("taskId", "x"),
            "rejectReply" to JSONObject().put("taskId", task).put("text", "no"), "holdReply" to JSONObject().put("taskId", task).put("text", "Dinner"),
        )
        for ((action, payload) in refused) {
            try { TeamExecutionContract.validate(command(action, payload)); fail("$action $payload") } catch (failure: TeamMessageFailure) { assertEquals("INVALID_INPUT", failure.code) }
        }
    }

    // Whose replies, and where they are shown

    @Test fun theBotsOfAChatWhoseRepliesAPersonDecidesOnAreTheOnesTheyMaintainThatAreInIt() {
        val cards = listOf(card("sharedbot_mine"), card("sharedbot_auto", daily = 5), card("sharedbot_theirs", maintainer = "owner"), card("sharedbot_older", generation = 0), card("sharedbot_elsewhere"))
        val participants = listOf("user:bob", "shared_bot:sharedbot_mine", "shared_bot:sharedbot_auto", "shared_bot:sharedbot_theirs", "shared_bot:sharedbot_older")
        fun ids(conversation: TeamMessageConversation, who: TeamMemberRef = viewer) = TeamHeldRules.bots(conversation, cards, who).map { it.sharedBotId }
        assertEquals("a Bot that holds replies need not answer on its own; one of someone else, of an older membership or outside the Chat is not theirs here", listOf("sharedbot_mine", "sharedbot_auto"), ids(chat(participants)))
        assertEquals("a person who maintains none has none", emptyList<String>(), ids(chat(participants), TeamMemberRef("bob", 1)))
        assertEquals("a Chat with no Bot in it has none", emptyList<String>(), ids(chat(listOf("user:bob"))))
        assertEquals("taking part in the Chat is not needed", listOf("sharedbot_mine"), ids(chat(listOf("user:bob", "shared_bot:sharedbot_mine"), kind = "bot_dm")))
    }

    @Test fun aReplySitsWithItsTaskOneNobodyAskedForIsARowOfItsOwnOldestFirstAndAnAskedOneWaitsForItsTask() {
        val rows = listOf(row(5, unasked = true), row(4), row(3), row(2, unasked = true), row(1, unasked = true))
        val placed = TeamHeldRules.place(rows, setOf("task-0004", "task-0001"))
        assertEquals(setOf("task-0004", "task-0001"), placed.byTask.keys)
        assertEquals("the stream reads oldest first; a reply whose task is not listed yet is not a row of its own unless nobody asked for it", listOf("task-0002", "task-0005"), placed.orphans.map { it.reply.taskId })
        val none = TeamHeldRules.place(emptyList(), emptySet())
        assertTrue(none.byTask.isEmpty()); assertTrue(none.orphans.isEmpty())
    }

    @Test fun theCardOfABotCountsTheRepliesThatStillWaitAndTheListGroupsThemByChat() {
        val ended = row(4).copy(ended = "REPLY_APPROVED")
        val rows = listOf(row(6, chat = "chat_0002", title = "Trip"), row(5), ended, row(2, chat = "chat_0003", title = null), row(1))
        val list = TeamHeldList(TeamHeldList.Status.Ready, rows, null, start + 3500 + 10 * minute)
        assertEquals("the ones whose time is up and the one that ended do not wait: only the two newest are still within their ten minutes", 2, TeamHeldRules.waiting(list))
        assertEquals("the ended one does not wait however much time is left", 4, TeamHeldRules.waiting(list.copy(now = start)))
        assertEquals(0, TeamHeldRules.waiting(TeamHeldList()))
        val chats = TeamHeldRules.byChat(list.rows)
        assertEquals("the Chat of the newest reply first", listOf("chat_0002", "chat_0001", "chat_0003"), chats.map { it.id })
        assertEquals("a Chat the service gave no title for has none: the screen names it", listOf("Trip", "Planning", null), chats.map { it.title })
        assertEquals("newest first within a Chat", listOf("task-0005", "task-0004", "task-0001"), chats[1].rows.map { it.reply.taskId })
        assertTrue(TeamHeldRules.byChat(emptyList()).isEmpty())
        assertEquals("the end of its time is the end of it, even before the service is asked again", "REPLY_EXPIRED", row(1).over(start + 1000 + 10 * minute))
        assertNull(row(1).over(start + 1000 + 10 * minute - 1))
        assertEquals("what the service said stands", "REPLY_APPROVED", ended.over(start))
    }

    @Test fun theRepliesOfSeveralBotsAreTaggedWithTheirBotNewestFirstAndABotThatIsNotTheirsAddsNothing() = runBlocking<Unit> {
        val replies = mapOf("bot-a" to listOf(reply(1), reply(3)), "bot-b" to listOf(reply(2)))
        val asked = mutableListOf<String>()
        val read = TeamHeldRules.read(listOf("bot-a", "bot-b", "bot-c")) { id ->
            asked += id
            if (id == "bot-c") throw TeamMessageFailure("MAINTAINER_REQUIRED")
            replies[id].orEmpty()
        }
        assertEquals(listOf("task-0003", "task-0002", "task-0001"), read.map { it.taskId }); assertEquals(listOf("bot-a", "bot-b", "bot-a"), read.map { it.sharedBotId })
        assertEquals(setOf("bot-a", "bot-b", "bot-c"), asked.toSet())
        for ((bots, code) in listOf(listOf("bot-c") to "MAINTAINER_REQUIRED", emptyList<String>() to "MAINTAINER_REQUIRED", listOf("bot-a", "bot-d") to "SERVICE_UNAVAILABLE")) {
            try {
                TeamHeldRules.read(bots) { id ->
                    if (id == "bot-d") throw TeamMessageFailure("SERVICE_UNAVAILABLE")
                    if (id == "bot-c") throw TeamMessageFailure("MAINTAINER_REQUIRED")
                    emptyList()
                }
                fail("$bots should not read")
            } catch (failure: TeamMessageFailure) { assertEquals("$bots: every Bot refusing is the person not being a maintainer, any other failure is the failure", code, failure.code) }
        }
        try {
            TeamHeldRules.read(listOf("bot-x", "bot-y")) { id -> throw TeamMessageFailure(if (id == "bot-x") "INVALID_DATA" else "SERVICE_UNAVAILABLE") }
            fail("should not read")
        } catch (failure: TeamMessageFailure) { assertEquals("of the Bots that failed, the first one's failure is told", "INVALID_DATA", failure.code) }
        assertEquals("an error that is not the service's is the service being unavailable", "SERVICE_UNAVAILABLE", TeamHeldRules.code(java.io.IOException("offline")))
    }

    // The list

    /** What a screen's board reads and decides, scripted by the test. */
    private class Service(var replies: List<TeamHeldReply>) {
        var failure: String? = null
        var reads = 0
        var approveOutcome: ((String) -> Result<TeamHeldDecision>)? = null
        var rejectOutcome: ((String) -> Result<TeamHeldDecision>)? = null
        /** `taskId|text|commandId` of every approval sent, `taskId|commandId` of every rejection. */
        val approvals = mutableListOf<String>()
        val rejections = mutableListOf<String>()
        fun read(): List<TeamHeldReply> { reads += 1; failure?.let { throw TeamMessageFailure(it) }; return replies }
        fun approve(taskId: String, text: String?, commandId: String): TeamHeldDecision {
            approvals += "$taskId|${text ?: "-"}|$commandId"
            val outcome = approveOutcome?.invoke(taskId) ?: Result.success(TeamHeldDecision(taskId, "completed", replyId = "reply-0001"))
            if (outcome.isSuccess) replies = replies.filter { it.taskId != taskId }
            return outcome.getOrThrow()
        }
        fun reject(taskId: String, commandId: String): TeamHeldDecision {
            rejections += "$taskId|$commandId"
            val outcome = rejectOutcome?.invoke(taskId) ?: Result.success(TeamHeldDecision(taskId, "declined", declineReason = "approval_rejected"))
            if (outcome.isSuccess) replies = replies.filter { it.taskId != taskId }
            return outcome.getOrThrow()
        }
    }
    /** The time the board under test keeps, which the test moves. */
    private class Clock(var value: Long = 1_800_000_000_000)
    private fun decideWith(service: Service) = TeamHeldDecide(approve = { id, text, command -> service.approve(id, text, command) }, reject = { id, command -> service.reject(id, command) })
    /** A board over the service; the screen going away is the sleep that throws. [lists] is every list it published. */
    private fun CoroutineScope.board(
        service: Service, clock: Clock = Clock(), online: () -> Boolean = { true }, sleep: suspend (Long) -> Unit = { throw CancellationException("the screen went away") },
    ): Pair<TeamHeldBoard, MutableList<TeamHeldList>> {
        val lists = mutableListOf<TeamHeldList>(); var ids = 0
        return TeamHeldBoard(this, read = { service.read() }, decide = decideWith(service), changed = { lists += it }, online = online, clock = { clock.value }, newId = { "command-${++ids}" }, sleep = sleep) to lists
    }
    /** Lets the other coroutines run until [done], and fails the test instead of waiting for ever. */
    private suspend fun spin(done: () -> Boolean) {
        repeat(20_000) { if (done()) return; yield() }
        fail("what the test waited for did not happen")
    }

    @Test fun theListIsReadWhenAskedNewestFirstAndAReadThatFailsKeepsWhatWasRead() = runBlocking<Unit> {
        val service = Service(listOf(reply(1), reply(5)))
        val (board, lists) = board(service)
        assertEquals(TeamHeldList.Status.Loading, board.list.status); assertTrue(board.list.rows.isEmpty())
        board.load()
        assertEquals(TeamHeldList.Status.Ready, board.list.status); assertEquals(listOf("task-0005", "task-0001"), board.list.rows.map { it.reply.taskId }); assertEquals(1, lists.size)
        assertTrue(board.list.rows.all { it.busy == null && it.error == null && it.ended == null })
        service.failure = "SERVICE_UNAVAILABLE"
        board.load()
        assertEquals(TeamHeldList.Status.Failed, board.list.status); assertEquals("SERVICE_UNAVAILABLE", board.list.error); assertEquals("what was read stays", 2, board.list.rows.size)
        service.failure = null
        board.load()
        assertEquals(TeamHeldList.Status.Ready, board.list.status); assertNull(board.list.error)
        service.failure = "MAINTAINER_REQUIRED"
        board.load()
        assertEquals(TeamHeldList.Status.Denied, board.list.status); assertTrue("the service says none of it is theirs: nothing of it stays", board.list.rows.isEmpty())
    }

    @Test fun nothingIsReadWithoutAConnectionAndTheListIsReadOnceItReturns() = runBlocking<Unit> {
        val service = Service(listOf(reply(1)))
        var connected = false
        val (board, lists) = board(service, online = { connected })
        board.load()
        assertEquals("no connection: no read, and nothing is said of one", 0, service.reads); assertEquals(TeamHeldList.Status.Loading, board.list.status); assertTrue(lists.isEmpty())
        connected = true
        board.load()
        assertEquals(1, service.reads); assertEquals(TeamHeldList.Status.Ready, board.list.status); assertEquals(1, board.list.rows.size)
    }

    @Test fun readsDoNotPileUpOneIsMadeAtATimeAndRequestsThatComeMeanwhileAreAnsweredByOneMore() = runBlocking<Unit> {
        val service = Service(listOf(reply(1)))
        val gates = List(2) { CompletableDeferred<Unit>() }
        var arrivals = 0
        val pending = TeamHeldBoard(this, read = { val mine = arrivals++; if (mine < 2) gates[mine].await(); service.read() }, decide = decideWith(service), changed = {})
        val first = launch { pending.load() }
        spin { arrivals >= 1 }
        // The requests that come while the read is on its way.
        val second = launch { pending.load() }; val third = launch { pending.load() }
        repeat(10) { yield() }
        assertEquals("one read is on its way", 1, arrivals)
        gates[0].complete(Unit)
        spin { arrivals >= 2 }
        gates[1].complete(Unit)
        first.join(); second.join(); third.join()
        assertEquals("and one more followed it, for all the requests that came meanwhile", 2, arrivals)
        assertEquals(TeamHeldList.Status.Ready, pending.list.status)
        // A read that answers after the board was closed changes nothing, and a closed board reads no more.
        val stop = CompletableDeferred<Unit>(); val seen = mutableListOf<TeamHeldList>(); var reads = 0
        val one = reply(1)
        val closing = TeamHeldBoard(this, read = { reads++; stop.await(); listOf(one) }, decide = decideWith(service), changed = { seen += it })
        val late = launch { closing.load() }
        spin { reads >= 1 }
        closing.close(); stop.complete(Unit); late.join()
        assertEquals("nothing was shown after it was closed", 0, seen.size); assertEquals(TeamHeldList.Status.Loading, closing.list.status)
        closing.load()
        assertEquals(1, reads)
    }

    @Test fun approvingSendsTheDecisionOnceARetryOfTheSameDecisionIsTheSameCommandAndWhatTheServiceTookLeavesTheList() = runBlocking<Unit> {
        val service = Service(listOf(reply(1), reply(2)))
        val (board, _) = board(service)
        board.load()
        service.approveOutcome = { Result.failure(TeamMessageFailure("SERVICE_UNAVAILABLE")) }
        assertFalse(board.approve("task-0001"))
        assertEquals("SERVICE_UNAVAILABLE", board.list.row("task-0001")?.error); assertNull("it can be tried again", board.list.row("task-0001")?.busy)
        assertFalse(board.approve("task-0001"))
        assertFalse(board.approve("task-0001", "Dinner at 8."))
        assertFalse(board.approve("task-0001", "Dinner at 8."))
        service.approveOutcome = null
        assertTrue(board.approve("task-0001", "Dinner at 8."))
        assertEquals("a retry of the same decision is the same command, a different decision is a new one",
            listOf("task-0001|-|command-1", "task-0001|-|command-1", "task-0001|Dinner at 8.|command-2", "task-0001|Dinner at 8.|command-2", "task-0001|Dinner at 8.|command-2"), service.approvals)
        assertEquals("taken: it is gone", listOf("task-0002"), board.list.rows.map { it.reply.taskId })
        assertFalse("a reply that is not on the list is not decided", board.approve("task-0001"))
        assertEquals(5, service.approvals.size)
        // A decision that is on its way is not sent again, whatever is pressed.
        val slow = CompletableDeferred<Unit>(); var arrivals = 0
        val blocked = TeamHeldBoard(this, read = { service.read() }, decide = TeamHeldDecide(approve = { id, _, _ -> arrivals++; slow.await(); TeamHeldDecision(id, "completed") }, reject = { id, command -> service.reject(id, command) }), changed = {})
        blocked.load()
        val first = async { blocked.approve("task-0002") }
        spin { arrivals >= 1 }
        assertEquals(TeamHeldRow.Busy.Approve, blocked.list.row("task-0002")?.busy)
        assertFalse(blocked.approve("task-0002")); assertFalse(blocked.reject("task-0002")); assertEquals(1, arrivals); assertTrue(service.rejections.isEmpty())
        slow.complete(Unit)
        assertTrue(first.await())
    }

    @Test fun aRejectIsOneCommandTooAndDropsTheReply() = runBlocking<Unit> {
        val service = Service(listOf(reply(1)))
        val (board, _) = board(service)
        board.load()
        service.rejectOutcome = { Result.failure(TeamMessageFailure("SERVICE_UNAVAILABLE")) }
        assertFalse(board.reject("task-0001")); assertFalse(board.reject("task-0001"))
        service.rejectOutcome = null
        assertTrue(board.reject("task-0001"))
        assertEquals(listOf("task-0001|command-1", "task-0001|command-1", "task-0001|command-1"), service.rejections)
        assertTrue(board.list.rows.isEmpty()); assertTrue(service.approvals.isEmpty())
    }

    @Test fun aReplyTheServiceSaysIsNoLongerThereStaysOnScreenSayingSoUntilItIsClosedAndAnyOtherFailureLeavesItAsItWas() = runBlocking<Unit> {
        for (code in listOf("REPLY_EXPIRED", "REPLY_SUPERSEDED", "REPLY_APPROVED", "REPLY_REJECTED", "REPLY_NOT_HELD", "TASK_REQUIRED", "MAINTAINER_REQUIRED")) {
            val service = Service(listOf(reply(1)))
            val (board, _) = board(service)
            board.load()
            service.approveOutcome = { Result.failure(TeamMessageFailure(code)) }; service.replies = emptyList()
            assertFalse(code, board.approve("task-0001"))
            repeat(5) { yield() }
            val row = checkNotNull(board.list.row("task-0001")) { code }
            assertEquals("it says what became of it, and the list that no longer has it does not take it away", code, row.ended); assertNull(row.busy); assertNull(row.error)
            assertFalse("nothing more is sent for it", board.reject("task-0001")); assertEquals(1, service.approvals.size); assertTrue(service.rejections.isEmpty())
            board.dismiss("task-0001")
            assertTrue("$code: read, and gone", board.list.rows.isEmpty())
        }
        val service = Service(listOf(reply(1)))
        val (board, _) = board(service)
        board.load()
        board.dismiss("task-0001")
        assertEquals("a reply that is still to be decided is not dismissed", 1, board.list.rows.size)
        service.approveOutcome = { Result.failure(TeamMessageFailure("INVALID_INPUT")) }
        board.approve("task-0001")
        assertEquals("INVALID_INPUT", board.list.row("task-0001")?.error); assertNull(board.list.row("task-0001")?.ended)
    }

    @Test fun anApprovalTheServiceTookThatPostedNothingSaysSo() = runBlocking<Unit> {
        val service = Service(listOf(reply(1)))
        val (board, _) = board(service)
        board.load()
        service.approveOutcome = { id -> Result.success(TeamHeldDecision(id, "declined", declineReason = "revoked")) }
        assertTrue("the decision was taken", board.approve("task-0001"))
        repeat(5) { yield() }
        assertEquals(TeamHeldRules.NOT_POSTED, board.list.row("task-0001")?.ended)
        board.dismiss("task-0001")
        assertTrue(board.list.rows.isEmpty())
    }

    @Test fun aReplyThatIsNoLongerListedGoesQuietlyUnlessItIsBeingDecidedOrItsTimeRanOutWhileItWasShown() = runBlocking<Unit> {
        val clock = Clock(start + 1000)
        val service = Service(listOf(reply(1, extra = mapOf("expiresAt" to start + 1000 + 5 * minute)), reply(2, extra = mapOf("expiresAt" to start + 1000 + minute)), reply(3, extra = mapOf("expiresAt" to start + 1000 + 8 * minute))))
        val slow = CompletableDeferred<Unit>(); var arrivals = 0
        val board = TeamHeldBoard(this, read = { service.read() }, decide = TeamHeldDecide(approve = { id, _, _ -> arrivals++; slow.await(); TeamHeldDecision(id, "completed") }, reject = { id, command -> service.reject(id, command) }),
            changed = {}, clock = { clock.value })
        board.load()
        val deciding = async { board.approve("task-0003") }
        spin { arrivals >= 1 }
        clock.value += 2 * minute; service.replies = emptyList()
        board.load()
        assertNull("dropped for a newer message, or decided on another screen: it simply goes", board.list.row("task-0001"))
        assertEquals("its time ran out while it was shown: it says so", "REPLY_EXPIRED", board.list.row("task-0002")?.ended)
        assertEquals("a decision that is on its way keeps its reply on screen", TeamHeldRow.Busy.Approve, board.list.row("task-0003")?.busy)
        slow.complete(Unit); deciding.await()
        repeat(5) { yield() }
        assertEquals(listOf("task-0002"), board.list.rows.map { it.reply.taskId })
    }

    // The time

    @Test fun theTimeMovesOnEvery30SecondsAndTheListIsReadAgainWhenTheFirstReplyRunsOut() = runBlocking<Unit> {
        val clock = Clock(start)
        val service = Service(listOf(reply(1, extra = mapOf("expiresAt" to start + 100_000)), reply(2, extra = mapOf("expiresAt" to start + 400_000))))
        val waits = mutableListOf<Long>()
        // Sleeping is the clock moving on; the sixth time the screen goes away.
        val (board, _) = board(service, clock, sleep = { ms ->
            waits += ms
            if (waits.size > 5) throw CancellationException("the screen went away")
            clock.value += ms
        })
        board.load()
        assertEquals(30_000L, TeamHeldRules.TICK_MS); assertEquals(start, board.list.now)
        service.replies = service.replies.filter { it.taskId != "task-0001" }
        try { board.keepTime() } catch (_: CancellationException) { }
        assertEquals("every 30 s, and once sooner: the first reply runs out 10 s after the third tick, and is read 0.5 s later", listOf(30_000L, 30_000L, 30_000L, 10_500L, 30_000L, 30_000L), waits)
        assertEquals("a tick reads nothing; the end of a reply is read for once", 2, service.reads)
        assertEquals("REPLY_EXPIRED", board.list.row("task-0001")?.ended); assertNull(board.list.row("task-0002")?.ended)
        assertEquals("the time is the time of the last wake", start + 130_500, board.list.now)
        // A closed board keeps no time.
        board.close()
        val after = waits.size
        board.keepTime()
        assertEquals(after, waits.size)
    }

    @Test fun theTimeIsKeptEvenWhenNothingWaitsAndWhenTheServiceStillListsAReplyThatRanOutHere() = runBlocking<Unit> {
        val clock = Clock(start)
        val service = Service(listOf(reply(1, extra = mapOf("expiresAt" to start + 5000))))
        val waits = mutableListOf<Long>()
        val (board, _) = board(service, clock, sleep = { ms ->
            waits += ms
            if (waits.size > 3) throw CancellationException("the screen went away")
            clock.value += ms
        })
        board.load()
        try { board.keepTime() } catch (_: CancellationException) { }
        // The service's clock is behind this one: it still lists the reply. The card is over by this clock, and the list is not read in a loop for it.
        assertEquals(listOf(5_500L, 30_000L, 30_000L, 30_000L), waits); assertEquals(2, service.reads)
        assertEquals("REPLY_EXPIRED", board.list.row("task-0001")?.over(board.list.now))
        assertEquals(0, TeamHeldRules.waiting(board.list))
        val empty = Service(emptyList())
        val waited = mutableListOf<Long>()
        val (quiet, _) = board(empty, clock, sleep = { ms -> waited += ms; if (waited.size > 1) throw CancellationException("the screen went away") })
        try { quiet.keepTime() } catch (_: CancellationException) { }
        assertEquals(listOf(30_000L, 30_000L), waited); assertEquals("with nothing to wait for, the time is kept and nothing is read", 0, empty.reads)
    }

    // What a screen keeps

    @Test fun aScreenKeepsTheBoardsListAsStateAndAsksForTheChatsTasksOnceTheServiceTookADecision() = runBlocking<Unit> {
        val service = Service(listOf(reply(1), reply(2)))
        var asked = 0
        val keeper = TeamHeldKeeper(this, decided = { asked += 1 }) { changed -> TeamHeldBoard(this, read = { service.read() }, decide = decideWith(service), changed = changed) }
        assertEquals("the list is loading until the first answer", TeamHeldList.Status.Loading, keeper.list.status)
        keeper.load()
        assertEquals(listOf("task-0002", "task-0001"), keeper.list.rows.map { it.reply.taskId }); assertEquals(TeamHeldList.Status.Ready, keeper.list.status)
        service.approveOutcome = { Result.failure(TeamMessageFailure("SERVICE_UNAVAILABLE")) }
        keeper.approve("task-0001")
        spin { keeper.list.row("task-0001")?.error != null }
        assertEquals("a decision the service refused fetches nothing", 0, asked)
        service.approveOutcome = null
        keeper.approve("task-0001")
        spin { asked == 1 }
        keeper.reject("task-0002")
        spin { asked == 2 }
        repeat(5) { yield() }
        assertTrue(keeper.list.rows.isEmpty())
        keeper.close()
        // A screen whose person maintains no Bot reads nothing and keeps an empty list that is ready.
        val idle = TeamHeldKeeper(this, decided = { asked += 1 }) { null }
        idle.load(); idle.approve("task-0001"); idle.reject("task-0001"); idle.dismiss("task-0001"); idle.reload()
        assertEquals(TeamHeldList.Status.Ready, idle.list.status); assertTrue(idle.list.rows.isEmpty()); assertEquals(2, asked); assertEquals(1, service.rejections.size)
    }

    // The words

    @Test fun everyWayAReplyCanBeRefusedHasWordsAndTheRestIsWhatEveryMessageCommandSays() {
        val table = listOf(
            listOf("REPLY_APPROVED", "REPLY_REJECTED", "REPLY_NOT_HELD") to R.string.teams_error_reply_decided, listOf("REPLY_EXPIRED") to R.string.teams_error_reply_expired,
            listOf("REPLY_SUPERSEDED") to R.string.teams_error_reply_superseded, listOf("TASK_REQUIRED") to R.string.teams_error_reply_gone,
            listOf("MAINTAINER_REQUIRED") to R.string.teams_error_held_maintainer, listOf(TeamHeldRules.NOT_POSTED) to R.string.teams_held_revoked,
        )
        for ((codes, words) in table) for (code in codes) assertEquals(code, words, TeamHeldRules.errorRes(code))
        for (code in TeamHeldRules.ended) assertNotEquals("a reply that is gone says why: $code", R.string.teams_error, TeamHeldRules.errorRes(code))
        for (code in listOf("SERVICE_UNAVAILABLE", "INVALID_INPUT", "MEMBERSHIP_REQUIRED", "INVALID_DATA", "SOMETHING_ELSE")) assertEquals(code, teamsErrorRes(code), TeamHeldRules.errorRes(code))
        assertNotEquals("the words of a reply, not those of an operation", teamsErrorRes("MAINTAINER_REQUIRED"), TeamHeldRules.errorRes("MAINTAINER_REQUIRED"))
        assertEquals(setOf("REPLY_APPROVED", "REPLY_REJECTED", "REPLY_NOT_HELD", "REPLY_EXPIRED", "REPLY_SUPERSEDED", "TASK_REQUIRED", "MAINTAINER_REQUIRED"), TeamHeldRules.ended)
    }

    @Test fun theWordsOfThisFeatureAreInEveryLanguageEachDifferentFromTheOthersWithABotNeverCalledAnAssistant() {
        val texts = listOf("values", "values-zh-rTW", "values-en", "values-ja", "values-ko").associateWith { directory ->
            val file = listOf("src/main/res/$directory/strings.xml", "app/src/main/res/$directory/strings.xml", "apps/android/native/app/src/main/res/$directory/strings.xml").map(::File).first { it.exists() }
            // Plain text on purpose: the Android XML classes on the unit-test class path are stubs.
            Regex("<string\\s+name=\"([^\"]+)\"[^>]*>(.+?)</string>", RegexOption.DOT_MATCHES_ALL).findAll(file.readText()).associate { it.groupValues[1] to it.groupValues[2] }
        }
        for (name in TeamHeldRules.stringNames) {
            for ((directory, names) in texts) assertFalse("$directory has no text for $name", names[name].isNullOrBlank())
            if (name.startsWith("teams_held_") || name.startsWith("teams_error_reply_") || name == "teams_error_held_maintainer") {
                assertEquals("$name differs in every language", 5, texts.values.map { it.getValue(name) }.toSet().size)
            }
        }
        for ((directory, names) in texts) {
            for (name in listOf("teams_held_expires_fmt", "teams_held_document_fmt")) assertTrue("$directory $name says where its figure goes", names.getValue(name).contains("%1\$s"))
            // The words are for the Bot's maintainer: a Bot is never called an assistant.
            for (name in TeamHeldRules.stringNames) assertFalse("$directory $name", Regex("assistant|助手|アシスタント|어시스턴트", RegexOption.IGNORE_CASE).containsMatchIn(names.getValue(name)))
        }
        assertEquals("待核准的回覆", texts.getValue("values-zh-rTW").getValue("teams_held_waiting")); assertEquals("Replies waiting", texts.getValue("values-en").getValue("teams_held_waiting"))
        assertEquals("Dropped %1\$s if not approved", texts.getValue("values-en").getValue("teams_held_expires_fmt")); assertEquals("Includes the document “%1\$s”", texts.getValue("values-en").getValue("teams_held_document_fmt"))
        assertEquals("the word of a decision a person is already used to", "核准", texts.getValue("values-zh-rTW").getValue("teams_approval_approve"))
        assertTrue(texts.getValue("values-ja").getValue("teams_error_held_maintainer").contains("メンテナー")); assertTrue(texts.getValue("values-ko").getValue("teams_error_held_maintainer").contains("메인테이너"))
    }
}
