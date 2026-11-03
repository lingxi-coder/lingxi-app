package com.lingxi.code.cron

import android.content.Context
import com.lingxi.code.R
import com.lingxi.code.bindings.runtime.CronTaskDto
import com.lingxi.code.bindings.runtime.MobileCronStoreHandle
import com.lingxi.code.bindings.runtime.MobileEngineHandle
import com.lingxi.code.bindings.runtime.CronOwnerReservationDto
import com.lingxi.code.bindings.runtime.CronOwnerCandidateDto
import com.lingxi.code.bindings.runtime.MobileCronAdmission
import com.lingxi.code.bindings.runtime.MobileCronOwnerAdmission
import com.lingxi.code.bindings.android.buildAndroidCronStore

/**
 * Narrow seam around generated UniFFI bindings. Storage/reconciliation never
 * builds an LLM client; only execution creates a short-lived Engine handle.
 */
internal interface CronEngineGateway {
    suspend fun list(scope: CronScope): List<CronTaskDto>
    suspend fun listStrict(scope: CronScope): List<CronTaskDto>
    suspend fun listOwnerReservationsStrict(scope: CronScope): List<CronOwnerReservationDto>
    suspend fun dueOccurrences(scope: CronScope, nowMs: Long): List<Pair<String, Long>>
    suspend fun create(
        scope: CronScope,
        cron: String,
        prompt: String,
        recurring: Boolean,
        automation: CronAutomation,
    ): CronTaskDto
    suspend fun update(
        scope: CronScope,
        taskId: String,
        cron: String,
        prompt: String,
        recurring: Boolean,
        automation: CronAutomation,
    ): CronTaskDto
    suspend fun delete(scope: CronScope, taskId: String): Boolean
    suspend fun runTaskIfDue(
        scope: CronScope,
        taskId: String,
        scheduledAtMs: Long,
    ): CronTaskExecutionBatch
    suspend fun runTaskNow(scope: CronScope, taskId: String, scheduledAtMs: Long): CronTaskExecutionBatch
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

    override suspend fun listStrict(scope: CronScope): List<CronTaskDto> =
        withStore(scope) { it.listStrict() }

    override suspend fun listOwnerReservationsStrict(scope: CronScope): List<CronOwnerReservationDto> =
        withStore(scope) { it.listOwnerReservationsStrict() }

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
        automation: CronAutomation,
    ): CronTaskDto = withStore(scope) { it.createConfigured(cron, prompt, recurring, automation.json) }

    override suspend fun update(
        scope: CronScope,
        taskId: String,
        cron: String,
        prompt: String,
        recurring: Boolean,
        automation: CronAutomation,
    ): CronTaskDto = withStore(scope) {
        it.updateConfigured(taskId, cron, prompt, recurring, automation.json)
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
        val fired = withEngine(scope) { handle ->
            withOwnerAdmission(scope) { admission ->
                handle.runCronTaskIfDue(taskId, scheduledAtMs.toULong(), admission)
            }
        }
        return CronTaskExecutionBatch(listOfNotNull(fired))
    }

    override suspend fun runTaskNow(
        scope: CronScope,
        taskId: String,
        scheduledAtMs: Long,
    ): CronTaskExecutionBatch {
        val fired = withEngine(scope) { handle ->
            withOwnerAdmission(scope) { admission ->
                handle.runCronTaskNowAt(taskId, scheduledAtMs.toULong(), admission)
            }
        }
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
            managedRoot = com.lingxi.code.settings.LinuxRuntimeBridge.managedRoot(appContext),
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
                com.lingxi.code.theme.LocaleWrapper.getString(appContext, R.string.cron_engine_unavailable),
            )
        return try {
            block(handle)
        } finally {
            runCatching { handle.destroy() }
        }
    }

    private suspend fun <T> withOwnerAdmission(scope: CronScope, block: suspend (MobileCronAdmission) -> T): T {
        val adapter = AndroidCronOwnerAdmission(appContext, scope)
        adapter.prepare()
        val attempt = CronOwnerAdmissionAttempt(adapter::validateCandidate, adapter::persist)
        var admission: MobileCronAdmission? = null
        var barrierProven = false
        return try {
            val created = MobileCronAdmission(object : MobileCronOwnerAdmission {
                override suspend fun beginOwner(candidate: CronOwnerCandidateDto): Boolean = attempt.begin(
                    CronOwnerCandidate(candidate.sessionId, candidate.sessionTranscriptPath,
                        candidate.sessionMode, candidate.sessionScopeKey),
                )
                override suspend fun confirmOwner(reservation: CronOwnerReservationDto): Boolean = attempt.confirm(
                    CronOwnerIdentity(reservation.sessionId, reservation.sessionTranscriptPath,
                        reservation.sessionMode, reservation.sessionScopeKey),
                )
            })
            admission = created
            block(created)
        } finally {
            kotlinx.coroutines.withContext(kotlinx.coroutines.NonCancellable) {
                try {
                    attempt.settleAfterCaller { admission?.cancelAndWait() }
                    barrierProven = true
                } finally {
                    if (barrierProven) runCatching { admission?.destroy() }
                }
            }
        }
    }
}

internal class CronPermanentExecutionException(message: String) : Exception(message)
