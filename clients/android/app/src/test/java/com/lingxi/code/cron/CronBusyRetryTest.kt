package com.lingxi.code.cron

import java.io.File
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runTest
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test

class CronBusyRetryTest {
    private lateinit var root: File
    private lateinit var history: CronRunHistoryStore
    private lateinit var scope: CronScope
    private var sequence = 0

    @Before
    fun setUp() {
        root = java.nio.file.Files.createTempDirectory("cron-busy-").toFile()
        history = CronRunHistoryStore(root, now = { 2000L }, newId = { "run-${sequence++}" })
        scope = CronScope("project-a", "project-a", "Project A", root.path, "/workspace/a")
    }

    @After
    fun tearDown() { root.deleteRecursively() }

    @Test
    fun busyHandoffWaitsForDurableWakeAndPreservesOccurrenceWhileOtherTasksRun() = runTest {
        val busy = history.enqueue(scope, "busy-task", "prompt", 1000L)!!
        history.markRunning(busy.runId, 1)
        val other = history.enqueue(scope.copy(scopeId = "project-b", projectId = "project-b"), "other-task", "other", 1000L)!!
        val enqueued = CompletableDeferred<CronRunRecord>()
        val committed = CompletableDeferred<Unit>()
        val handoff = launch {
            parkBusyCronRun(history, busy.runId, 2, "busy: Target conversation is active") {
                enqueued.complete(it)
                committed.await()
            }
        }
        val wake = enqueued.await()
        assertFalse("Execution must not release its chain before durable handoff", handoff.isCompleted)
        assertEquals(busy.runId, wake.runId)
        assertEquals(busy.scheduledAtMs, wake.scheduledAtMs)
        committed.complete(Unit)
        handoff.join()
        // The returned execution can now finish its chain node. The next task
        // runs while the busy occurrence stays parked in its independent wake.
        history.markRunning(other.runId, 1)
        assertEquals(CronRunStatus.Running, history.record(other.runId)?.status)
        val reloaded = CronRunHistoryStore(root)
        assertEquals(busy.runId, busyCronRetryRecord(reloaded, busy.runId, 2)?.runId)
        assertEquals(CronRunStatus.Queued, reloaded.record(busy.runId)?.status)
    }

    @Test
    fun failedHandoffRemainsQueuedAndCannotBeReportedAsSuccessful() = runTest {
        val run = history.enqueue(scope, "task", "prompt", 1000L)!!
        history.markRunning(run.runId, 1)
        val failure = runCatching {
            parkBusyCronRun(history, run.runId, 2, "busy: Target is active") { error("WorkManager unavailable") }
        }.exceptionOrNull()
        assertEquals("WorkManager unavailable", failure?.message)
        assertEquals(CronRunStatus.Queued, history.record(run.runId)?.status)
    }

    @Test
    fun busyCoalescesLaterPendingOccurrenceWithoutChangingItsOwnId() = runTest {
        val first = history.enqueue(scope, "task", "prompt", 1000L)!!
        history.markRunning(first.runId, 1)
        val newer = history.enqueue(scope, "task", "prompt", 2000L)!!
        parkBusyCronRun(history, first.runId, 2, "busy: Target is active") { }
        assertEquals(listOf(first.runId), history.records().filterNot { it.status.isTerminal }.map { it.runId })
        assertEquals(CronRunStatus.Cancelled, history.record(newer.runId)?.status)
        assertNull(history.enqueue(scope, "task", "prompt", 3000L))
    }

    @Test
    fun freshWorkerTransientRetryCannotRewindPersistedBusyWakeGeneration() = runTest {
        val run = history.enqueue(scope, "task", "prompt", 1000L)!!
        parkBusyCronRun(history, run.runId, 8, "busy: Target is active") { }
        assertNotNull(busyCronRetryRecord(history, run.runId, 8))
        history.markRunning(run.runId, 9)
        // A new WorkManager job reports attempt zero even after earlier busy
        // wakes. Its transport retry must preserve the persistent generation.
        history.markRetry(run.runId, 1, "temporary network error")
        assertEquals(9, history.record(run.runId)?.attempt)
        assertNull(busyCronRetryRecord(history, run.runId, 8))
        history.markRunning(run.runId, 1)
        assertEquals(9, history.record(run.runId)?.attempt)
        history.markRetry(run.runId, 1, "temporary network error")
        val reloaded = CronRunHistoryStore(root)
        assertEquals(9, reloaded.record(run.runId)?.attempt)
        assertNull(busyCronRetryRecord(reloaded, run.runId, 8))
    }

    @Test
    fun recoveryWakeRetainsStartedStateAndCannotMasqueradeAsRunnableBusyRetry() {
        val run = history.enqueue(scope, "started-task", "prompt", 1000L)!!
        history.markRunning(run.runId, 3)
        assertNull(busyCronRetryRecord(history, run.runId, 3))
        assertEquals(CronRunStatus.Running, busyCronRetryRecord(history, run.runId, 3, recoverRunning = true)?.status)
        assertNull(busyCronRetryRecord(history, run.runId, 2, recoverRunning = true))
        history.markTerminal(run.runId, CronRunStatus.Interrupted)
        assertNull(busyCronRetryRecord(history, run.runId, 3, recoverRunning = true))
    }

    @Test
    fun stalePausedAndDeletedWakesDoNotRequeue() = runTest {
        val run = history.enqueue(scope, "task", "prompt", 1000L)!!
        parkBusyCronRun(history, run.runId, 2, "busy: Target is active") { }
        assertNull(busyCronRetryRecord(history, run.runId, 1))
        history.cancelQueued(scope.scopeId, run.taskId)
        assertNull(busyCronRetryRecord(history, run.runId, 2))
        val deleted = history.enqueue(scope, "deleted-task", "prompt", 2000L)!!
        parkBusyCronRun(history, deleted.runId, 2, "busy: Target is active") { }
        history.cancelUnfinished(scope.scopeId, deleted.taskId, "Task deleted")
        assertNull(busyCronRetryRecord(history, deleted.runId, 2))
    }
}
