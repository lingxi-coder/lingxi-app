package com.lingxi.code.cron

import android.annotation.SuppressLint
import android.content.Context
import androidx.test.platform.app.InstrumentationRegistry
import androidx.work.Configuration
import androidx.work.CoroutineWorker
import androidx.work.ListenableWorker
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.WorkerFactory
import androidx.work.WorkerParameters
import androidx.work.impl.WorkManagerImpl
import androidx.work.testing.SynchronousExecutor
import androidx.work.testing.WorkManagerTestInitHelper
import java.io.File
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Uses WorkManager's in-memory test database and an isolated history directory. */
@SuppressLint("RestrictedApi")
class CronBusyWorkManagerTest {
    @Test
    fun busyWakeDoesNotBlockUnrelatedSuccessorAndReusesThePersistedRunId() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(context.cacheDir, "cron-busy-work-${UUID.randomUUID()}")
        val history = CronRunHistoryStore(root)
        val scope = CronScope("test-a", "test-a", "Test A", root.path, "/workspace/test-a")
        val busy = history.enqueue(scope, "busy", "fixture", 1000L)!!
        val unrelated = history.enqueue(scope.copy(scopeId = "test-b", projectId = "test-b"), "other", "fixture", 1000L)!!
        val unrelatedFinished = CountDownLatch(1)
        val retryFinished = CountDownLatch(1)
        val attempts = ConcurrentHashMap<String, AtomicInteger>()
        val wakeIdentity = AtomicReference<String>()
        val failure = AtomicReference<Throwable>()
        val previous = WorkManagerImpl.getInstance(context)
        var testManager: WorkManager? = null
        try {
            val factory = object : WorkerFactory() {
                override fun createWorker(appContext: Context, workerClassName: String, workerParameters: WorkerParameters): ListenableWorker? {
                    if (workerClassName != CronExecutionWorker::class.java.name &&
                        workerClassName != CronBusyRetryWorker::class.java.name) return null
                    return object : CoroutineWorker(appContext, workerParameters) {
                        override suspend fun doWork(): Result = try {
                            val id = inputData.getString(CronWorkKeys.RUN_ID)!!
                            if (workerClassName == CronBusyRetryWorker::class.java.name) {
                                // Replace repository/provider access with the isolated store;
                                // use production wake lookup and execution enqueue unchanged.
                                wakeIdentity.set(id)
                                val record = busyCronRetryRecord(history, id, inputData.getInt(CronWorkKeys.BUSY_ATTEMPT, -1))!!
                                CronWorkScheduler.enqueueExecutionChain(appContext, listOf(record))?.let { operation ->
                                    withContext(Dispatchers.IO) { operation.result.get() }
                                }
                            } else {
                                val count = attempts.computeIfAbsent(id) { AtomicInteger() }.incrementAndGet()
                                history.markRunning(id, count)
                                if (id == busy.runId && count == 1) {
                                    // Exercise the real persisted park and independent delayed
                                    // WorkManager request, including waiting for its commit.
                                    parkBusyCronRun(history, id, 2, "busy: fixture conversation") {
                                        CronWorkScheduler.enqueueBusyRetry(appContext, it)
                                    }
                                } else {
                                    history.markTerminal(id, CronRunStatus.Succeeded, resultText = "fixture completed")
                                    if (id == unrelated.runId) unrelatedFinished.countDown() else retryFinished.countDown()
                                }
                            }
                            Result.success()
                        } catch (error: Throwable) {
                            failure.compareAndSet(null, error)
                            Result.failure()
                        }
                    }
                }
            }
            WorkManagerTestInitHelper.initializeTestWorkManager(context,
                Configuration.Builder().setExecutor(SynchronousExecutor()).setWorkerFactory(factory).build())
            val manager = WorkManager.getInstance(context)
            testManager = manager
            val driver = WorkManagerTestInitHelper.getTestDriver(context)!!
            CronWorkScheduler.enqueueExecutionChain(context, listOf(busy, unrelated))!!.result.get(10, TimeUnit.SECONDS)
            val initial = manager.getWorkInfosForUniqueWork(CronWorkNames.SERIAL_EXECUTION).get(10, TimeUnit.SECONDS)
            assertEquals(2, initial.size)
            initial.forEach { driver.setAllConstraintsMet(it.id) }
            assertTrue("Unrelated successor must finish while busy wake remains delayed: ${failure.get()}", unrelatedFinished.await(10, TimeUnit.SECONDS))
            eventually {
                manager.getWorkInfosForUniqueWork(CronWorkNames.SERIAL_EXECUTION).get().all { it.state == WorkInfo.State.SUCCEEDED }
            }
            val delayed = manager.getWorkInfosForUniqueWork(CronWorkNames.busyRetry(busy.runId, 2)).get(10, TimeUnit.SECONDS).single()
            assertEquals(WorkInfo.State.ENQUEUED, delayed.state)
            assertNull(wakeIdentity.get())
            assertEquals(1, attempts[busy.runId]?.get())
            assertEquals(CronRunStatus.Queued, CronRunHistoryStore(root).record(busy.runId)?.status)
            assertEquals(CronRunStatus.Succeeded, history.record(unrelated.runId)?.status)

            driver.setInitialDelayMet(delayed.id)
            eventually { manager.getWorkInfosForUniqueWork(CronWorkNames.SERIAL_EXECUTION).get().size == 3 }
            val retry = manager.getWorkInfosForUniqueWork(CronWorkNames.SERIAL_EXECUTION).get().single { candidate -> initial.none { it.id == candidate.id } }
            driver.setAllConstraintsMet(retry.id)
            assertTrue("Busy occurrence must run after its independent delay: ${failure.get()}", retryFinished.await(10, TimeUnit.SECONDS))
            assertEquals(busy.runId, wakeIdentity.get())
            assertEquals(2, attempts[busy.runId]?.get())
            assertNotNull(history.record(busy.runId))
            assertEquals(1000L, history.record(busy.runId)?.scheduledAtMs)
            assertEquals(CronRunStatus.Succeeded, history.record(busy.runId)?.status)
            failure.get()?.let { throw AssertionError("Probe worker failed", it) }
        } finally {
            testManager?.cancelAllWork()?.result?.get(10, TimeUnit.SECONDS)
            if (testManager != null) WorkManagerTestInitHelper.closeWorkDatabase()
            WorkManagerImpl.setDelegate(previous)
            root.deleteRecursively()
        }
    }

    private fun eventually(predicate: () -> Boolean) {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(10)
        while (!predicate()) {
            check(System.nanoTime() < deadline) { "Timed out waiting for WorkManager state" }
            Thread.sleep(20)
        }
    }
}
