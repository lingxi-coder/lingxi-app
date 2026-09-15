package com.lingxi.code.cron

import com.lingxi.code.bindings.ReasoningSelectionDto
import org.json.JSONObject
import com.lingxi.code.model.persistenceKey
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class CronAutomationTest {
    @Test
    fun defaultsCaptureQualifiedModelAndAutomaticReasoning() {
        val config = CronAutomation.defaults("provider/model")
        assertEquals("provider/model", config.model)
        assertEquals("active", config.status)
        assertEquals("new_session", config.runMode)
        assertEquals("all", config.notificationPolicy)
        assertEquals("automatic", JSONObject(config.reasoningJson).getString("type"))
    }

    @Test
    fun reasoningKeepsProviderSpecificLevelAndTokenBudget() {
        val config = CronAutomation.defaults("provider/model")
            .withReasoning(ReasoningSelectionDto.Level("high"))
        assertEquals("high", JSONObject(config.reasoningJson).getString("id"))
        val budget = config.withReasoning(ReasoningSelectionDto.TokenBudget(8192uL))
        assertEquals(8192, JSONObject(budget.reasoningJson).getInt("tokens"))
    }

    @Test
    fun copyingDoesNotContinueTheOriginalSession() {
        val original = CronAutomation("""{"version":2,"status":"completed","runMode":"task_session","ownedSessionId":"old","targetSessionId":"selected","model":"provider/model","name":"Daily brief"}""")
        val copy = original.copied()
        assertEquals("active", copy.status)
        assertEquals("new_session", copy.runMode)
        assertEquals("Daily brief", copy.name)
        assertEquals("provider/model", copy.model)
        assertFalse(JSONObject(copy.json).has("ownedSessionId"))
        assertFalse(JSONObject(copy.json).has("targetSessionId"))
    }
    @Test
    fun failureNotificationsExcludeSuccessAndPauseCancellation() {
        org.junit.Assert.assertTrue(shouldNotifyCronRun("failed", CronRunStatus.Failed))
        org.junit.Assert.assertTrue(shouldNotifyCronRun("failed", CronRunStatus.Interrupted))
        assertFalse(shouldNotifyCronRun("failed", CronRunStatus.Succeeded))
        assertFalse(shouldNotifyCronRun("failed", CronRunStatus.Cancelled))
        assertFalse(shouldNotifyCronRun("none", CronRunStatus.Failed))
        assertFalse(shouldNotifyCronRun("all", CronRunStatus.Running))
    }

    @Test
    fun scheduledResultRoutesToManagedScopeInsteadOfGlobalRoot() {
        val scope = com.lingxi.code.model.ConversationScope.Scheduled
        assertEquals("scheduled", scope.persistenceKey())
        assertEquals(scope, com.lingxi.code.model.conversationScopeFromKey("scheduled"))
        org.junit.Assert.assertNotEquals(scope, com.lingxi.code.model.conversationScopeFromKey("global"))
    }

    @Test
    fun completedNativeRunRecoversEvenWhenTaskHasNoNextOccurrence() {
        val config = CronAutomation("""{"status":"completed","runs":[{"scheduledAt":1000,"finishedAt":1200,"status":"succeeded","model":"provider/actual","sessionId":"result-session","summary":"done"}]}""")
        val record = CronRunRecord("host-run", "task", "global", null, "None", "prompt", 1000, 1001, status = CronRunStatus.Running)
        val recovered = config.terminalFor(record)!!
        assertEquals(CronRunStatus.Succeeded, recovered.status)
        assertEquals("provider/actual", recovered.model)
        assertEquals("result-session", recovered.sessionId)
        assertEquals(1200L, recovered.finishedAtMs)
        org.junit.Assert.assertNull(config.terminalFor(record.copy(scheduledAtMs = 2000)))
        org.junit.Assert.assertNull(config.terminalFor(record.copy(manual = true, triggeredAtMs = 2000)))
    }

    @Test
    fun manualIdentityMatchesHostOccurrenceRatherThanLaterNativeTimestamp() {
        val config = CronAutomation("""{"runs":[
            {"scheduledAt":1100,"manualOccurrenceAt":1000,"status":"succeeded","sessionId":"correct"},
            {"scheduledAt":2100,"manualOccurrenceAt":2000,"status":"failed","sessionId":"other-manual"}
        ]}""")
        val record = CronRunRecord("host-run", "task", "global", null, "None", "prompt", 1000, 1000, manual = true)
        assertEquals("correct", config.terminalFor(record)?.sessionId)
        assertEquals("other-manual", config.terminalFor(record.copy(scheduledAtMs = 2000))?.sessionId)
        org.junit.Assert.assertNull(config.terminalFor(record.copy(scheduledAtMs = 3000)))
        org.junit.Assert.assertNull(config.terminalFor(record.copy(manual = false, scheduledAtMs = 1100)))
    }

    @Test
    fun legacyManualJournalFallbackIsOnlyUsedWithoutIdentityMarker() {
        val legacy = CronAutomation("""{"runs":[{"scheduledAt":1100,"status":"succeeded","sessionId":"legacy"}]}""")
        val record = CronRunRecord("host-run", "task", "global", null, "None", "prompt", 1000, 1000, manual = true)
        assertEquals("legacy", legacy.terminalFor(record)?.sessionId)
        val mismatched = CronAutomation("""{"runs":[{"scheduledAt":1100,"manualOccurrenceAt":999,"status":"succeeded","sessionId":"unrelated"}]}""")
        org.junit.Assert.assertNull(mismatched.terminalFor(record))
        val roundTrip = mismatched.change("name", "Edited task")
        assertEquals(999L, JSONObject(roundTrip.json).getJSONArray("runs").getJSONObject(0).getLong("manualOccurrenceAt"))
    }

    // 🚨 A manual run parked in Running does not block the task's own next
    // occurrence, so the journal can hold a SCHEDULED terminal entry stamped
    // after the manual run was triggered. The legacy time-only fallback matched
    // it and closed the manual run out with a session it never produced.
    @Test
    fun aScheduledJournalEntryNeverClosesOutAParkedManualRun() {
        val config = CronAutomation("""{"runs":[
            {"id":"task-1700000060","scheduledAt":1100,"status":"succeeded","sessionId":"scheduled-session","summary":"scheduled"}
        ]}""")
        val parked = CronRunRecord("host-run", "task", "global", null, "None", "prompt", 1000, 1000,
            status = CronRunStatus.Running, manual = true)
        org.junit.Assert.assertNull(
            "the scheduled occurrence's result must not be attributed to Run now",
            config.terminalFor(parked),
        )
        // …while the task's own scheduled record still recovers from it.
        val scheduled = parked.copy(manual = false, scheduledAtMs = 1100)
        assertEquals("scheduled-session", config.terminalFor(scheduled)?.sessionId)
        // And an id-stamped manual entry still matches its manual record, with no
        // `manualOccurrenceAt` marker to go on.
        val manualEntry = CronAutomation("""{"runs":[
            {"id":"task-manual-1700000000-ab","scheduledAt":1100,"status":"succeeded","sessionId":"manual-session"}
        ]}""")
        assertEquals("manual-session", manualEntry.terminalFor(parked)?.sessionId)
        org.junit.Assert.assertNull(manualEntry.terminalFor(scheduled))
    }

    @Test
    fun nativeReasoningFieldOrderStillSelectsTheSamePickerOption() {
        val native = CronAutomation("""{"reasoning":{"id":"high","type":"level"}}""")
        val picker = CronAutomation.defaults("provider/model").withReasoning(ReasoningSelectionDto.Level("high"))
        assertEquals(picker.reasoningJson, native.reasoningJson)
        assertEquals("High", native.reasoningLabel)
    }

}
