package com.lingxi.code.teams

import com.lingxi.code.R
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * Bots asking Bots (Bot v3 B8b), as a member reads it: the chain of a delegated task, the Bot a task waits for, who may cancel, the line of a row of the auto-reply log and the card of a Bot. The
 * decoding and the rules, no UI and no network; `packages/teams/delegation.ts` and iOS `TeamDelegationTests` say the same.
 */
class TeamDelegationsTest {
    private val alice = TeamMemberRef("alice", 1)
    private val bob = TeamMemberRef("bob", 1)
    private val carol = TeamMemberRef("carol", 1)
    private val dan = TeamMemberRef("dan", 1)

    private fun ref(member: TeamMemberRef) = JSONObject().put("uid", member.uid).put("generation", member.generation)
    private fun card(id: String, name: String, maintainer: TeamMemberRef, state: String = "published", more: JSONObject.() -> Unit = {}): TeamBotCard {
        val json = JSONObject().put("sharedBotId", id).put("name", name).put("description", "").put("maintainer", ref(maintainer)).put("revision", 1).put("state", state)
            .put("permitted", true).put("ready", true).put("onlineUntil", 0).put("usePolicy", JSONObject().put("mode", "all")).put("quotas", JSONObject().put("team", 1).put("member", 1).put("bot", 1))
        json.more()
        return TeamBotCard.parse(json)
    }
    private fun cards() = listOf(card("sharedbot_planner", "Planner", bob), card("sharedbot_travel", "Travel", carol))
    private fun link(id: String, name: String) = JSONObject().put("sharedBotId", id).put("name", name)
    /** What the service writes of a task another Bot asked for (B8a): Alice asked Planner, Planner asked Travel. */
    private fun delegation(parent: String = "sharedbot_planner", chain: List<JSONObject> = listOf(link("sharedbot_planner", "Planner"), link("sharedbot_travel", "Travel"))) =
        JSONObject().put("delegationId", "delegation_1").put("rootTaskId", "task_root").put("parentTaskId", "task_root").put("parentBotId", parent).put("depth", 1)
            .put("expectReply", true).put("chain", JSONArray(chain))
    private fun task(id: String = "task_child", bot: String = "sharedbot_travel", state: String = "running", delegation: JSONObject? = null, wait: JSONObject? = null, more: JSONObject.() -> Unit = {}): TeamTask {
        val json = JSONObject().put("taskId", id).put("conversationId", "chat_0001").put("sharedBotId", bot).put("requester", ref(alice)).put("state", state).put("createdAt", 1).put("deadline", 2)
        if (delegation != null) json.put("delegation", delegation)
        if (wait != null) json.put("wait", wait)
        json.more()
        return TeamTask.parse(json)
    }
    /** The task of Planner that waits for the reply of Travel's task; the person who asked is the recipient of the wait, and has nothing to answer. */
    private fun plannerWaiting(kind: String = "delegation"): TeamTask {
        val wait = JSONObject().put("waitId", "wait_1").put("kind", kind).put("recipient", ref(alice)).put("expiresAt", 9_000_000L).put("messageId", "message_1")
        if (kind == "delegation") wait.put("delegation", JSONObject().put("sharedBotId", "sharedbot_travel").put("taskId", "task_child"))
        return task("task_root", bot = "sharedbot_planner", state = "waiting", wait = wait)
    }

    // The chain

    @Test fun aTaskAnotherBotAskedForCarriesWhoAskedAndTheChainFromTheBotThePersonAsked() {
        val asked = task(delegation = delegation())
        assertEquals(TeamTaskDelegation("sharedbot_planner", listOf(TeamDelegationLink("sharedbot_planner", "Planner"), TeamDelegationLink("sharedbot_travel", "Travel"))), asked.delegation)
        assertNull("a task nobody delegated has none", task().delegation)
        assertEquals("what this version does not read of a task does not fail it", "task_child", task(more = { put("somethingNew", JSONObject().put("a", 1)) }).taskId)
        // What cannot be read as a delegation is none, and the task is still a task.
        for (bad in listOf<Any>(JSONArray(), "Planner → Travel", JSONObject().put("parentBotId", "x"), delegation(chain = emptyList()), delegation(parent = ""),
            JSONObject().put("parentBotId", "sharedbot_planner").put("chain", JSONArray().put(JSONObject().put("sharedBotId", "a"))))) {
            val read = task(more = { put("delegation", bad) })
            assertNull("$bad", read.delegation); assertEquals("task_child", read.taskId)
        }
    }

    @Test fun theChainReadsFromTheBotAskedToTheBotThatRunsTheTaskAndNamesNoBotByItsId() {
        val cards = cards()
        assertEquals("Planner → Travel", TeamDelegations.chain(task(delegation = delegation()), cards, "Bot"))
        assertNull("a task nobody delegated", TeamDelegations.chain(task(), cards, "Bot"))
        assertNull("a chain of one Bot is no chain", TeamDelegations.chain(task(delegation = delegation(chain = listOf(link("sharedbot_travel", "Travel")))), cards, "Bot"))
        val three = delegation(chain = listOf(link("sharedbot_planner", "Planner"), link("sharedbot_travel", "Travel"), link("sharedbot_chef", "Chef")))
        assertEquals("at most two hops, so at most three Bots", "Planner → Travel → Chef", TeamDelegations.chain(task(delegation = three), cards, "Bot"))
        // A Bot is named by the card the Team lists now; the name it had when it was asked is for a Bot that has no card.
        val renamed = listOf(card("sharedbot_planner", "Planner", bob), card("sharedbot_travel", "Travel 2", carol))
        assertEquals("Planner → Travel 2", TeamDelegations.chain(task(delegation = delegation()), renamed, "Bot"))
        assertEquals("no card of either: the names of the chain", "Planner → Travel", TeamDelegations.chain(task(delegation = delegation()), emptyList(), "Bot"))
        // A chain made from a handoff names a Bot by its id: that is no name.
        val byId = delegation(chain = listOf(link("sharedbot_planner", "sharedbot_planner"), link("sharedbot_travel", "Travel")))
        assertEquals("Bot → Travel", TeamDelegations.chain(task(delegation = byId), emptyList(), "Bot"))
    }

    // Waiting for a Bot

    @Test fun aTaskThatWaitsForAnotherBotNamesItAndIsNotAQuestionToThePersonWhoAsked() {
        val waiting = plannerWaiting()
        assertEquals("delegation", waiting.wait?.kind)
        assertEquals(TeamTaskWaitDelegation("sharedbot_travel", "task_child"), waiting.wait?.delegation)
        assertEquals("the service names the person who asked as the recipient", alice, waiting.wait?.recipient)
        assertNull("there is nothing for them to answer", TeamTaskActions.answerableWait(waiting, alice, emptySet()))
        assertNull("a question to a person waits for no Bot", plannerWaiting("human_input").wait?.delegation)
        assertEquals("a question to a person is answered as before", "wait_1", TeamTaskActions.answerableWait(plannerWaiting("human_input"), alice, emptySet()))
        assertFalse(waiting.terminal); assertFalse(waiting.busy)
        // A wait whose Bot this version cannot read is still a wait.
        val unreadable = task("task_root", bot = "sharedbot_planner", state = "waiting", wait = JSONObject().put("waitId", "wait_1").put("kind", "delegation").put("recipient", ref(alice))
            .put("expiresAt", 9_000_000L).put("messageId", "message_1").put("delegation", JSONObject().put("sharedBotId", "sharedbot_travel")))
        assertNull(unreadable.wait?.delegation); assertEquals("delegation", unreadable.wait?.kind)
    }

    @Test fun theBotATaskWaitsForIsNamedByItsCardThenByTheTaskItAskedAndThenNotAtAll() {
        val waiting = plannerWaiting()
        val planner = card("sharedbot_planner", "Planner", bob)
        val asked = task("task_child", delegation = delegation(chain = listOf(link("sharedbot_planner", "Planner"), link("sharedbot_travel", "Trip desk"))))
        assertEquals("by its card", "Travel", TeamDelegations.waitingFor(waiting, cards()))
        assertEquals("no card of it: the name it had when it was asked", "Trip desk", TeamDelegations.waitingFor(waiting, listOf(planner), listOf(asked)))
        assertNull("no name to give", TeamDelegations.waitingFor(waiting, listOf(planner)))
        assertNull("another task is not the one it asked", TeamDelegations.waitingFor(waiting, emptyList(), listOf(task("task_other", delegation = delegation()))))
        val byId = task("task_child", delegation = delegation(chain = listOf(link("sharedbot_planner", "Planner"), link("sharedbot_travel", "sharedbot_travel"))))
        assertNull("an id is not a name", TeamDelegations.waitingFor(waiting, emptyList(), listOf(byId)))
        assertNull("a task that waits for a person names no Bot", TeamDelegations.waitingFor(plannerWaiting("human_input"), cards()))
        val over = task("task_root", bot = "sharedbot_planner", state = "running", wait = JSONObject().put("waitId", "w").put("kind", "delegation").put("recipient", ref(alice)).put("expiresAt", 1L)
            .put("messageId", "m").put("delegation", JSONObject().put("sharedBotId", "sharedbot_travel").put("taskId", "task_child")))
        assertNull("a wait that is over is not shown", TeamDelegations.waitingFor(over, cards()))
    }

    // Who may cancel

    @Test fun whoMayCancelTheRequesterTheMaintainerOfTheBotThatRunsItAndTheMaintainerOfTheBotThatAsked() {
        val cards = cards()
        fun may(viewer: TeamMemberRef, task: TeamTask) = TeamTaskActions.canCancel(task, viewer, cards.firstOrNull { it.sharedBotId == task.sharedBotId }?.maintainer, TeamDelegations.delegator(task, cards))
        val asked = task(delegation = delegation())
        assertEquals("the maintainer of Planner, which asked", bob, TeamDelegations.delegator(asked, cards))
        assertTrue("the person who asked", may(alice, asked)); assertTrue("the maintainer of the Bot that runs it", may(carol, asked)); assertTrue("the maintainer of the Bot that asked for it", may(bob, asked))
        assertFalse("anybody else", may(dan, asked)); assertFalse("the membership they held before is not the one they hold now", may(TeamMemberRef("bob", 0), asked))
        // Planner's maintainer may stop what Planner asked for, not what somebody else asked Travel for.
        val plain = task()
        assertNull(TeamDelegations.delegator(plain, cards)); assertFalse(may(bob, plain)); assertTrue(may(carol, plain))
        assertNull("a Bot that is not in the list is nobody's to maintain", TeamDelegations.delegator(asked, listOf(cards[1]))); assertFalse(TeamTaskActions.canCancel(asked, bob, carol, null))
        assertNull("the Bot that asked is the one in the delegation, not any Bot of the chain", TeamDelegations.delegator(task(delegation = delegation(parent = "sharedbot_other")), cards))
        for (state in listOf("completed", "failed", "cancelled", "expired")) assertFalse(state, may(bob, task(state = state, delegation = delegation())))
        for (state in listOf("queued", "running", "waiting", "awaiting_approval")) assertTrue(state, may(bob, task(state = state, delegation = delegation())))
        assertFalse("it is already being stopped", may(bob, task(delegation = delegation(), more = { put("cancelRequestedAt", 5L) })))
        assertFalse("nobody is signed in", TeamTaskActions.canCancel(asked, null, carol, bob))
    }

    // The auto-reply log

    private fun row(more: JSONObject.() -> Unit = {}): JSONObject {
        val json = JSONObject().put("taskId", "task-0001").put("conversationId", "chat_0001").put("messageId", "message-1").put("createdAt", 1_700_000_000_000L).put("state", "completed")
            .put("triage", JSONObject().put("action", "run").put("source", "rule").put("reason", "addressed").put("at", 1_700_000_001_000L))
        json.more()
        return json
    }

    @Test fun aRowOfTheLogWhoseTaskAnotherBotAskedForSaysWhichBotAndOneThisVersionCannotReadIsStillARow() {
        val read = checkNotNull(TeamTriageItem.parse(row { put("delegation", delegation()) }))
        assertEquals("sharedbot_planner", read.delegation?.parentBotId)
        assertEquals("Planner", TeamDelegations.delegatedBy(read.delegation, "Bot"))
        assertNull("a message a person sent", checkNotNull(TeamTriageItem.parse(row())).delegation); assertNull(TeamDelegations.delegatedBy(null, "Bot"))
        assertEquals("a Bot named by its id is the Bot", "Bot", TeamDelegations.delegatedBy(TeamTaskDelegation.parse(delegation(chain = listOf(link("sharedbot_planner", "sharedbot_planner"), link("sharedbot_travel", "Travel")))), "Bot"))
        assertEquals("a parent that is not in the chain has no name to give", "Bot", TeamDelegations.delegatedBy(TeamTaskDelegation.parse(delegation(parent = "sharedbot_gone")), "Bot"))
        for (bad in listOf<Any>(JSONObject().put("parentBotId", "sharedbot_planner").put("chain", JSONArray()), "planner", 7, JSONArray(), JSONObject.NULL)) {
            val item = checkNotNull(TeamTriageItem.parse(row { put("delegation", bad) }))
            assertNull("$bad", item.delegation); assertEquals("the row is still a row", "task-0001", item.taskId)
        }
        val page = TeamTriagePage.parse(JSONObject().put("items", JSONArray().put(row { put("delegation", delegation()) })))
        assertEquals("a page carries it", 2, page.items.first().delegation?.chain?.size)
    }

    // The Bot's card

    @Test fun aPublishedBotsCardSaysWhomItTakesRequestsFromAndWhomItMayAskByName() {
        val cards = cards()
        fun bot(state: String = "published", more: JSONObject.() -> Unit = {}) = card("sharedbot_desk", "Desk", dan, state, more)
        assertNull("a Bot published before delegation says nothing of it", bot().acceptDelegation)
        assertEquals(TeamDelegations.Accepts.Nobody, TeamDelegations.accepts(bot(), cards))
        assertEquals(TeamDelegations.Accepts.Nobody, TeamDelegations.accepts(bot { put("acceptDelegation", "none") }, cards))
        assertEquals(TeamDelegations.Accepts.TeamBots, TeamDelegations.accepts(bot { put("acceptDelegation", "team_bots") }, cards))
        assertEquals(TeamDelegations.Accepts.Listed(listOf("Planner", "Travel")), TeamDelegations.accepts(bot { put("acceptDelegation", "listed"); put("acceptFrom", JSONArray(listOf("sharedbot_planner", "sharedbot_travel"))) }, cards))
        assertEquals("a Bot that is gone from the Team is not listed", TeamDelegations.Accepts.Listed(listOf("Planner")),
            TeamDelegations.accepts(bot { put("acceptDelegation", "listed"); put("acceptFrom", JSONArray(listOf("sharedbot_gone", "sharedbot_planner"))) }, cards))
        assertEquals("a list of Bots that are all gone takes nobody", TeamDelegations.Accepts.Nobody, TeamDelegations.accepts(bot { put("acceptDelegation", "listed"); put("acceptFrom", JSONArray(listOf("sharedbot_gone"))) }, cards))
        assertEquals("a list that is not there is no one", TeamDelegations.Accepts.Nobody, TeamDelegations.accepts(bot { put("acceptDelegation", "listed") }, cards))
        assertEquals("a code this version does not know takes nobody", TeamDelegations.Accepts.Nobody, TeamDelegations.accepts(bot { put("acceptDelegation", "from_the_future") }, cards))
        assertEquals(listOf("Planner", "Travel"), TeamDelegations.delegatesTo(bot { put("delegateTo", JSONArray(listOf("sharedbot_gone", "sharedbot_planner", "sharedbot_travel"))) }, cards))
        assertEquals("what is no list of text is no list", emptyList<String>(), bot { put("delegateTo", "sharedbot_planner") }.delegateTo)
        // The items, as the card says them: "label: value", the names as they are, and nothing about the limit its maintainer set.
        assertEquals(listOf(TeamBotItem(R.string.teams_bot_accepts_delegation, TeamBotValue.Words(R.string.teams_bot_accepts_team))), TeamBotEntry.delegation(bot { put("acceptDelegation", "team_bots") }, cards))
        assertEquals(listOf(TeamBotItem(R.string.teams_bot_accepts_delegation, TeamBotValue.Words(R.string.teams_bot_accepts_none))), TeamBotEntry.delegation(bot(), cards))
        val listed = bot { put("acceptDelegation", "listed"); put("acceptFrom", JSONArray(listOf("sharedbot_planner"))); put("delegateTo", JSONArray(listOf("sharedbot_travel"))) }
        assertEquals(
            listOf(TeamBotItem(R.string.teams_bot_accepts_delegation, TeamBotValue.Figure("Planner")), TeamBotItem(R.string.teams_bot_delegates_to, TeamBotValue.Figure("Travel"))),
            TeamBotEntry.delegation(listed, cards),
        )
        val limited = TeamBotEntry.delegation(bot { put("acceptDelegation", "team_bots"); put("delegation", JSONObject().put("daily", 9)); put("delegateTo", JSONArray(listOf("sharedbot_travel"))) }, cards)
        assertEquals(2, limited.size); assertFalse("the limit of its maintainer is not for a member", limited.any { (it.value as? TeamBotValue.Figure)?.text?.contains("9") == true })
        assertTrue("a Bot that is not published is not asked", TeamBotEntry.delegation(bot("paused") { put("acceptDelegation", "team_bots") }, cards).isEmpty())
    }

    // Words, and what only the service sends

    private fun words(directory: String): Map<String, String> {
        val file = listOf("src/main/res/$directory/strings.xml", "app/src/main/res/$directory/strings.xml", "apps/android/native/app/src/main/res/$directory/strings.xml").map(::File).first { it.exists() }
        // Plain text on purpose: the Android XML classes on the unit-test class path are stubs.
        return Regex("<string\\s+name=\"([^\"]+)\"[^>]*>(.+?)</string>", RegexOption.DOT_MATCHES_ALL).findAll(file.readText()).associate { it.groupValues[1] to it.groupValues[2] }
    }

    @Test fun theDelegationWordsAreInEveryLanguageAndTheTraditionalChineseIsNotTheSimplified() {
        val names = listOf("teams_task_waiting_bot_fmt", "teams_task_delegation_chain_fmt", "teams_triage_delegated_by_fmt", "teams_bot_accepts_delegation", "teams_bot_accepts_none", "teams_bot_accepts_team", "teams_bot_delegates_to")
        val hans = words("values")
        for (directory in listOf("values", "values-zh-rTW", "values-en", "values-ja", "values-ko")) {
            val text = words(directory)
            for (name in names) {
                assertFalse("$directory has no text for $name", text[name].isNullOrBlank())
                // A Bot is a Bot, in every language.
                assertFalse("$directory $name", text.getValue(name).contains("assistant", ignoreCase = true) || listOf("助手", "助理", "アシスタント", "어시스턴트").any { text.getValue(name).contains(it) })
                if (name.endsWith("_fmt")) assertTrue("$directory: $name keeps the name it says", text.getValue(name).contains("%1\$s"))
                if (directory != "values" && !(name == "teams_bot_accepts_none" && directory == "values-zh-rTW")) assertNotEquals("$directory $name", hans.getValue(name), text.getValue(name))
            }
        }
        assertEquals("Waiting for %1\$s", words("values-en").getValue("teams_task_waiting_bot_fmt")); assertEquals("Delegated: %1\$s", words("values-en").getValue("teams_task_delegation_chain_fmt"))
        assertEquals("委派：%1\$s", words("values-zh-rTW").getValue("teams_task_delegation_chain_fmt")); assertEquals("委托：%1\$s", hans.getValue("teams_task_delegation_chain_fmt"))
        assertTrue("the log says it with this", "teams_triage_delegated_by_fmt" in TeamTriageRules.stringNames)
    }

    @Test fun whatOnlyTheServiceSendsIsRefusedFromAMember() {
        val root = generateSequence(File(System.getProperty("user.dir"))) { it.parentFile }.first { File(it, "packages/teams/contracts/execution-vectors.json").isFile }
        val rows = JSONObject(File(root, "packages/teams/contracts/execution-vectors.json").readText()).getJSONArray("commands").let { array -> (0 until array.length()).map { array.getJSONObject(it) } }
        for (name in listOf("reject-delegateTask-member", "reject-detachDelegation-member")) {
            val row = checkNotNull(rows.firstOrNull { it.getString("name") == name }) { "$name is in the contract" }
            assertFalse(name, row.getBoolean("valid"))
            assertTrue("$name: a member cannot send it", runCatching { TeamExecutionContract.validate(row.getJSONObject("input")) }.isFailure)
        }
    }
}
