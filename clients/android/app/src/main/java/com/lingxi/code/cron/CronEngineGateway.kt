package com.lingxi.code.cron

import android.content.Context
import com.lingxi.code.R
import com.lingxi.code.bindings.CronTaskDto
import com.lingxi.code.bindings.MobileCronStoreHandle
import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.bindings.buildAndroidCronStore

/**
 * Narrow seam around generated UniFFI bindings. Storage/reconciliation never
 * builds an LLM client; only execution creates a short-lived Engine handle.
 */
internal interface CronEngineGateway {
    suspend fun list(scope: CronScope): List<CronTaskDto>
    suspend fun dueOccurrences(scope: CronScope, nowMs: Long): List<Pair<String, Long>>
    suspend fun create(
        scope: CronScope,
        cron: String,
        prompt: String,
        recurring: Boolean,
    ): CronTaskDto
    suspend fun update(
        scope: CronScope,
        taskId: String,
        cron: String,
        prompt: String,
        recurring: Boolean,
    ): CronTaskDto
    suspend fun delete(scope: CronScope, taskId: String): Boolean
    suspend fun runTaskIfDue(
        scope: CronScope,
        taskId: String,
        scheduledAtMs: Long,
    ): CronTaskExecutionBatch
    suspend fun runTaskNow(scope: CronScope, taskId: String): CronTaskExecutionBatch
    suspend fun acknowledgeOccurrence(
        scope: CronScope,
        taskId: String,
        scheduledAtMs: Long,
    ): Boolean
}

internal class MobileCronEngineGateway(context: Context) : CronEngineGateway {
    private val appContext = context.applicationContext

    override suspend fun list(scope: CronScope): List<CronTaskDto> =
        withStore(scope) { it.list() }

    override suspend fun dueOccurrences(scope: CronScope, nowMs: Long): List<Pair<String, Long>> =
        withStore(scope) { store ->
            store.dueOccurrences(nowMs.toULong()).map { occurrence ->
                occurrence.taskId to occurrence.scheduledAtMs.toLong()
            }
        }

    override suspend fun create(
        scope: CronScope,
        cron: String,
        prompt: String,
        recurring: Boolean,
    ): CronTaskDto = withStore(scope) { it.create(cron, prompt, recurring) }

    override suspend fun update(
        scope: CronScope,
        taskId: String,
        cron: String,
        prompt: String,
        recurring: Boolean,
    ): CronTaskDto = withStore(scope) {
        it.update(taskId, cron, prompt, recurring)
    }

    override suspend fun delete(scope: CronScope, taskId: String): Boolean =
        withStore(scope) { it.delete(taskId) }

    /**
     * Rust atomically checks the task and occurrence anchor. Retryable failures
     * remain unacknowledged so WorkManager can use the same anchor on retry.
     */
    override suspend fun runTaskIfDue(
        scope: CronScope,
        taskId: String,
        scheduledAtMs: Long,
    ): CronTaskExecutionBatch {
        val fired = withEngine(scope) {
            it.runCronTaskIfDue(taskId, scheduledAtMs.toULong())
        }
        return CronTaskExecutionBatch(listOfNotNull(fired))
    }

    override suspend fun runTaskNow(
        scope: CronScope,
        taskId: String,
    ): CronTaskExecutionBatch {
        val fired = withEngine(scope) { it.runCronTaskNow(taskId) }
        return CronTaskExecutionBatch(listOfNotNull(fired))
    }

    override suspend fun acknowledgeOccurrence(
        scope: CronScope,
        taskId: String,
        scheduledAtMs: Long,
    ): Boolean = withEngine(scope) {
        it.acknowledgeCronOccurrence(taskId, scheduledAtMs.toULong())
    }

    private suspend fun <T> withStore(
        scope: CronScope,
        block: suspend (MobileCronStoreHandle) -> T,
    ): T {
        val store = buildAndroidCronStore(
            appFilesRoot = appContext.filesDir.absolutePath,
            projectCwd = scope.projectId?.let { scope.workspacePath },
        )
        return try {
            block(store)
        } finally {
            runCatching { store.destroy() }
        }
    }

    private suspend fun <T> withEngine(
        scope: CronScope,
        block: suspend (MobileEngineHandle) -> T,
    ): T {
        val handle = HeadlessEngineFactory.build(appContext, scope)
            ?: throw CronPermanentExecutionException(
                appContext.getString(R.string.cron_engine_unavailable),
            )
        return try {
            block(handle)
        } finally {
            runCatching { handle.destroy() }
        }
    }
}

internal class CronPermanentExecutionException(message: String) : Exception(message)
