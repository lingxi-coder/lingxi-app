package com.lingxi.code.teams

import com.lingxi.code.R
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.math.BigDecimal
import java.util.Locale

/**
 * The usage line under a Team task (U5: the total tokens and the cost in the task's currency), the states of a run card (U4), which messages are
 * Markdown, and which links a Bot may open.
 */
class TeamsBotTextTest {
    private val words = TeamUsageStrings.english
    private fun usage(json: String?) = json?.let { TeamTaskUsage.parse(JSONObject(it)) }
    private fun line(json: String?) = TeamsBotText.usageLine(usage(json), words)
    private fun complete(input: Long, output: Long, costMicros: String, currency: String? = "USD") =
        """{"status":"complete","inputTokens":$input,"outputTokens":$output,"costMicros":$costMicros""" + (currency?.let { ""","currency":"$it"""" } ?: "") + "}"

    @Test fun missingOrAbsentUsageIsUnavailableAndNeverInventsACost() {
        assertEquals("Usage unavailable", line(null))
        assertEquals("Usage unavailable", line("""{"status":"missing"}"""))
        assertNull(TeamsBotText.tokenSplit(null, words))
        assertNull(TeamsBotText.tokenSplit(usage("""{"status":"missing"}"""), words))
    }

    @Test fun partialUsagePutsPartialUsageInFront() {
        assertEquals("Partial usage \u00b7 7 tokens \u00b7 cost unknown", line("""{"status":"partial","inputTokens":3,"outputTokens":4}"""))
        assertEquals("Partial usage \u00b7 7 tokens \u00b7 $0.0012", line("""{"status":"partial","inputTokens":3,"outputTokens":4,"costMicros":1200,"currency":"USD"}"""))
    }

    @Test fun completeUsageHasNoPrefixAndShowsTheTotalOfInputAndOutput() {
        assertEquals("579 tokens \u00b7 $0.0013", line(complete(123, 456, "1322")))
        assertEquals("1.2K tokens \u00b7 $0.0012", line(complete(1_000, 234, "1200")))
        assertEquals("3.5M tokens \u00b7 $12.50", line(complete(3_000_000, 500_000, "12500000")))
        assertEquals("0 tokens \u00b7 $0.00", line(complete(0, 0, "0")))
    }

    @Test fun theSplitStaysAvailableAsTheSpokenValue() {
        assertEquals("123 / 456 tokens", TeamsBotText.tokenSplit(usage(complete(123, 456, "1322")), words))
        assertEquals("3 / 4 tokens", TeamsBotText.tokenSplit(usage("""{"status":"partial","inputTokens":3,"outputTokens":4}"""), words))
    }

    @Test fun costHasTwoToFourDecimalsAndAPositiveCostBelowTheSmallestReadsLessThan() {
        assertEquals("1 tokens \u00b7 $0.0001", line(complete(1, 0, "100")))
        assertEquals("1 tokens \u00b7 <$0.0001", line(complete(1, 0, "99")))
        assertEquals("1 tokens \u00b7 <$0.0001", line(complete(1, 0, "1")))
        assertEquals("1 tokens \u00b7 $0.0013", line(complete(1, 0, "1322")))
        assertEquals("1 tokens \u00b7 $2.50", line(complete(1, 0, "2500000")))
        assertEquals("1 tokens \u00b7 0.0012 EUR", line(complete(1, 0, "1200", "EUR")))
    }

    @Test fun noCostMicrosSaysCostUnknown() {
        assertEquals("579 tokens \u00b7 cost unknown", line("""{"status":"complete","inputTokens":123,"outputTokens":456}"""))
        assertEquals("579 tokens \u00b7 cost unknown", line("""{"status":"complete","inputTokens":123,"outputTokens":456,"costMicros":null,"currency":"USD"}"""))
    }

    @Test fun costWithoutCurrencySaysCurrencyUnknownAfterTheAmount() {
        assertEquals("3 tokens \u00b7 2.50 currency unknown", line(complete(1, 2, "2500000", currency = null)))
        assertEquals("3 tokens \u00b7 <0.0001 currency unknown", line(complete(1, 2, "10", currency = null)))
        assertEquals("3 tokens \u00b7 2.50 currency unknown", line("""{"status":"complete","inputTokens":1,"outputTokens":2,"costMicros":2500000,"currency":" "}"""))
    }

    @Test fun theRealMoneyFormatKeepsTwoToFourDecimalsInTheCurrency() {
        assertEquals("$0.0012", TeamsBotText.money(BigDecimal("0.0012"), "USD", Locale.US))
        assertEquals("$2.50", TeamsBotText.money(BigDecimal("2.5"), "USD", Locale.US))
        assertEquals("$0.0013", TeamsBotText.money(BigDecimal("0.001322"), "USD", Locale.US))
        assertEquals("2.50", TeamsBotText.money(BigDecimal("2.5"), null, Locale.US))
        // A code the platform does not know is shown as sent after the bare number rather than dropped.
        assertEquals("1.00 ZZZ-bad", TeamsBotText.money(BigDecimal.ONE, "ZZZ-bad", Locale.US))
    }

    @Test fun everyRunStateHasALabelADetailAGlyphAndATone() {
        data class Row(val label: Int, val detail: Int?, val glyph: RunGlyph, val tone: RunTone, val active: Boolean)
        val table = mapOf(
            "queued" to Row(R.string.teams_run_queued, R.string.teams_run_queued_detail, RunGlyph.Clock, RunTone.Muted, true),
            "running" to Row(R.string.teams_run_processing, R.string.teams_run_processing_detail, RunGlyph.Spinner, RunTone.Muted, true),
            "completed" to Row(R.string.chat_status_completed, null, RunGlyph.Check, RunTone.Ok, false),
            "failed" to Row(R.string.chat_status_failed, R.string.teams_run_failed_detail, RunGlyph.Alert, RunTone.Danger, false),
            "expired" to Row(R.string.teams_run_expired, R.string.teams_run_expired_detail, RunGlyph.Expired, RunTone.Muted, false),
            "interrupted" to Row(R.string.teams_run_interrupted, R.string.teams_run_interrupted_detail, RunGlyph.Paused, RunTone.Muted, false),
            "authorization_required" to Row(R.string.teams_run_authorization_required, R.string.teams_run_authorization_required_detail, RunGlyph.Locked, RunTone.Muted, true),
            "revoked" to Row(R.string.teams_run_revoked, R.string.teams_run_revoked_detail, RunGlyph.Blocked, RunTone.Muted, false),
            "something-new" to Row(R.string.chat_status_unknown, null, RunGlyph.Unknown, RunTone.Muted, false),
        )
        for ((state, want) in table) {
            val got = TeamsBotText.runState(state)
            assertEquals(state, want, Row(got.label, got.detail, got.glyph, got.tone, got.active))
        }
    }

    @Test fun onlyCompletedIsOkOnlyFailedIsDangerAndOnlyTheOpenStatesOfferRefresh() {
        val states = listOf("queued", "running", "completed", "failed", "expired", "interrupted", "authorization_required", "revoked", "other")
        assertEquals(listOf("completed"), states.filter { TeamsBotText.runState(it).tone == RunTone.Ok })
        assertEquals(listOf("failed"), states.filter { TeamsBotText.runState(it).tone == RunTone.Danger })
        assertEquals(listOf("queued", "running", "authorization_required"), states.filter { TeamsBotText.runState(it).active })
        // Every state but Completed and the unknown one says what is going on or what to do in one sentence.
        assertEquals(listOf("completed", "other"), states.filter { TeamsBotText.runState(it).detail == null })
    }

    @Test fun runStatesNeverBorrowAnotherScreensWords() {
        val borrowed = setOf(R.string.cron_status_queued, R.string.teams_task_expired, R.string.teams_task_interrupted, R.string.teams_task_authorization_required)
        for (state in listOf("queued", "running", "completed", "failed", "expired", "interrupted", "authorization_required", "revoked", "other")) {
            assertTrue(state, TeamsBotText.runState(state).label !in borrowed)
        }
    }

    @Test fun taskParsesUsageWhenPresentAndToleratesItsAbsence() {
        val base = """{"taskId":"t","conversationId":"c","sharedBotId":"b","requester":{"uid":"u","generation":1},"state":"completed","createdAt":1,"deadline":2"""
        val with = TeamTask.parse(JSONObject(base + ""","usage":{"status":"partial","inputTokens":5,"outputTokens":6,"costMicros":null}}"""))
        assertEquals(TeamTaskUsage("partial", 5, 6, null, null), with.usage)
        assertNull(TeamTask.parse(JSONObject("$base}")).usage)
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
