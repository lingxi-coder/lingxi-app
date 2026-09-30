package com.lingxi.code.cron

import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.util.UUID
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

class CronRunHistoryStoreInstrumentedTest {
    private lateinit var root: File
    private lateinit var store: CronRunHistoryStore
    private var clock = 1_000L
    private var sequence = 0

    @Before
    fun setUp() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        root = File(context.cacheDir, "cron-history-${UUID.randomUUID()}")
        store = CronRunHistoryStore(
            historyRoot = root,
            now = { clock++ },
            newId = { "run-${sequence++}" },
        )
    }

    @After
    fun tearDown() {
        root.deleteRecursively()
    }

    @Test
    fun unfinishedOccurrenceDeduplicatesAndTerminalRunAllowsNextOccurrence() {
        val scope = CronScope("global", null, "全局", root.path, "/workspace/global")
        val first = store.enqueue(scope, "task", "prompt", 900L)

        assertNotNull(first)
        assertNull(store.enqueue(scope, "task", "prompt", 900L))

        store.markTerminal(first!!.runId, CronRunStatus.Succeeded, resultText = "ok")
        assertNotNull(store.enqueue(scope, "task", "prompt", 1_800L))
    }

    @Test
    fun atomicIndexRoundTripsAndBoundsResultBytes() {
        val scope = CronScope("global", null, "全局", root.path, "/workspace/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markTerminal(
            run.runId,
            CronRunStatus.Succeeded,
            resultText = "🙂".repeat(MAX_CRON_RESULT_BYTES),
        )

        val reloaded = CronRunHistoryStore(root).record(run.runId)

        assertNotNull(reloaded)
        assertTrue(reloaded!!.resultText!!.toByteArray(Charsets.UTF_8).size <= MAX_CRON_RESULT_BYTES)
        assertEquals(CronRunStatus.Succeeded, reloaded.status)
    }

    @Test
    fun checksumFailureRecoversLastGoodBackupAndQuarantinesPrimary() {
        val scope = CronScope("global", null, "全局", root.path, "/workspace/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markTerminal(run.runId, CronRunStatus.Succeeded, resultText = "durable")
        File(root, "index.json").writeText(
            """{"version":2,"payload":"{}","sha256":"tampered"}""",
            Charsets.UTF_8,
        )

        val recovered = CronRunHistoryStore(root).record(run.runId)

        assertEquals("durable", recovered?.resultText)
        assertTrue(
            root.listFiles().orEmpty().any { it.name.startsWith("index.corrupt.") },
        )
        assertEquals(
            "durable",
            CronRunHistoryStore(root).record(run.runId)?.resultText,
        )
    }
    @Test
    fun executionIdentityAndNotificationClaimSurviveReload() {
        val scope = CronScope("global", null, "None", root.path, "/workspace/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.attachExecution(run.runId, "session-1", "provider/model", "failed")
        store.markTerminal(run.runId, CronRunStatus.Failed, errorMessage = "failed")
        assertTrue(store.claimNotification(run.runId))
        val reloaded = CronRunHistoryStore(root)
        org.junit.Assert.assertFalse(reloaded.claimNotification(run.runId))
        assertEquals("session-1", reloaded.record(run.runId)?.sessionId)
        assertEquals("provider/model", reloaded.record(run.runId)?.model)
        assertEquals("failed", reloaded.record(run.runId)?.notificationPolicy)
    }

    @Test
    fun pauseCancelsQueuedButPreservesRunning() {
        val scope = CronScope("global", null, "None", root.path, "/workspace/global")
        val queued = store.enqueue(scope, "queued-task", "prompt", 900L)!!
        val running = store.enqueue(scope, "running-task", "prompt", 900L)!!
        store.markRunning(running.runId, 1)
        store.cancelQueued(scope.scopeId, "queued-task")
        store.cancelQueued(scope.scopeId, "running-task")
        assertEquals(CronRunStatus.Cancelled, store.record(queued.runId)?.status)
        assertEquals(CronRunStatus.Running, store.record(running.runId)?.status)
    }

    @Test
    fun automationEditsPreserveUnknownMetadataAndCopyResetsSession() {
        val original = CronAutomation("""{"version":2,"futureField":"keep","status":"completed","runMode":"task_session","ownedSessionId":"old"}""")
        val copy = original.copied().change("model", "provider/model")
        assertEquals("keep", org.json.JSONObject(copy.json).getString("futureField"))
        assertEquals("active", copy.status)
        assertEquals("new_session", copy.runMode)
        org.junit.Assert.assertFalse(org.json.JSONObject(copy.json).has("ownedSessionId"))
    }

    @Test
    fun runningTaskCoalescesOnlyOnePendingOccurrence() {
        val scope = CronScope("global", null, "None", root.path, "/workspace/global")
        val running = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markRunning(running.runId, 1)
        assertNotNull(store.enqueue(scope, "task", "prompt", 1_800L))
        assertNull(store.enqueue(scope, "task", "prompt", 2_700L))
        assertEquals(2, store.records().count { !it.status.isTerminal })
    }

    @Test
    fun interruptedOccurrenceCannotBeDispatchedAgain() {
        val scope = CronScope("global", null, "None", root.path, "/workspace/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markTerminal(run.runId, CronRunStatus.Interrupted, errorMessage = "process interrupted")
        assertNull(store.enqueue(scope, "task", "prompt", 900L))
        assertNotNull(store.enqueue(scope, "task", "prompt", 1_800L))
    }

}
