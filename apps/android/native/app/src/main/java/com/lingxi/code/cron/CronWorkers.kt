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
import androidx.work.Operation
import androidx.work.OutOfQuotaPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import com.lingxi.code.R
import com.lingxi.code.bindings.runtime.CronFireStatusDto
import com.lingxi.code.bindings.runtime.FiredCronJobDto
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
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
    const val BUSY_ATTEMPT = "busy_attempt"
    const val RECOVER_RUNNING = "recover_running"
}

internal object CronWorkNames {
    const val WATCHDOG = "cron-watchdog-15m"
    const val RECONCILE = "cron-reconcile"
    const val SERIAL_EXECUTION = "cron-global-serial-execution"
    const val TAG_EXECUTION = "cron-execution"

    fun busyRetry(runId: String, attempt: Int): String = "cron-busy-$runId-$attempt"

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

    fun enqueueExecutionChain(context: Context, records: List<CronRunRecord>): Operation? {
        if (records.isEmpty()) return null
        val requests = records.map(::executionRequest)
        var continuation = WorkManager.getInstance(context.applicationContext).beginUniqueWork(
            CronWorkNames.SERIAL_EXECUTION,
            ExistingWorkPolicy.APPEND_OR_REPLACE,
            requests.first(),
        )
        for (request in requests.drop(1)) {
            continuation = continuation.then(request)
        }
        return continuation.enqueue()
    }

    suspend fun enqueueBusyRetry(context: Context, record: CronRunRecord, recoverRunning: Boolean = false) {
        val request = OneTimeWorkRequestBuilder<CronBusyRetryWorker>()
            .setInitialDelay(30, TimeUnit.SECONDS)
            .setInputData(Data.Builder()
                .putString(CronWorkKeys.RUN_ID, record.runId)
                .putInt(CronWorkKeys.BUSY_ATTEMPT, record.attempt)
                .putBoolean(CronWorkKeys.RECOVER_RUNNING, recoverRunning)
                .build())
            .build()
        // This delayed wake is independent of SERIAL_EXECUTION: the busy task
        // releases that chain now, and rejoins its tail only after the delay.
        val operation = WorkManager.getInstance(context.applicationContext).enqueueUniqueWork(
            CronWorkNames.busyRetry(record.runId, record.attempt),
            ExistingWorkPolicy.KEEP,
            request,
        )
        withContext(Dispatchers.IO) { operation.result.get() }
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

/** Durable busy retry wakes never wait at the front of the execution chain. */
class CronBusyRetryWorker(appContext: Context, params: WorkerParameters) : CoroutineWorker(appContext, params) {
    override suspend fun doWork(): Result {
        val runId = inputData.getString(CronWorkKeys.RUN_ID) ?: return Result.success()
        val attempt = inputData.getInt(CronWorkKeys.BUSY_ATTEMPT, -1)
        val repository = AndroidCronRepository.get(applicationContext)
        val record = busyCronRetryRecord(repository.historyStore, runId, attempt, inputData.getBoolean(CronWorkKeys.RECOVER_RUNNING, false)) ?: return Result.success()
        return try {
            // Pause/delete cancel queued history; execution also revalidates
            // current task state and Rust atomically validates the occurrence.
            CronWorkScheduler.enqueueExecutionChain(applicationContext, listOf(record))?.let { operation ->
                withContext(Dispatchers.IO) { operation.result.get() }
            }
            Result.success()
        } catch (cancel: CancellationException) {
            throw cancel
        } catch (_: Exception) {
            Result.retry()
        }
    }
}

internal fun busyCronRetryRecord(history: CronRunHistoryStore, runId: String, attempt: Int, recoverRunning: Boolean = false): CronRunRecord? =
    history.record(runId)?.takeIf {
        it.status == (if (recoverRunning) CronRunStatus.Running else CronRunStatus.Queued) && it.attempt == attempt
    }

internal suspend fun parkBusyCronRun(
    history: CronRunHistoryStore,
    runId: String,
    attempt: Int,
    message: String,
    enqueueWake: suspend (CronRunRecord) -> Unit,
) {
    val queued = history.markRetry(runId, attempt, message) ?: return
    if (queued.status == CronRunStatus.Queued) enqueueWake(queued)
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

class CronExecutionWorker internal constructor(
    appContext: Context,
    params: WorkerParameters,
    private val repository: AndroidCronRepository,
    private val gateway: CronEngineGateway,
    private val reconcileSchedules: suspend () -> Unit = {
        CronCoordinator(appContext, repository).reconcile("execution-finished")
    },
) : CoroutineWorker(appContext, params) {
    constructor(appContext: Context, params: WorkerParameters) : this(
        appContext, params, AndroidCronRepository.get(appContext), MobileCronEngineGateway(appContext),
    )

    private val history = repository.historyStore
    private val runId = params.inputData.getString(CronWorkKeys.RUN_ID)

    override suspend fun doWork(): Result = try {
        executeOccurrence()
    } catch (cancel: CancellationException) {
        throw cancel
    } catch (error: Throwable) {
        // Includes history/scope reads before a task can be safely identified.
        // An exception must never fail the entire WorkManager dependency chain.
        Log.w(TAG, "cron occurrence preflight unavailable", error)
        Result.retry()
    }

    private suspend fun executeOccurrence(): Result {
        val id = runId ?: return Result.success()
        val existing = history.record(id) ?: return Result.success()
        if (existing.status.isTerminal) {
            repository.postPendingNotifications(id)
            return Result.success()
        }
        val scope = repository.scope(existing.scopeId)
            ?: return terminalFailure(
                existing,
                applicationContext.getString(R.string.cron_workspace_unavailable),
            )
        try {
            val task = gateway.list(scope).firstOrNull { it.id == existing.taskId }
            if (task != null && repository.recoverNativeTerminal(existing, task)) return Result.success()
            if (existing.status == CronRunStatus.Running) {
                history.markTerminal(id, CronRunStatus.Interrupted, errorMessage = "Execution was interrupted; it will not be replayed")
                postTerminal(history.record(id))
                return Result.success()
            }
            if (task == null || !task.isActive()) {
                history.markTerminal(id, CronRunStatus.Cancelled, errorMessage = "Task is no longer active")
                return Result.success()
            }
            val automation = CronAutomation.from(task)
            history.attachExecution(id, model = automation.model, notificationPolicy = automation.notificationPolicy)
            history.markRunning(id, maxOf(existing.attempt, runAttemptCount) + 1)
        } catch (cancel: CancellationException) {
            // No model invocation has started: retain the queued occurrence.
            throw cancel
        } catch (error: Throwable) {
            Log.w(TAG, "cron task preflight failed", error)
            // A previous started execution needs native terminal reconciliation;
            // never turn it back into runnable work merely because reads failed.
            return try {
                withContext(NonCancellable) {
                    val attempt = maxOf(existing.attempt, runAttemptCount) + 1
                    if (existing.status == CronRunStatus.Running) {
                        // Advance only the wake generation, retaining started
                        // state so successful recovery can never invoke it again.
                        history.markRunning(id, attempt)?.takeIf { it.status == CronRunStatus.Running }?.let {
                            CronWorkScheduler.enqueueBusyRetry(applicationContext, it, recoverRunning = true)
                        }
                    } else {
                        parkBusyCronRun(history, id, attempt,
                            "preflight: ${error.message ?: "Scheduled task is temporarily unavailable"}") {
                            CronWorkScheduler.enqueueBusyRetry(applicationContext, it)
                        }
                    }
                }
                repository.refresh()
                Result.success()
            } catch (cancel: CancellationException) {
                throw cancel
            } catch (handoffError: Throwable) {
                Log.w(TAG, "cron preflight retry handoff failed", handoffError)
                Result.retry()
            }
        }
        repository.refresh()
        return try {
            val batch = withTimeout(AGENT_RUN_BUDGET_MS) {
                if (existing.manual) {
                    gateway.runTaskNow(scope, existing.taskId, existing.scheduledAtMs)
                } else {
                    gateway.runTaskIfDue(scope, existing.taskId, existing.scheduledAtMs)
                }
            }
            applyBatch(scope, existing, batch.firedJobs)
        } catch (timeout: TimeoutCancellationException) {
            terminalFailure(
                existing,
                applicationContext.getString(R.string.cron_execution_timeout_message),
                status = CronRunStatus.TimedOut,
            )
        } catch (cancel: CancellationException) {
            // A busy handoff has no started model turn. Its independent wake
            // survives this worker stopping, so keep that occurrence queued.
            val parked = history.record(id)?.let {
                it.status == CronRunStatus.Queued && it.errorMessage?.startsWith("busy:") == true
            } == true
            if (!parked) history.markTerminal(id, CronRunStatus.Interrupted, errorMessage = "Execution was interrupted; it will not be replayed")
            repository.refresh()
            throw cancel
        } catch (error: Throwable) {
            if (error is CronPermanentExecutionException || !isRetryableCronFailure(error.message)) {
                terminalFailure(
                    existing,
                    error.message
                        ?: applicationContext.getString(R.string.cron_execution_failed_default_android),
                )
            } else if (runAttemptCount + 1 < MAX_EXECUTION_ATTEMPTS) {
                history.markRetry(
                    id,
                    runAttemptCount + 1,
                    error.message
                        ?: applicationContext.getString(R.string.cron_temporary_network_error),
                )
                repository.refresh()
                Result.retry()
            } else {
                terminalFailure(
                    existing,
                    error.message ?: applicationContext.getString(R.string.cron_retries_exhausted),
                )
            }
        } finally {
            withContext(NonCancellable) {
                try {
                    withTimeout(RECONCILE_BUDGET_MS) {
                        reconcileSchedules()
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
            val queued = history.record(requested.runId)?.takeIf { it.taskId == fired.id && !it.status.isTerminal } ?: continue
            val actualModel = runCatching {
                gateway.list(scope).firstOrNull { it.id == fired.id }?.let { task ->
                    val runs = org.json.JSONObject(CronAutomation.from(task).json).optJSONArray("runs")
                    if (runs == null) null else (runs.length() - 1 downTo 0).firstNotNullOfOrNull { index ->
                        val run = runs.getJSONObject(index)
                        run.optString("model").takeIf {
                            it.isNotBlank() && run.optString("sessionId") == fired.sessionId
                        }
                    }
                }
            }.getOrNull()
            history.attachExecution(queued.runId, sessionId = fired.sessionId, model = actualModel)
            when (val status = fired.status) {
                CronFireStatusDto.Ok -> {
                    history.markTerminal(
                        queued.runId,
                        CronRunStatus.Succeeded,
                        resultText = fired.resultText?.ifBlank { null }
                            ?: applicationContext.getString(R.string.chat_status_completed),
                    )
                    postTerminal(history.record(queued.runId))
                    if (queued.runId == requested.runId) requestedResult = Result.success()
                }
                is CronFireStatusDto.Failed -> {
                    if (status.message.startsWith("busy:")) {
                        val result = try {
                            withContext(NonCancellable) {
                                parkBusyCronRun(history, queued.runId, maxOf(queued.attempt, runAttemptCount) + 1, status.message) {
                                    CronWorkScheduler.enqueueBusyRetry(applicationContext, it)
                                }
                            }
                            // Only release the chain after the independent wake
                            // commits. A failed handoff retains this worker retry.
                            Result.success()
                        } catch (cancel: CancellationException) {
                            throw cancel
                        } catch (_: Exception) {
                            Result.retry()
                        }
                        if (queued.runId == requested.runId) requestedResult = result
                    } else if (fired.retryable && runAttemptCount + 1 < MAX_EXECUTION_ATTEMPTS) {
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
                errorMessage = applicationContext.getString(
                    R.string.cron_task_removed_or_claimed_message,
                ),
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
        record?.let { repository.postPendingNotifications(it.runId) }
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
