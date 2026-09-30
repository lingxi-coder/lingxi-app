package com.lingxi.code.cron

import java.io.File
import java.util.UUID
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

class CronRunHistoryPersistenceTest {
    private lateinit var root: File
    private lateinit var store: CronRunHistoryStore
    private var clock = 1_000L
    private var sequence = 0

    @Before
    fun setUp() {
        root = java.nio.file.Files.createTempDirectory("cron-history-").toFile()
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
    fun restartRecoversSavedTerminalNotificationsOnceUsingPersistedPolicy() {
        val scope = CronScope("global", null, "None", root.path, "/workspace/global")
        fun terminal(task: String, policy: String, status: CronRunStatus): String {
            val run = store.enqueue(scope, task, "saved prompt", 900L)!!
            store.attachExecution(run.runId, notificationPolicy = policy)
            store.markTerminal(run.runId, status, resultText = "saved result")
            return run.runId
        }
        val success = terminal("all", "all", CronRunStatus.Succeeded)
        val failure = terminal("failed", "failed", CronRunStatus.Failed)
        terminal("off", "none", CronRunStatus.Failed)
        terminal("success-filtered", "failed", CronRunStatus.Succeeded)
        val queued = store.enqueue(scope, "queued", "prompt", 900L)!!
        val running = store.enqueue(scope, "running", "prompt", 900L)!!
        store.markRunning(running.runId, 1)

        val posted = mutableListOf<CronRunRecord>()
        // No task or Engine is required to deliver the already persisted results.
        CronRunHistoryStore(root).postPendingNotifications(post = posted::add)
        CronRunHistoryStore(root).postPendingNotifications(post = posted::add)

        assertEquals(setOf(success, failure), posted.map { it.runId }.toSet())
        assertEquals(2, posted.size)
        assertTrue(posted.all { it.resultText == "saved result" })
        assertEquals(CronRunStatus.Queued, store.record(queued.runId)?.status)
        assertEquals(CronRunStatus.Running, store.record(running.runId)?.status)
    }

    @Test
    fun failedNotificationClaimWriteRemainsRecoverable() {
        val scope = CronScope("global", null, "None", root.path, "/workspace/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markTerminal(run.runId, CronRunStatus.Succeeded)
        // Block the write the CLAIM depends on, which lives in index.json.
        //
        // Two ways of blocking it that look right do not work. Replacing
        // sessions.json with a directory fails a write `writeLocked` swallows
        // ON PURPOSE — the sidecar is derived, and its failure must not strand
        // a run as Running forever — so the claim still lands and the
        // notification still posts. Replacing index.json with a directory is
        // undone by `readLocked`, which quarantines a primary it cannot read
        // and then writes a fresh one. Making the history root unwritable is
        // what actually fails the claim write while leaving index.json
        // readable, so the record can still be inspected below.
        var posts = 0
        try {
            assertTrue(root.setWritable(false, false))
            assertTrue(runCatching { store.postPendingNotifications { posts++ } }.isFailure)
            assertEquals(0, posts)
            org.junit.Assert.assertFalse(store.record(run.runId)!!.notificationDelivered)
        } finally {
            assertTrue(root.setWritable(true, true))
        }
        CronRunHistoryStore(root).postPendingNotifications { posts++ }
        CronRunHistoryStore(root).postPendingNotifications { posts++ }
        assertEquals(1, posts)
    }

    @Test
    fun workerTerminalRedeliveryAndRecoveryShareDurableClaim() {
        val scope = CronScope("global", null, "None", root.path, "/workspace/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markTerminal(run.runId, CronRunStatus.Succeeded)
        var posts = 0
        store.postPendingNotifications(run.runId) { posts++ }
        CronRunHistoryStore(root).postPendingNotifications { posts++ }
        assertEquals(1, posts)
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

    @Test
    fun generatedSessionIndexOutlivesRunRetention() {
        val scope = CronScope("global", null, "None", root.path, "/workspace/global")
        repeat(25) { index ->
            val run = store.enqueue(scope, "task", "prompt", index.toLong())!!
            store.attachExecution(run.runId, sessionId = "session-$index")
            store.markTerminal(run.runId, CronRunStatus.Succeeded)
        }
        val reloaded = CronRunHistoryStore(root)
        assertEquals(20, reloaded.records().size)
        assertEquals(25, reloaded.generatedSessions().size)
        assertTrue(reloaded.generatedSessions().any { it.sessionId == "session-0" })
    }

}
