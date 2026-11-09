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

    @Test fun onlySharedBotMessagesAreMarkdown() {
        assertTrue(TeamsBotText.rendersMarkdown("shared_bot", false))
        assertFalse(TeamsBotText.rendersMarkdown("user", false))
        assertFalse(TeamsBotText.rendersMarkdown("shared_bot", true))
    }

    @Test fun botLinksLeaveTheAppOnlyAsWebPages() {
        for (ok in listOf("https://example.com/a", "HTTP://example.com", "http://example.com:8080/x?y=1")) assertTrue(ok, TeamsBotText.allowsLink(ok))
        for (bad in listOf("javascript:alert(1)", "tel:123", "mailto:a@b.c", "file:///etc/passwd", "lingxi://open_terminal?initCommand=ls", "sms:1", "https:///nohost", "data:text/html,hi", "intent://x#Intent;end", "not a url", "")) assertFalse(bad, TeamsBotText.allowsLink(bad))
    }
}
