package com.lingxi.code.conversation

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class CompactionProgressTest {
    @Test fun hybridProgressFixture() {
        val fixture = RuntimeProtocolFixtures.snapshot("compaction_hybrid_progress.json")
        val cases = JSONObject(fixture.readText()).getJSONArray("cases")
        for (i in 0 until cases.length()) {
            val sample = cases.getJSONObject(i)
            val elapsed = sample.getLong("elapsed_ms")
            assertEquals("${elapsed}ms", if (sample.isNull("percent")) null else sample.getInt("percent"), compactProgressPercent(sample.getString("phase"), elapsed))
        }
    }

    @Test fun stagesPreserveTheSummaryStartAndStopOnTerminalEvents() {
        var status = reduceCompactionStatus(null, "preparing", null, 1_000)
        assertEquals(1_000L, status?.startedAtMillis)
        status = reduceCompactionStatus(status, "summarizing", null, 10_000)
        for (phase in listOf("preparing", "summarizing", "restoring")) {
            status = reduceCompactionStatus(status, phase, null, 100_000)
            assertEquals(1_000L, status?.startedAtMillis)
        }
        assertEquals("restoring", status?.phase)
        assertEquals(100_000L, status?.phaseStartedAtMillis)
        assertEquals(status, reduceCompactionStatus(status, "summarizing", null, 200_000))
        val unknown = reduceCompactionStatus(status, "future_phase", null, 210_000)
        assertTrue(unknown!!.unknownPhase)
        assertEquals(status, reduceCompactionStatus(unknown, "preparing", null, 220_000))
        val skipped = reduceCompactionStatus(status, "skipped", null)
        assertEquals(skipped, reduceCompactionStatus(skipped, "complete", null))
        val next = reduceCompactionStatus(skipped, "preparing", null, 230_000)
        assertEquals(230_000L, next?.phaseStartedAtMillis)
        assertEquals(0, compactProgressPercent(next!!.phase, 0))
        assertEquals(CompactionProgressStatus.Skipped, reduceCompactionStatus(status, "skipped", null)?.status)
        assertNull(compactProgressPercent("queued", 100_000))
        assertNull(reduceCompactionStatus(status, "cancelled", null))
        assertEquals(CompactionProgressStatus.Failed, reduceCompactionStatus(status, "error", "API failed")?.status)
        val completed = CompactionProgressUi(CompactionProgressStatus.Completed, messagesBefore = 42, messagesAfter = 8, bytesSaved = 2048)
        assertEquals(completed, reduceCompactionStatus(completed, "complete", null))
    }
}
