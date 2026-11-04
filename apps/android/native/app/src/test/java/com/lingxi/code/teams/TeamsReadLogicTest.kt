package com.lingxi.code.teams

import org.junit.Assert.assertEquals
import org.junit.Test

/** D12 / SPEC-mobile-team-bot-ui section 8.6: when the app marks a conversation read by itself. */
class TeamsReadLogicTest {
    @Test fun everyAutoMarkReadCaseMatchesTheVectors() {
        val cases = TeamsVectors.root.getJSONObject("mobile").getJSONArray("autoMarkRead").objects()
        assertEquals(10, cases.size)
        cases.forEach { case ->
            val input = case.getJSONObject("input")
            val got = TeamsReadLogic.autoMarkReadSeq(
                input.getBoolean("online"), input.getBoolean("fromCache"), input.getBoolean("foreground"),
                if (input.isNull("atBottomMs")) null else input.getLong("atBottomMs"), input.getLong("newestRenderedSeq"), input.getLong("lastReadSeq"),
                if (input.isNull("thresholdMs")) null else input.getLong("thresholdMs"),
            )
            assertEquals(case.getString("name"), if (case.isNull("expect")) null else case.getLong("expect"), got)
        }
    }

    @Test fun theShippedThresholdIsOneSecondAndTheDefaultArgumentUsesIt() {
        assertEquals(1000L, TeamsReadLogic.AUTO_MARK_READ_MS)
        assertEquals(12L, TeamsReadLogic.autoMarkReadSeq(online = true, fromCache = false, foreground = true, atBottomMs = 1000, newestRenderedSeq = 12, lastReadSeq = 9))
        assertEquals(null, TeamsReadLogic.autoMarkReadSeq(online = true, fromCache = false, foreground = true, atBottomMs = 999, newestRenderedSeq = 12, lastReadSeq = 9))
    }
}
