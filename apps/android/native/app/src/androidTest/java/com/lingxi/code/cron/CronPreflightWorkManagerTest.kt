package com.lingxi.code.cron

import android.annotation.SuppressLint
import android.content.Context
import android.content.ContextWrapper
import androidx.test.platform.app.InstrumentationRegistry
import androidx.work.Configuration
import androidx.work.CoroutineWorker
import androidx.work.Data
import androidx.work.ListenableWorker
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.WorkerFactory
import androidx.work.WorkerParameters
import androidx.work.impl.WorkManagerImpl
import androidx.work.testing.SynchronousExecutor
import androidx.work.testing.TestListenableWorkerBuilder
import androidx.work.testing.WorkManagerTestInitHelper
import com.lingxi.code.bindings.runtime.CronFireStatusDto
import com.lingxi.code.bindings.runtime.CronTaskDto
import com.lingxi.code.bindings.runtime.FiredCronJobDto
import java.io.File
import java.io.IOException
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import org.junit.Assert.*
import org.junit.Test

@SuppressLint("RestrictedApi")
class CronPreflightWorkManagerTest {
    @Test
    fun actualWorkerParksReadFailuresWithoutFailingSuccessorsOrReplayingStartedRuns() {
        val app = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(app.cacheDir, "cron-preflight-${UUID.randomUUID()}").apply { mkdirs() }
        val context = object : ContextWrapper(app) {
            override fun getApplicationContext(): Context = this
            override fun getFilesDir(): File = File(root, "files").apply { mkdirs() }
            override fun getCacheDir(): File = File(root, "cache").apply { mkdirs() }
        }
        val history = CronRunHistoryStore(File(root, "history"))
        val scope = CronScope.global(context)
        val queued = history.enqueue(scope, "read-error", "fixture", 1000, manual = true)!!
        val started = history.enqueue(scope, "started-error", "fixture", 1001, manual = true)!!
        history.markRunning(started.runId, 1)
        history.attachExecution(started.runId, notificationPolicy = "none")
        val other = history.enqueue(scope, "other", "fixture", 1002, manual = true)!!
        val cancelled = history.enqueue(scope, "cancelled", "fixture", 1003, manual = true)!!
        val tasks = listOf("read-error", "started-error", "other", "cancelled").map(::task)
        val calls = ConcurrentHashMap<String, AtomicInteger>()
        val acknowledgements = AtomicInteger()
        val goodGateway = FixtureGateway(tasks, calls, acknowledgements)
        val failedReads = ConcurrentHashMap<String, AtomicInteger>()
        lateinit var repository: AndroidCronRepository
        var ownedRepository: AndroidCronRepository? = null
        val previous = WorkManagerImpl.getInstance(app)
        var manager: WorkManager? = null
        try {
            val factory = object : WorkerFactory() {
                override fun createWorker(appContext: Context, workerClassName: String, workerParameters: WorkerParameters): ListenableWorker? {
                    val id = workerParameters.inputData.getString(CronWorkKeys.RUN_ID)
                    if (workerClassName == CronBusyRetryWorker::class.java.name) {
                        return object : CoroutineWorker(context, workerParameters) {
                            override suspend fun doWork(): Result {
                                val record = busyCronRetryRecord(history, id!!,
                                    inputData.getInt(CronWorkKeys.BUSY_ATTEMPT, -1),
                                    inputData.getBoolean(CronWorkKeys.RECOVER_RUNNING, false)) ?: return Result.success()
                                CronWorkScheduler.enqueueExecutionChain(context, listOf(record))?.let {
                                    withContext(Dispatchers.IO) { it.result.get() }
                                }
                                return Result.success()
                            }
                        }
                    }
                    if (workerClassName != CronExecutionWorker::class.java.name) return null
                    val taskId = history.record(id!!)?.taskId
                    val gateway = object : CronEngineGateway by goodGateway {
                        override suspend fun list(scope: CronScope): List<CronTaskDto> {
                            if (taskId == "cancelled") throw CancellationException("fixture preflight cancellation")
                            if (taskId in setOf("read-error", "started-error") &&
                                failedReads.computeIfAbsent(taskId!!) { AtomicInteger() }.incrementAndGet() == 1) {
                                throw IOException("fixture temporary task read failure")
                            }
                            return goodGateway.list(scope)
                        }
                    }
                    // Exercise the actual production worker; only storage/model
                    // boundaries are fixtures, never external providers or keys.
                    return CronExecutionWorker(context, workerParameters, repository, gateway,
                        reconcileSchedules = { repository.refreshNow() })
                }
            }
            WorkManagerTestInitHelper.initializeTestWorkManager(context,
                Configuration.Builder().setExecutor(SynchronousExecutor()).setWorkerFactory(factory).build())
            val workManager = WorkManager.getInstance(context)
            manager = workManager
            repository = AndroidCronRepository.create(context, gateway = goodGateway, historyStore = history)
            ownedRepository = repository
            val driver = WorkManagerTestInitHelper.getTestDriver(context)!!
            CronWorkScheduler.enqueueExecutionChain(context, listOf(queued, started, other))!!.result.get(10, TimeUnit.SECONDS)
            val originals = workManager.getWorkInfosForUniqueWork(CronWorkNames.SERIAL_EXECUTION).get()
            originals.forEach { driver.setAllConstraintsMet(it.id) }
            eventually { workManager.getWorkInfosForUniqueWork(CronWorkNames.SERIAL_EXECUTION).get().all { it.state == WorkInfo.State.SUCCEEDED } }
            assertEquals(CronRunStatus.Queued, history.record(queued.runId)?.status)
            assertNull(history.record(queued.runId)?.startedAtMs)
            assertEquals(CronRunStatus.Running, history.record(started.runId)?.status)
            assertEquals(CronRunStatus.Succeeded, history.record(other.runId)?.status)
            assertEquals(0, calls[queued.taskId]?.get() ?: 0)
            assertEquals(0, calls[started.taskId]?.get() ?: 0)
            assertEquals(0, acknowledgements.get())

            for (record in listOf(queued, started)) {
                val pending = history.record(record.runId)!!
                val wake = workManager.getWorkInfosForUniqueWork(CronWorkNames.busyRetry(record.runId, pending.attempt)).get().single()
                assertEquals(WorkInfo.State.ENQUEUED, wake.state)
                driver.setInitialDelayMet(wake.id)
            }
            eventually { workManager.getWorkInfosForUniqueWork(CronWorkNames.SERIAL_EXECUTION).get().size == 5 }
            workManager.getWorkInfosForUniqueWork(CronWorkNames.SERIAL_EXECUTION).get()
                .filter { candidate -> originals.none { it.id == candidate.id } }
                .forEach { driver.setAllConstraintsMet(it.id) }
            eventually { history.record(queued.runId)?.status == CronRunStatus.Succeeded && history.record(started.runId)?.status == CronRunStatus.Interrupted }
            assertEquals(1, calls[queued.taskId]?.get())
            assertEquals(0, calls[started.taskId]?.get() ?: 0)
            assertEquals(0, acknowledgements.get())

            val worker = TestListenableWorkerBuilder<CronExecutionWorker>(context)
                .setInputData(Data.Builder().putString(CronWorkKeys.RUN_ID, cancelled.runId).build())
                .setWorkerFactory(factory).build()
            val cancellation = runBlocking { runCatching { worker.doWork() }.exceptionOrNull() }
            assertTrue(cancellation is CancellationException)
            assertEquals(CronRunStatus.Queued, history.record(cancelled.runId)?.status)
            assertNull(history.record(cancelled.runId)?.startedAtMs)
        } finally {
            ownedRepository?.dispose()
            manager?.cancelAllWork()?.result?.get(10, TimeUnit.SECONDS)
            if (manager != null) WorkManagerTestInitHelper.closeWorkDatabase()
            WorkManagerImpl.setDelegate(previous)
            root.deleteRecursively()
        }
    }

    private fun task(id: String) = CronTaskDto(
        CronAutomation.defaults("fixture/model").change("notificationPolicy", "none").json,
        id, "0 9 * * *", "fixture", 1uL, null, true, null, "Daily", true, null,
    )

    private class FixtureGateway(
        val tasks: List<CronTaskDto>,
        val calls: ConcurrentHashMap<String, AtomicInteger>,
        val acknowledgements: AtomicInteger,
    ) : CronEngineGateway {
        override suspend fun list(scope: CronScope) = tasks
        override suspend fun dueOccurrences(scope: CronScope, nowMs: Long) = emptyList<Pair<String, Long>>()
        override suspend fun runTaskNow(scope: CronScope, taskId: String, scheduledAtMs: Long): CronTaskExecutionBatch {
            calls.computeIfAbsent(taskId) { AtomicInteger() }.incrementAndGet()
            return CronTaskExecutionBatch(listOf(FiredCronJobDto(null, taskId, "fixture", "done", CronFireStatusDto.Ok, false)))
        }
        override suspend fun runTaskIfDue(scope: CronScope, taskId: String, scheduledAtMs: Long) = error("No scheduled execution in fixture")
        override suspend fun acknowledgeOccurrence(scope: CronScope, taskId: String, scheduledAtMs: Long): Boolean { acknowledgements.incrementAndGet(); return true }
        override suspend fun create(scope: CronScope, cron: String, prompt: String, recurring: Boolean, automation: CronAutomation): CronTaskDto = error("No mutation in fixture")
        override suspend fun update(scope: CronScope, taskId: String, cron: String, prompt: String, recurring: Boolean, automation: CronAutomation): CronTaskDto = error("No mutation in fixture")
        override suspend fun delete(scope: CronScope, taskId: String): Boolean = error("No mutation in fixture")
    }

    private fun eventually(predicate: () -> Boolean) {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(10)
        while (!predicate()) {
            check(System.nanoTime() < deadline) { "Timed out waiting for WorkManager state" }
            Thread.sleep(20)
        }
    }
}
