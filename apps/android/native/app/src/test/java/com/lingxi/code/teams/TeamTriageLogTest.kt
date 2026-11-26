package com.lingxi.code.teams

import com.lingxi.code.R
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.yield
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.File

/** The auto-reply log of a Bot for the person who maintains it: the rules, the decoding, the paged list, and why a Bot stayed silent. No UI, no network. */
class TeamTriageLogTest {
    // The service's rows, as JSON, and the page they come in.
    private fun row(n: Int, state: String = "declined", action: String = "decline", title: String? = "Planning", probability: Double? = 0.12, reason: String? = "below_threshold"): JSONObject {
        val verdict = JSONObject().put("action", action).put("source", "decision").put("at", 1_700_000_000_000L + n * 1000L)
        if (reason != null) verdict.put("reason", reason)
        if (probability != null) verdict.put("probability", probability)
        val row = JSONObject().put("taskId", "task-%04d".format(n)).put("conversationId", "chat_0001").put("messageId", "message-$n").put("createdAt", 1_700_000_000_000L + n)
            .put("triage", verdict).put("state", state)
        if (title != null) row.put("conversationTitle", title)
        return row
    }
    private fun item(n: Int) = checkNotNull(TeamTriageItem.parse(row(n)))
    private fun page(numbers: List<Int>, next: String? = null) = TeamTriagePage(numbers.map(::item), next)

    private fun card(id: String, maintainer: String = "owner", generation: Long = 3, autoReply: Boolean = true): TeamBotCard {
        val auto = if (autoReply) ",\"autoReply\":{\"daily\":20}" else ""
        return TeamBotCard.parse(JSONObject(
            """{"sharedBotId":"$id","name":"Helper $id","description":"","maintainer":{"uid":"$maintainer","generation":$generation},"revision":2,"state":"published","permitted":true,"ready":true,"onlineUntil":0,"usePolicy":{"mode":"all"},"quotas":{"team":1,"member":1,"bot":1}$auto}"""))
    }
    private fun chat(kind: String, participants: List<String>, on: List<String>? = null) = TeamMessageConversation(
        id = "chat_0001", kind = kind, title = "Planning", participants = participants, lastSeq = 0, lastChangeSeq = 0, lastReadSeq = 0,
        sharedBotIds = participants.filter { it.startsWith("shared_bot:") }.map { it.removePrefix("shared_bot:") }, autoReply = on?.let { TeamConversationAutoReply(1, it) },
    )
    private fun task(id: String, state: String, bot: String = "sharedbot_01") = TeamTask.parse(JSONObject(
        """{"taskId":"$id","conversationId":"chat_0001","sharedBotId":"$bot","requester":{"uid":"member","generation":1},"state":"$state","createdAt":1,"deadline":2}"""))
    private val owner = TeamMemberRef("owner", 3)

    // Where a message ended up

    @Test fun everyStateOfATaskIsOneOfFiveOutcomes() {
        val expected = mapOf(
            "completed" to TeamTriageOutcome.Replied, "declined" to TeamTriageOutcome.Silent, "cancelled" to TeamTriageOutcome.Silent,
            "queued" to TeamTriageOutcome.Waiting, "running" to TeamTriageOutcome.Waiting, "waiting" to TeamTriageOutcome.Waiting, "awaiting_approval" to TeamTriageOutcome.Approval,
            "failed" to TeamTriageOutcome.Failed, "expired" to TeamTriageOutcome.Failed, "interrupted" to TeamTriageOutcome.Failed,
            "authorization_required" to TeamTriageOutcome.Failed, "revoked" to TeamTriageOutcome.Failed, "needs_reconciliation" to TeamTriageOutcome.Failed,
        )
        assertEquals(expected, TeamTriageRules.outcomes)
        // Every outcome has its word; a decision this version does not know has none.
        TeamTriageOutcome.values().forEach { assertNotNull(TeamTriageRules.outcomeLabel(it)) }
        assertEquals(R.string.teams_triage_action_run, TeamTriageRules.actionLabel("run"))
        assertEquals(R.string.teams_triage_action_maybe, TeamTriageRules.actionLabel("run_may_decline"))
        assertNull(TeamTriageRules.actionLabel("explode"))
        assertNull(TeamTriageRules.reasonLabel("a_reason_from_the_future"))
        assertNull(TeamTriageRules.reasonLabel(null))
        TeamTriageRules.reasons.forEach { assertNotNull(it, TeamTriageRules.reasonLabel(it)) }
    }

    @Test fun aProbabilityIsShownAsAWholePercentAndHalvesGoUp() {
        assertEquals(12, TeamTriageRules.percent(0.12))
        assertEquals(0, TeamTriageRules.percent(0.0))
        assertEquals(100, TeamTriageRules.percent(1.0))
        assertEquals(13, TeamTriageRules.percent(0.125))
        assertEquals(1, TeamTriageRules.percent(0.005))
    }

    // Reading the service's answer

    @Test fun aRowIsReadAsTheServiceWroteIt() {
        val full = item(7)
        assertEquals(TeamTriageItem("task-0007", "chat_0001", "message-7", 1_700_000_000_007L,
            TeamTriageVerdict("decline", "decision", 1_700_000_007_000L, "below_threshold", 0.12), "declined", "Planning"), full)
        // A Bot's own decision has no probability, and a Chat may have no title.
        val bare = TeamTriageItem.parse(row(8, state = "completed", action = "run", title = null, probability = null, reason = "addressed"))
        assertEquals(TeamTriageVerdict("run", "decision", 1_700_000_008_000L, "addressed", null), bare?.triage)
        assertNull(bare?.conversationTitle)
        // An empty title is no title.
        assertNull(TeamTriageItem.parse(row(9, title = ""))?.conversationTitle)
        // A reason or a probability this version does not know is left out of the row; the decision is kept.
        val future = TeamTriageItem.parse(row(10, reason = "a_reason_from_the_future", probability = 1.5))
        assertNotNull(future); assertNull(future?.triage?.reason); assertNull(future?.triage?.probability)
        assertEquals("decline", future?.triage?.action)
        assertNull(TeamTriageItem.parse(row(11, probability = -0.1))?.triage?.probability)
        assertEquals(0.0, TeamTriageItem.parse(row(12, probability = 0.0))?.triage?.probability)
        assertEquals(1.0, TeamTriageItem.parse(row(13, probability = 1.0))?.triage?.probability)
    }

    @Test fun aHeldReplySaysHowItCameOutAndACodeThisVersionDoesNotKnowIsLeftOut() {
        for (approval in listOf("held", "approved", "rejected", "timeout", "superseded")) {
            assertEquals(approval, TeamTriageItem.parse(row(3, state = "awaiting_approval").put("approval", approval))?.approval)
        }
        assertNull("a reply that never was held says nothing", item(4).approval)
        assertEquals("a row whose reply waits is a row this version reads", "awaiting_approval", TeamTriageItem.parse(row(4, state = "awaiting_approval"))?.state)
        for (approval in listOf<Any>("a_new_outcome", "", 7, JSONObject.NULL)) {
            val parsed = TeamTriageItem.parse(row(5).put("approval", approval))
            assertNotNull("$approval: the row is still read", parsed); assertNull("$approval", parsed?.approval)
        }
        // The words: a reply that still waits says so as its outcome, so `held` has none of its own; the others say how it was decided.
        assertEquals(R.string.teams_task_waiting_approval, TeamTriageRules.outcomeLabel(TeamTriageOutcome.Approval))
        assertEquals(listOf(R.string.teams_approval_reply_approved, R.string.teams_approval_reply_declined, R.string.teams_triage_approval_timeout, R.string.teams_triage_approval_superseded),
            listOf("approved", "rejected", "timeout", "superseded").map { TeamTriageRules.approvalLabel(it) })
        for (none in listOf(null, "held", "a_new_outcome")) assertNull("$none", TeamTriageRules.approvalLabel(none))
        // Every code the service sends is one this version lists.
        assertEquals(setOf("held", "approved", "rejected", "timeout", "superseded"), TeamTriageRules.approvals)
    }

    @Test fun aRowThisVersionCannotReadIsNotOne() {
        assertNull(TeamTriageItem.parse(null))
        assertNull("an action it does not know", TeamTriageItem.parse(row(1, action = "explode")))
        assertNull("a state it does not know", TeamTriageItem.parse(row(1, state = "teleported")))
        assertNull("a source it does not know", TeamTriageItem.parse(row(1).also { it.getJSONObject("triage").put("source", "oracle") }))
        assertNull("no verdict", TeamTriageItem.parse(row(1).also { it.remove("triage") }))
        assertNull("no task", TeamTriageItem.parse(row(1).also { it.remove("taskId") }))
        assertNull("an empty task", TeamTriageItem.parse(row(1).also { it.put("taskId", "") }))
        assertNull("an empty chat", TeamTriageItem.parse(row(1).also { it.put("conversationId", "") }))
        assertNull("an empty message", TeamTriageItem.parse(row(1).also { it.put("messageId", "") }))
        assertNull("a time as text", TeamTriageItem.parse(row(1).also { it.put("createdAt", "1700000000000") }))
        assertNull("a verdict time as text", TeamTriageItem.parse(row(1).also { it.getJSONObject("triage").put("at", "1700000000000") }))
        assertNull("a time no date can hold", TeamTriageItem.parse(row(1).also { it.put("createdAt", 9.0e15) }))
        assertNull("a time no date can hold, before", TeamTriageItem.parse(row(1).also { it.put("createdAt", -9.0e15) }))
        assertNotNull("the last time a date can hold", TeamTriageItem.parse(row(1).also { it.put("createdAt", 8.64e15) }))
        assertNull("a state that is not text", TeamTriageItem.parse(row(1).also { it.put("state", 3) }))
        assertNull("a verdict that is not an object", TeamTriageItem.parse(row(1).also { it.put("triage", "decline") }))
    }

    @Test fun aPageKeepsWhatItCanReadAndAnAnswerThatIsNoPageIsInvalidData() {
        val answer = JSONObject().put("items", org.json.JSONArray().put(row(3)).put(row(2, action = "explode")).put("not a row").put(row(1))).put("nextCursor", "task-0001")
        val read = TeamTriagePage.parse(answer)
        assertEquals(listOf("task-0003", "task-0001"), read.items.map { it.taskId })
        assertEquals("task-0001", read.nextCursor)
        // The end of the log: no cursor, a null one or an empty one.
        assertNull(TeamTriagePage.parse(JSONObject().put("items", org.json.JSONArray())).nextCursor)
        assertNull(TeamTriagePage.parse(JSONObject().put("items", org.json.JSONArray()).put("nextCursor", JSONObject.NULL)).nextCursor)
        assertNull(TeamTriagePage.parse(JSONObject().put("items", org.json.JSONArray()).put("nextCursor", "")).nextCursor)
        for (broken in listOf(JSONObject(), JSONObject().put("items", "none"), JSONObject().put("items", org.json.JSONArray()).put("nextCursor", 7))) {
            try { TeamTriagePage.parse(broken); fail("$broken is not a page") } catch (failure: TeamMessageFailure) { assertEquals("INVALID_DATA", failure.code) }
        }
    }

    @Test fun everyVerdictReasonAndOutcomeHasWordsInEveryLanguage() {
        for (directory in listOf("values", "values-zh-rTW", "values-en", "values-ja", "values-ko")) {
            val file = listOf("src/main/res/$directory/strings.xml", "app/src/main/res/$directory/strings.xml", "apps/android/native/app/src/main/res/$directory/strings.xml").map(::File).first { it.exists() }
            // Plain text on purpose: the Android XML classes on the unit-test class path are stubs.
            val names = Regex("<string\\s+name=\"([^\"]+)\"[^>]*>(.+?)</string>", RegexOption.DOT_MATCHES_ALL).findAll(file.readText()).associate { it.groupValues[1] to it.groupValues[2] }
            for (name in TeamTriageRules.stringNames) assertFalse("$directory has no text for $name", names[name].isNullOrBlank())
            assertTrue("$directory: the probability keeps its number", names.getValue("teams_triage_probability_fmt").contains("%1\$d"))
            // The words are for the Bot's maintainer: a Bot is never called an assistant.
            for (name in TeamTriageRules.stringNames.filter { it.startsWith("teams_triage_") }) assertFalse("$directory $name", names.getValue(name).contains("assistant", ignoreCase = true))
        }
    }

    // Whose log

    @Test fun theBotsOfAChatWhoseLogAPersonReadsAreTheOnesTheyMaintainThatAnswerOnTheirOwn() {
        val mine = card("sharedbot_01"); val othersBot = card("sharedbot_02", maintainer = "someone"); val quiet = card("sharedbot_03", autoReply = false)
        val handedOver = card("sharedbot_04", generation = 2)
        val participants = listOf("user:member", "shared_bot:sharedbot_01", "shared_bot:sharedbot_02", "shared_bot:sharedbot_03", "shared_bot:sharedbot_04")
        val cards = listOf(mine, othersBot, quiet, handedOver)
        assertEquals(listOf(mine), TeamTriageRules.bots(chat("group", participants), cards, owner))
        // A Bot that is not published to answer on its own is still read while the Chat has it switched on.
        assertEquals(listOf(mine, quiet), TeamTriageRules.bots(chat("group", participants, on = listOf("sharedbot_03")), cards, owner))
        // A maintainer need not take part in a Chat to read what their Bot decided in it.
        assertEquals(listOf(mine), TeamTriageRules.bots(chat("group", listOf("user:member", "shared_bot:sharedbot_01")), cards, owner))
        // Not a Bot of the Chat, no log; and nobody else maintains these Bots.
        assertTrue(TeamTriageRules.bots(chat("group", listOf("user:member", "shared_bot:sharedbot_02")), cards, owner).isEmpty())
        assertTrue(TeamTriageRules.bots(chat("group", participants), cards, TeamMemberRef("member", 3)).isEmpty())
        // The membership that is held now decides: the same person after leaving and coming back is another.
        assertTrue(TeamTriageRules.bots(chat("group", participants), cards, TeamMemberRef("owner", 4)).isEmpty())
        assertTrue(TeamTriageRules.maintains(mine, owner)); assertFalse(TeamTriageRules.maintains(mine, TeamMemberRef("owner", 2)))
    }

    @Test fun onlyTheMaintainerOfABotDmIsToldWhyItStayedSilent() {
        val dm = chat("bot_dm", listOf("user:member", "shared_bot:sharedbot_01"))
        val cards = listOf(card("sharedbot_01"))
        assertEquals("sharedbot_01", TeamTriageRules.reasonBot(dm, cards, owner)?.sharedBotId)
        assertNull("somebody else", TeamTriageRules.reasonBot(dm, cards, TeamMemberRef("member", 1)))
        assertNull("a Chat with several people is not a DM", TeamTriageRules.reasonBot(chat("group", listOf("user:member", "shared_bot:sharedbot_01")), cards, owner))
        assertNull("a DM whose Bot is not known", TeamTriageRules.reasonBot(dm, emptyList(), owner))
        assertNull("a DM with no Bot", TeamTriageRules.reasonBot(chat("bot_dm", listOf("user:member")), cards, owner))
        // Only the tasks that stayed silent are asked about, and only the Bot's own.
        val tasks = listOf(task("task-0001", "declined"), task("task-0002", "completed"), task("task-0003", "declined", bot = "sharedbot_02"), task("task-0004", "declined"))
        assertEquals(listOf("task-0001", "task-0004"), TeamTriageRules.silentTasks(tasks, "sharedbot_01"))
        assertTrue(TeamTriageRules.silentTasks(emptyList(), "sharedbot_01").isEmpty())
    }

    @Test fun theLineUnderDidNotReplyHasTheReasonAndHowLikelyAReplyWasThought() {
        assertEquals(null to null, TeamTriageRules.reasonParts(null))
        assertEquals(R.string.teams_triage_reason_below_threshold to 12, TeamTriageRules.reasonParts(item(1)))
        assertEquals(R.string.teams_triage_reason_addressed to null, TeamTriageRules.reasonParts(checkNotNull(TeamTriageItem.parse(row(2, reason = "addressed", probability = null)))))
        assertEquals(null to 12, TeamTriageRules.reasonParts(checkNotNull(TeamTriageItem.parse(row(3, reason = "a_reason_from_the_future")))))
        assertEquals(null to null, TeamTriageRules.reasonParts(checkNotNull(TeamTriageItem.parse(row(4, reason = null, probability = null)))))
    }

    // The paged list

    @Test fun theListReadsAPageAtATimeUntilTheLogEnds() = runBlocking {
        val all = (45 downTo 1).map(::item)
        val statuses = mutableListOf<TeamTriageList.Status>(); val cursors = mutableListOf<String?>()
        val reader = TeamTriageReader(read = { cursor, limit ->
            cursors += cursor
            val from = cursor?.let { id -> all.indexOfFirst { it.taskId == id } + 1 } ?: 0
            val rows = all.subList(from, minOf(all.size, from + limit))
            TeamTriagePage(rows, if (from + limit < all.size) rows.last().taskId else null)
        }, changed = { statuses += it.status })
        assertEquals(TeamTriageList.Status.Loading, reader.list.status)
        reader.load()
        assertEquals(listOf(TeamTriageList.Status.Loading, TeamTriageList.Status.Ready), statuses)
        assertEquals(20, reader.list.items.size); assertEquals("task-0045", reader.list.items.first().taskId); assertEquals("task-0026", reader.list.nextCursor)
        reader.more()
        assertEquals(40, reader.list.items.size); assertEquals("task-0006", reader.list.items.last().taskId); assertEquals("task-0006", reader.list.nextCursor)
        reader.more()
        assertEquals(45, reader.list.items.size); assertNull("the end of the log", reader.list.nextCursor)
        val before = cursors.size
        reader.more()
        assertEquals("nothing is asked after the end", before, cursors.size)
        assertEquals(listOf(null, "task-0026", "task-0006"), cursors)
        assertEquals(45, reader.list.items.map { it.taskId }.toSet().size)
        reader.load()
        assertEquals("refreshing starts over from the newest", 20, reader.list.items.size)
    }

    @Test fun aPageThatRepeatsRowsOfTheOneBeforeAddsNothingTwice() = runBlocking {
        val pages = ArrayDeque(listOf(page(listOf(3, 2), next = "task-0002"), page(listOf(2, 1))))
        val reader = TeamTriageReader(read = { _, _ -> pages.removeFirst() }, changed = {})
        reader.load(); reader.more()
        assertEquals(listOf("task-0003", "task-0002", "task-0001"), reader.list.items.map { it.taskId })
    }

    @Test fun aReadThatFailsKeepsWhatWasReadAndWhereItStoppedAndOnlyNotTheMaintainerEndsTheLog() = runBlocking {
        val outcomes = ArrayDeque<Result<TeamTriagePage>>(listOf(Result.success(page(listOf(3, 2), next = "task-0002")), Result.failure(TeamMessageFailure("SERVICE_UNAVAILABLE")), Result.success(page(listOf(1)))))
        val cursors = mutableListOf<String?>()
        val reader = TeamTriageReader(read = { cursor, _ -> cursors += cursor; outcomes.removeFirst().getOrThrow() }, changed = {})
        reader.load(); reader.more()
        assertEquals(TeamTriageList.Status.Failed, reader.list.status); assertEquals("SERVICE_UNAVAILABLE", reader.list.error)
        assertEquals("the rows read before stay", 2, reader.list.items.size); assertEquals("task-0002", reader.list.nextCursor)
        reader.more()
        assertEquals(TeamTriageList.Status.Ready, reader.list.status); assertEquals(3, reader.list.items.size)
        assertEquals("reading again goes on from where it stopped", listOf(null, "task-0002", "task-0002"), cursors)
        // A first page that fails has nothing and starts again.
        val first = TeamTriageReader(read = { _, _ -> throw java.io.IOException("offline") }, changed = {})
        first.load()
        assertEquals(TeamTriageList.Status.Failed, first.list.status); assertTrue(first.list.items.isEmpty())
        assertEquals("an error that is not the service's is the service being unavailable", "SERVICE_UNAVAILABLE", first.list.error)
        first.more()
        assertEquals("there is no page to go on from", TeamTriageList.Status.Failed, first.list.status)
        // Not the maintainer: no log, no rows, nothing more to read.
        var reads = 0
        val denied = TeamTriageReader(read = { _, _ -> reads += 1; throw TeamMessageFailure("MAINTAINER_REQUIRED") }, changed = {})
        denied.load(); denied.more()
        assertEquals(TeamTriageList.Status.Denied, denied.list.status); assertTrue(denied.list.items.isEmpty()); assertEquals(1, reads)
        assertFalse(denied.list.busy)
    }

    @Test fun aReadThatWasOvertakenOrAnswersAfterTheListWasClosedChangesNothing() = runBlocking {
        val gates = List(3) { CompletableDeferred<Unit>() }
        val pages = listOf(page(listOf(1)), page(listOf(9)), page(listOf(5)))
        var arrivals = 0
        val seen = mutableListOf<TeamTriageList.Status>()
        val reader = TeamTriageReader(read = { _, _ -> val mine = arrivals++; gates[mine].await(); pages[mine] }, changed = { seen += it.status })
        val stale = launch { reader.load() }
        yield()
        val current = launch { reader.load() }
        yield()
        gates[1].complete(Unit); current.join()
        gates[0].complete(Unit); stale.join()
        assertEquals("the older answer did not replace the newer one", listOf("task-0009"), reader.list.items.map { it.taskId })
        val late = launch { reader.load() }
        yield()
        reader.close()
        val shown = seen.size
        gates[2].complete(Unit); late.join()
        assertEquals("nothing was shown after it was closed", shown, seen.size)
        assertTrue("what the closed read would have brought is not there", reader.list.items.isEmpty())
    }

    // Why a Bot stayed silent

    @Test fun theReasonOfASilenceIsLookedForFromTheNewestVerdictDownOnlyAsFarAsItMust() = runBlocking {
        val all = (260 downTo 1).map(::item)
        val limits = mutableListOf<Int>()
        val read: TeamTriageRead = { cursor, limit ->
            limits += limit
            val from = cursor?.let { id -> all.indexOfFirst { it.taskId == id } + 1 } ?: 0
            val rows = all.subList(from, minOf(all.size, from + limit))
            TeamTriagePage(rows, if (from + limit < all.size) rows.last().taskId else null)
        }
        val near = TeamTriageSearch.find(listOf("task-0260", "task-0220"), read = read)
        assertEquals(setOf("task-0260", "task-0220"), near.keys); assertEquals("both are on the first page: one read of 50", listOf(50), limits)
        limits.clear()
        val far = TeamTriageSearch.find(listOf("task-0260", "task-0150"), read = read)
        assertEquals(setOf("task-0260", "task-0150"), far.keys); assertEquals(3, limits.size)
        limits.clear()
        val lost = TeamTriageSearch.find(listOf("task-0150", "task-9999"), read = read)
        assertEquals("a task with no row is not in the answer", setOf("task-0150"), lost.keys)
        assertEquals("and the search is not endless", TeamTriageRules.FIND_PAGES, limits.size); assertEquals(4, TeamTriageRules.FIND_PAGES)
        limits.clear()
        assertTrue(TeamTriageSearch.find(listOf("task-0001"), pages = 2, read = read).isEmpty()); assertEquals("the cap of pages is a parameter", 2, limits.size)
        limits.clear()
        assertTrue(TeamTriageSearch.find(emptyList(), read = read).isEmpty()); assertEquals("nothing wanted, nothing read", 0, limits.size)
        // A cursor that does not move ends the search; a read that fails ends it with what it has.
        var stuck = 0
        assertTrue(TeamTriageSearch.find(listOf("x"), read = { _, _ -> stuck += 1; TeamTriagePage(emptyList(), "task-0001") }).isEmpty()); assertEquals(2, stuck)
        val two = item(2)
        val partial = TeamTriageSearch.find(listOf("task-0002", "task-0001"), read = { cursor, _ ->
            if (cursor != null) throw TeamMessageFailure("SERVICE_UNAVAILABLE")
            TeamTriagePage(listOf(two), "task-0002")
        })
        assertEquals(setOf("task-0002"), partial.keys)
    }

    @Test fun aSearchThatIsCancelledIsNotASearchThatFailed() = runBlocking {
        val gate = CompletableDeferred<Unit>()
        var entered = false
        val search = launch {
            TeamTriageSearch.find(listOf("task-0001"), read = { _, _ -> entered = true; gate.await(); TeamTriagePage(emptyList()) })
            fail("a cancelled search does not come back with an answer")
        }
        yield()
        assertTrue(entered)
        search.cancel(); search.join()
        assertTrue(search.isCancelled)
    }
}
