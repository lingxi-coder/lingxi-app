package com.lingxi.code.cron

import android.content.Context
import android.util.Log
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.Data
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequest
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import com.lingxi.code.bindings.CronFireStatusDto
import com.lingxi.code.bindings.FiredCronJobDto
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout

internal object CronWorkKeys {
    const val SCHEDULED_AT_MS = "scheduled_at_ms"
    const val RUN_ID = "run_id"
    const val SCOPE_ID = "scope_id"
    const val TASK_ID = "task_id"
    const val REASON = "reason"
}

internal object CronWorkNames {
    const val WATCHDOG = "cron-watchdog-15m"
    const val RECONCILE = "cron-reconcile"
    const val SERIAL_EXECUTION = "cron-global-serial-execution"
    const val TAG_EXECUTION = "cron-execution"

    fun dispatch(scheduledAtMs: Long): String = "cron-dispatch-$scheduledAtMs"
    fun occurrence(scopeId: String, taskId: String, scheduledAtMs: Long): String =
        "cron-occurrence-$scopeId-$taskId-$scheduledAtMs"
}

internal object CronWorkScheduler {
    private const val WATCHDOG_MINUTES = 15L

    fun ensureWatchdog(context: Context) {
        val request = PeriodicWorkRequestBuilder<CronDispatchWorker>(
            WATCHDOG_MINUTES,
            TimeUnit.MINUTES,
        )
            .setInputData(
                Data.Builder()
                    .putString(CronWorkKeys.REASON, "watchdog")
                    .build(),
            )
            .addTag(CronWorkNames.WATCHDOG)
            .build()
        WorkManager.getInstance(context.applicationContext).enqueueUniquePeriodicWork(
            CronWorkNames.WATCHDOG,
            ExistingPeriodicWorkPolicy.KEEP,
            request,
        )
    }

    fun enqueueDispatch(context: Context, scheduledAtMs: Long, expedited: Boolean) {
        val builder = OneTimeWorkRequestBuilder<CronDispatchWorker>()
            .setInputData(
                Data.Builder()
                    .putLong(CronWorkKeys.SCHEDULED_AT_MS, scheduledAtMs)
                    .putString(CronWorkKeys.REASON, if (expedited) "exact-alarm" else "reconcile")
                    .build(),
            )
            .addTag(CronWorkNames.dispatch(scheduledAtMs))
        if (expedited) {
            builder.setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
        }
        WorkManager.getInstance(context.applicationContext).enqueueUniqueWork(
            CronWorkNames.dispatch(scheduledAtMs),
            ExistingWorkPolicy.KEEP,
            builder.build(),
        )
    }

    fun enqueueReconcile(context: Context, reason: String) {
        val request = OneTimeWorkRequestBuilder<CronReconcileWorker>()
            .setInputData(Data.Builder().putString(CronWorkKeys.REASON, reason).build())
            .addTag(CronWorkNames.RECONCILE)
            .build()
        WorkManager.getInstance(context.applicationContext).enqueueUniqueWork(
            CronWorkNames.RECONCILE,
            ExistingWorkPolicy.KEEP,
            request,
        )
    }

    fun enqueueExecutionChain(context: Context, records: List<CronRunRecord>) {
        if (records.isEmpty()) return
        val requests = records.map(::executionRequest)
        var continuation = WorkManager.getInstance(context.applicationContext).beginUniqueWork(
            CronWorkNames.SERIAL_EXECUTION,
            ExistingWorkPolicy.APPEND_OR_REPLACE,
            requests.first(),
        )
        for (request in requests.drop(1)) {
            continuation = continuation.then(request)
        }
        continuation.enqueue()
    }

    private fun executionRequest(record: CronRunRecord): OneTimeWorkRequest =
        OneTimeWorkRequestBuilder<CronExecutionWorker>()
            .setConstraints(
                Constraints.Builder()
                    .setRequiredNetworkType(NetworkType.CONNECTED)
                    .build(),
            )
            .setInputData(
                Data.Builder()
                    .putString(CronWorkKeys.RUN_ID, record.runId)
                    .putString(CronWorkKeys.SCOPE_ID, record.scopeId)
                    .putString(CronWorkKeys.TASK_ID, record.taskId)
                    .putLong(CronWorkKeys.SCHEDULED_AT_MS, record.scheduledAtMs)
                    .build(),
            )
            .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30, TimeUnit.SECONDS)
            .addTag(CronWorkNames.TAG_EXECUTION)
            .addTag(
                CronWorkNames.occurrence(
                    record.scopeId,
                    record.taskId,
                    record.scheduledAtMs,
                ),
            )
            .build()
}

class CronDispatchWorker(
    appContext: Context,
    params: WorkerParameters,
) : CoroutineWorker(appContext, params) {
    override suspend fun doWork(): Result {
        val repository = AndroidCronRepository.get(applicationContext)
        val now = System.currentTimeMillis()
        return try {
            val due = repository.loadDueOccurrences(now)
                .sortedBy(ScopedCronOccurrence::scheduledAtMs)
            val claimed = due.mapNotNull { occurrence ->
                repository.historyStore.enqueue(
                    scope = occurrence.scope,
                    taskId = occurrence.task.id,
                    prompt = occurrence.task.prompt,
                    scheduledAtMs = occurrence.scheduledAtMs,
                    triggeredAtMs = now,
                )
            }
            CronWorkScheduler.enqueueExecutionChain(applicationContext, claimed)
            CronCoordinator(applicationContext, repository).reconcile(
                inputData.getString(CronWorkKeys.REASON) ?: "dispatch",
            )
            Result.success()
        } catch (cancel: CancellationException) {
            throw cancel
        } catch (error: Throwable) {
            Log.w(TAG, "cron dispatch failed", error)
            Result.retry()
        }
    }

    companion object {
        private const val TAG = "CronDispatchWorker"
    }
}

class CronReconcileWorker(
    appContext: Context,
    params: WorkerParameters,
) : CoroutineWorker(appContext, params) {
    override suspend fun doWork(): Result {
        return try {
            val reason = inputData.getString(CronWorkKeys.REASON) ?: "system"
            CronCoordinator(applicationContext).reconcile(reason)
            CronWorkScheduler.enqueueDispatch(
                applicationContext,
                System.currentTimeMillis(),
                expedited = false,
            )
            Result.success()
        } catch (cancel: CancellationException) {
            throw cancel
        } catch (error: Throwable) {
            Log.w(TAG, "cron reconcile worker failed", error)
            Result.retry()
        }
    }

    companion object {
        private const val TAG = "CronReconcileWorker"
    }
}

class CronExecutionWorker(
    appContext: Context,
    params: WorkerParameters,
) : CoroutineWorker(appContext, params) {
    private val repository = AndroidCronRepository.get(appContext)
    private val history = repository.historyStore
    private val gateway: CronEngineGateway = MobileCronEngineGateway(appContext)
    private val runId = params.inputData.getString(CronWorkKeys.RUN_ID)

    override suspend fun doWork(): Result {
        val id = runId ?: return Result.success()
        val existing = history.record(id) ?: return Result.success()
        if (existing.status.isTerminal) return Result.success()
        val scope = repository.scope(existing.scopeId)
            ?: return terminalFailure(existing, "Project 工作区不存在或已失效")
        history.markRunning(id, runAttemptCount + 1)
        repository.refresh()
        return try {
            val batch = withTimeout(AGENT_RUN_BUDGET_MS) {
                if (existing.manual) {
                    gateway.runTaskNow(scope, existing.taskId)
                } else {
                    gateway.runTaskIfDue(scope, existing.taskId, existing.scheduledAtMs)
                }
            }
            applyBatch(scope, existing, batch.firedJobs)
        } catch (timeout: TimeoutCancellationException) {
            terminalFailure(
                existing,
                "执行超过 3 分钟",
                status = CronRunStatus.TimedOut,
            )
        } catch (cancel: CancellationException) {
            history.markRetry(id, runAttemptCount + 1, "系统中断，等待重新调度")
            repository.refresh()
            throw cancel
        } catch (error: Throwable) {
            if (error is CronPermanentExecutionException || !isRetryableCronFailure(error.message)) {
                terminalFailure(existing, error.message ?: "定时任务执行失败")
            } else if (runAttemptCount + 1 < MAX_EXECUTION_ATTEMPTS) {
                history.markRetry(
                    id,
                    runAttemptCount + 1,
                    error.message ?: "临时网络或服务错误",
                )
                repository.refresh()
                Result.retry()
            } else {
                terminalFailure(existing, error.message ?: "重试次数已耗尽")
            }
        } finally {
            withContext(NonCancellable) {
                try {
                    withTimeout(RECONCILE_BUDGET_MS) {
                        CronCoordinator(applicationContext, repository)
                            .reconcile("execution-finished")
                    }
                } catch (error: Throwable) {
                    // Do not replay a completed occurrence merely because the
                    // follow-up alarm refresh failed. The watchdog and system
                    // reconciliation receiver repair scheduling.
                    Log.w(TAG, "post-execution cron reconcile failed", error)
                }
            }
        }
    }

    private suspend fun applyBatch(
        scope: CronScope,
        requested: CronRunRecord,
        firedJobs: List<FiredCronJobDto>,
    ): Result {
        var requestedResult: Result? = null
        for (fired in firedJobs) {
            val queued = history.unfinished(scope.scopeId, fired.id) ?: continue
            when (val status = fired.status) {
                CronFireStatusDto.Ok -> {
                    history.markTerminal(
                        queued.runId,
                        CronRunStatus.Succeeded,
                        resultText = fired.resultText?.ifBlank { null } ?: "已完成",
                    )
                    postTerminal(history.record(queued.runId))
                    if (queued.runId == requested.runId) requestedResult = Result.success()
                }
                is CronFireStatusDto.Failed -> {
                    if (fired.retryable &&
                        runAttemptCount + 1 < MAX_EXECUTION_ATTEMPTS
                    ) {
                        history.markRetry(queued.runId, runAttemptCount + 1, status.message)
                        if (queued.runId == requested.runId) requestedResult = Result.retry()
                    } else {
                        if (fired.retryable && !queued.manual) {
                            runCatching {
                                gateway.acknowledgeOccurrence(
                                    scope,
                                    queued.taskId,
                                    queued.scheduledAtMs,
                                )
                            }
                        }
                        history.markTerminal(
                            queued.runId,
                            CronRunStatus.Failed,
                            errorMessage = status.message,
                        )
                        postTerminal(history.record(queued.runId))
                        if (queued.runId == requested.runId) requestedResult = Result.success()
                    }
                }
            }
        }
        val finalRequested = history.record(requested.runId)
        if (requestedResult == null && finalRequested?.status?.isTerminal != true) {
            history.markTerminal(
                requested.runId,
                CronRunStatus.Skipped,
                errorMessage = "任务已删除、尚未到期或已由同作用域批次领取",
            )
            postTerminal(history.record(requested.runId))
            requestedResult = Result.success()
        }
        repository.refresh()
        return requestedResult ?: Result.success()
    }

    private suspend fun terminalFailure(
        record: CronRunRecord,
        message: String,
        status: CronRunStatus = CronRunStatus.Failed,
    ): Result {
        if (!record.manual) {
            val scope = repository.scope(record.scopeId)
            if (scope != null) {
                runCatching {
                    gateway.acknowledgeOccurrence(scope, record.taskId, record.scheduledAtMs)
                }
            }
        }
        history.markTerminal(record.runId, status, errorMessage = message)
        postTerminal(history.record(record.runId))
        repository.refresh()
        // Per-item failure is terminal in history, not in the Work dependency
        // graph. Returning success lets the next globally serialized item run.
        return Result.success()
    }

    private fun postTerminal(record: CronRunRecord?) {
        record ?: return
        val body = record.resultText ?: record.errorMessage ?: "已完成"
        CronNotifications.postResult(
            context = applicationContext,
            title = record.prompt.lineSequence().firstOrNull()?.take(40)?.ifBlank { null }
                ?: "定时任务",
            body = body,
            tag = "cron-${record.taskId}",
            runId = record.runId,
        )
    }

    companion object {
        private const val TAG = "CronExecutionWorker"
        private const val AGENT_RUN_BUDGET_MS = 3L * 60L * 1000L
        private const val RECONCILE_BUDGET_MS = 10_000L
        private const val MAX_EXECUTION_ATTEMPTS = 5
    }
}

internal fun isRetryableCronFailure(message: String?): Boolean {
    val normalized = message?.lowercase() ?: return false
    return RETRYABLE_MARKERS.any(normalized::contains)
}

private val RETRYABLE_MARKERS = listOf(
    "http 429",
    "status 429",
    "too many requests",
    "rate limit",
    "http 500",
    "http 502",
    "http 503",
    "http 504",
    "status 500",
    "status 502",
    "status 503",
    "status 504",
    "temporarily unavailable",
    "connection reset",
    "connection failed",
    "timeout",
    "timed out",
)
