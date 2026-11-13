package com.lingxi.code.teams

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The usage line under a Team task (same rules as Desktop's `usageText`), which messages are Markdown, and which links a Bot may open. */
class TeamsBotTextTest {
    private val words = TeamUsageStrings.english
    private fun line(json: String?) = TeamsBotText.usageLine(json?.let { TeamTaskUsage.parse(JSONObject(it)) }, words)

    @Test fun missingOrAbsentUsageNeverInventsACost() {
        assertEquals("Missing usage · cost unknown", line(null))
        assertEquals("Missing usage · cost unknown", line("""{"status":"missing"}"""))
    }

    @Test fun partialUsageWithoutCostSaysCostUnknown() {
        assertEquals("Partial usage · 3 / 4 tokens · cost unknown", line("""{"status":"partial","inputTokens":3,"outputTokens":4}"""))
    }

    @Test fun completeUsageShowsTokensAndCostInSixDecimals() {
        assertEquals("Complete usage · 123 / 456 tokens · 0.001322 USD", line("""{"status":"complete","inputTokens":123,"outputTokens":456,"costMicros":1322,"currency":"USD"}"""))
        assertEquals("Complete usage · 3 / 4 tokens · 0.000000 USD", line("""{"status":"complete","inputTokens":3,"outputTokens":4,"costMicros":0,"currency":"USD"}"""))
    }

    @Test fun costWithoutCurrencySaysCurrencyUnknown() {
        assertEquals("Complete usage · 1 / 2 tokens · 2.500000 currency unknown", line("""{"status":"complete","inputTokens":1,"outputTokens":2,"costMicros":2500000}"""))
    }

    @Test fun taskParsesUsageWhenPresentAndToleratesItsAbsence() {
        val base = """{"taskId":"t","conversationId":"c","sharedBotId":"b","requester":{"uid":"u","generation":1},"state":"completed","createdAt":1,"deadline":2"""
        val with = TeamTask.parse(JSONObject(base + ""","usage":{"status":"partial","inputTokens":5,"outputTokens":6,"costMicros":null}}"""))
        assertEquals(TeamTaskUsage("partial", 5, 6, null, null), with.usage)
        assertNull(TeamTask.parse(JSONObject("$base}")).usage)
    }

    // Waiting and cancelling (M2)

    private fun task(state: String, extra: String = "") = TeamTask.parse(JSONObject(
        """{"taskId":"t","conversationId":"c","sharedBotId":"b","requester":{"uid":"asker","generation":3},"state":"$state","createdAt":1,"deadline":2$extra}"""))
    private val wait = ""","wait":{"waitId":"wait_1","kind":"human_input","recipient":{"uid":"asker","generation":3},"expiresAt":9000000,"messageId":"question"}"""
    private val asker = TeamMemberRef("asker", 3)
    private val keeper = TeamMemberRef("keeper", 3)
    private val other = TeamMemberRef("other", 3)

    @Test fun aWaitingTaskIsNeitherFinishedNorPolled() {
        val waiting = task("waiting", wait)
        assertEquals(TeamTaskWait("wait_1", "human_input", asker, 9_000_000, "question"), waiting.wait)
        assertFalse(waiting.terminal); assertFalse(waiting.busy)
        for (state in listOf("queued", "running")) { assertTrue(state, task(state).busy); assertFalse(state, task(state).terminal) }
        for (state in listOf("completed", "failed", "expired", "interrupted", "authorization_required", "revoked", "cancelled")) {
            assertTrue(state, task(state).terminal); assertFalse(state, task(state).busy)
        }
        assertNull(task("running").wait); assertNull(task("running").cancelRequestedAt)
        assertNull(task("running", ""","cancelRequestedAt":null,"wait":null""").wait)
    }

    @Test fun onlyTheRequesterAndTheMaintainerMayCancelAnUnfinishedTask() {
        for (state in listOf("queued", "running", "waiting")) {
            val live = task(state, if (state == "waiting") wait else "")
            assertTrue(state, TeamTaskActions.canCancel(live, asker, keeper))
            assertTrue(state, TeamTaskActions.canCancel(live, keeper, keeper))
            assertFalse(state, TeamTaskActions.canCancel(live, other, keeper))
            assertFalse(state, TeamTaskActions.canCancel(live, other, null))
            assertFalse(state, TeamTaskActions.canCancel(live, null, null))
        }
        // The same person after rejoining is another member.
        assertFalse(TeamTaskActions.canCancel(task("queued"), TeamMemberRef("asker", 4), null))
        for (state in listOf("completed", "failed", "cancelled", "expired")) assertFalse(state, TeamTaskActions.canCancel(task(state), asker, keeper))
        val stopping = task("running", ""","cancelRequestedAt":5""")
        assertTrue(stopping.cancelling)
        assertFalse(TeamTaskActions.canCancel(stopping, asker, keeper))
        assertFalse(task("cancelled", ""","cancelRequestedAt":5""").cancelling)
    }

    @Test fun onlyTheMemberATaskWaitsForMayAnswerItAndOnlyOnce() {
        val waiting = task("waiting", wait)
        assertEquals("wait_1", TeamTaskActions.answerableWait(waiting, asker, emptySet()))
        assertNull(TeamTaskActions.answerableWait(waiting, keeper, emptySet()))
        assertNull(TeamTaskActions.answerableWait(waiting, null, emptySet()))
        assertNull(TeamTaskActions.answerableWait(waiting, TeamMemberRef("asker", 4), emptySet()))
        assertNull(TeamTaskActions.answerableWait(waiting, asker, setOf("wait_1")))
        assertEquals("wait_1", TeamTaskActions.answerableWait(waiting, asker, setOf("wait_0")))
        assertNull(TeamTaskActions.answerableWait(task("running", wait), asker, emptySet()))
        assertNull(TeamTaskActions.answerableWait(task("waiting"), asker, emptySet()))
        assertNull(TeamTaskActions.answerableWait(null as TeamTask?, asker, emptySet()))
    }

    @Test fun onlyTheBotsQuestionOfAWaitingTaskIsAnswerable() {
        fun message(kind: String, taskId: String?, waitId: String?) = TeamMessage("m", 1, if (kind == "user") "asker" else "", kind, "Which colour?", 1, "change", false, null, null, taskId = taskId, waitId = waitId)
        val tasks = listOf(task("waiting", wait))
        assertEquals("wait_1", TeamTaskActions.answerableWait(message("shared_bot", "t", "wait_1"), tasks, asker, emptySet()))
        // An earlier question of the same task, the requester's own answer, a message without a task and an unknown task are not.
        assertNull(TeamTaskActions.answerableWait(message("shared_bot", "t", "wait_0"), tasks, asker, emptySet()))
        assertNull(TeamTaskActions.answerableWait(message("user", "t", "wait_1"), tasks, asker, emptySet()))
        assertNull(TeamTaskActions.answerableWait(message("shared_bot", null, "wait_1"), tasks, asker, emptySet()))
        assertNull(TeamTaskActions.answerableWait(message("shared_bot", "t", null), tasks, asker, emptySet()))
        assertNull(TeamTaskActions.answerableWait(message("shared_bot", "gone", "wait_1"), tasks, asker, emptySet()))
        assertNull(TeamTaskActions.answerableWait(message("shared_bot", "t", "wait_1"), tasks, keeper, emptySet()))
        assertNull(TeamTaskActions.answerableWait(null as TeamMessage?, tasks, asker, emptySet()))
        val outbox = listOf(JSONObject("""{"action":"answerWait","payload":{"waitId":"wait_1","changeId":"c1"}}"""), JSONObject("""{"action":"mutate","changeId":"c2"}"""), JSONObject("""{"action":"submit","payload":{"changeId":"c3"}}"""))
        assertEquals(setOf("wait_1"), TeamTaskActions.pendingWaitIds(outbox))
        assertNull(TeamTaskActions.answerableWait(message("shared_bot", "t", "wait_1"), tasks, asker, TeamTaskActions.pendingWaitIds(outbox)))
    }

    // Approving a list change (M2 Q05)
    private val approvalWait = ""","epoch":2,"wait":{"waitId":"wait_1","kind":"tool_approval","recipient":{"uid":"asker","generation":3},"expiresAt":9000000,"messageId":"request"}"""
    private fun posted(author: String, extra: String = "") = TeamMessage.parse(JSONObject("""{"id":"m","seq":1,"author":$author,"kind":"text","text":"x","changeSeq":1,"changeId":"change_0001","taskId":"t","waitId":"wait_1"$extra}"""))
    private val bot = """{"kind":"shared_bot","sharedBotId":"sharedbot_01","displayName":"Helper"}"""
    private val person = """{"kind":"user","uid":"asker"}"""

    @Test fun aProposedChangeIsDecidedOnItsCardAndNeverAnsweredInWords() {
        val waiting = task("waiting", approvalWait)
        assertNull(TeamTaskActions.answerableWait(waiting, asker, emptySet()))
        assertEquals(TeamApprovalState.Decide, TeamTaskActions.approvalState("wait_1", waiting, asker, emptyList()))
        assertEquals(TeamApprovalState.Waiting("asker"), TeamTaskActions.approvalState("wait_1", waiting, keeper, emptyList()))
        assertEquals(TeamApprovalState.Waiting("asker"), TeamTaskActions.approvalState("wait_1", waiting, null, emptyList()))
        // The same person after rejoining is another member.
        assertEquals(TeamApprovalState.Waiting("asker"), TeamTaskActions.approvalState("wait_1", waiting, TeamMemberRef("asker", 4), emptyList()))
        // A question is not an approval, and another wait of the same task is not this request.
        assertEquals(TeamApprovalState.Closed, TeamTaskActions.approvalState("wait_1", task("waiting", wait), asker, emptyList()))
        assertEquals(TeamApprovalState.Closed, TeamTaskActions.approvalState("wait_0", waiting, asker, emptyList()))
    }

    @Test fun aDecidedRequestSaysWhoDecidedAndAnUndecidedOneIsClosed() {
        val done = task("completed")
        val approved = posted(person, ""","decision":"approve""""); val declined = posted(person, ""","decision":"deny"""")
        assertEquals(TeamApprovalState.Approved("asker"), TeamTaskActions.approvalState("wait_1", done, asker, listOf(approved)))
        assertEquals(TeamApprovalState.Declined("asker"), TeamTaskActions.approvalState("wait_1", null, other, listOf(declined)))
        assertEquals(TeamApprovalState.Closed, TeamTaskActions.approvalState("wait_1", task("expired"), asker, emptyList()))
        // Only a member's reply with one of the two decisions counts.
        for (stray in listOf(posted(person), posted(person, ""","decision":"maybe""""), posted(person, ""","decision":true"""), posted(bot, ""","decision":"approve"""")))
            assertEquals(TeamApprovalState.Closed, TeamTaskActions.approvalState("wait_1", done, asker, listOf(stray)))
        assertEquals(TeamApprovalState.Closed, TeamTaskActions.approvalState("wait_2", done, asker, listOf(approved)))
    }

    @Test fun aRequestIsShownOnlyWhenEveryChangeCanBeSaidInWords() {
        val changes = """[{"op":"add","text":"Bread"},{"op":"update","itemId":"item_0001","done":true,"item":"Milk"},{"op":"remove","itemId":"item_0002","item":null}]"""
        val request = posted(bot, ""","approval":{"title":"Groceries","changes":$changes}""").approvalRequest!!
        assertEquals("Groceries", request.title)
        assertEquals(listOf(TeamListApprovalChange("add", text = "Bread"), TeamListApprovalChange("update", done = true, item = "Milk"), TeamListApprovalChange("remove")), request.changes)
        // A request that cannot be read never fails the message it is on; it is just not shown as a request.
        for (broken in listOf(""""soon"""", """{"title":"Groceries"}""", """{"title":"Groceries","changes":[]}""", """{"title":"Groceries","changes":[{"op":"clear"}]}""", """{"title":"Groceries","changes":[{"op":"add"}]}""",
            """{"title":"Groceries","changes":[{"op":"update","itemId":"item_0001"}]}""", """{"title":"Groceries","changes":[{"op":"add","text":5}]}""", """{"changes":$changes}""")) {
            val read = posted(bot, ""","approval":$broken""")
            assertNull(broken, read.approvalRequest); assertEquals("x", read.text)
        }
        // Only a Bot asks, and a recalled request asks nothing.
        assertNull(posted(person, ""","approval":{"title":"Groceries","changes":$changes}""").approvalRequest)
        assertNull(posted(bot, ""","recalled":true,"approval":{"title":"Groceries","changes":$changes}""").approvalRequest)
    }

    @Test fun aWaitExpiryIsRoundedUpToMinutesHoursOrDays() {
        fun expiry(ms: Long) = TeamTaskActions.expiry(1_000_000 + ms, 1_000_000).let { "${it.first} ${it.second}" }
        assertEquals("1 MINUTES", expiry(-5))
        assertEquals("1 MINUTES", expiry(1))
        assertEquals("1 MINUTES", expiry(60_000))
        assertEquals("2 MINUTES", expiry(60_001))
        assertEquals("59 MINUTES", expiry(59 * 60_000L))
        assertEquals("1 HOURS", expiry(60 * 60_000L))
        assertEquals("2 HOURS", expiry(61 * 60_000L))
        assertEquals("24 HOURS", expiry(24 * 3_600_000L))
        assertEquals("47 HOURS", expiry(47 * 3_600_000L))
        assertEquals("2 DAYS", expiry(48 * 3_600_000L))
        assertEquals("3 DAYS", expiry(49 * 3_600_000L))
    }

    @Test fun onlySharedBotMessagesAreMarkdown() {
        assertTrue(TeamsBotText.rendersMarkdown("shared_bot", false))
        assertFalse(TeamsBotText.rendersMarkdown("user", false))
        assertFalse(TeamsBotText.rendersMarkdown("shared_bot", true))
    }

    @Test fun botLinksLeaveTheAppOnlyAsWebPages() {
        for (ok in listOf("https://example.com/a", "HTTP://example.com", "http://example.com:8080/x?y=1")) assertTrue(ok, TeamsBotText.allowsLink(ok))
        for (bad in listOf("javascript:alert(1)", "tel:123", "mailto:a@b.c", "file:///etc/passwd", "lingxi://open_terminal?initCommand=ls", "sms:1", "https:///nohost", "data:text/html,hi", "intent://x#Intent;end", "not a url", "")) assertFalse(bad, TeamsBotText.allowsLink(bad))
    }

    @Test fun aBotsMarkdownReadsAsOnePlainLine() {
        val vectors = listOf(
            "# QA Follow-Up Check\n\n**Goal: close the open items.**\n\n- Scope: defects" to "QA Follow-Up Check Goal: close the open items. Scope: defects",
            "**G" to "G",
            "- [ ] Confirm the list\n- [x] Done" to "Confirm the list Done",
            "| Item | Owner |\n|---|---|\n| Confirm scope | QA 2 |" to "Item Owner Confirm scope QA 2",
            "See [the doc](https://example.com/a) and `code`" to "See the doc and code",
            "```ts\nconst a = 1;\n```" to "const a = 1;",
            "2 * 3 * 4 and snake_case_name" to "2 * 3 * 4 and snake_case_name",
            "1. first\n2. second" to "first second",
            "> quoted _text_" to "quoted text",
            "![chart](https://x/y.png)" to "chart",
            "---" to "",
            "" to "",
        )
        for ((markdown, plain) in vectors) assertEquals(markdown, plain, TeamsBotText.plainText(markdown))
    }

    @Test fun plainTextKeepsWhatIsNotMarkup() {
        assertEquals("[ ] not a list item", TeamsBotText.plainText("[ ] not a list item"))
        assertEquals("a*b*c and a_b_c", TeamsBotText.plainText("a*b*c and a_b_c"))
        assertEquals("emphasis and more", TeamsBotText.plainText("*emphasis* and __more__"))
        assertEquals("a b", TeamsBotText.plainText("a\r\n\r\nb"))
        assertEquals("a b", TeamsBotText.plainText("| a | b |\n| :--- | ---: |"))
        assertEquals("run ls | grep x", TeamsBotText.plainText("run `ls | grep x`"))
    }
}
