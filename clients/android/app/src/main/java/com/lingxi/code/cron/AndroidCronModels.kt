package com.lingxi.code.cron

import android.content.Context
import com.lingxi.code.R
import com.lingxi.code.bindings.CronTaskDto

const val GLOBAL_CRON_SCOPE_ID = "global"

/**
 * A validated Android execution scope. Only [workspacePath] is exposed to the
 * headless engine; Project metadata and SAF grants remain outside the workspace.
 */
data class CronScope(
    val scopeId: String,
    val projectId: String?,
    val projectName: String,
    val workspacePath: String,
    val guestPath: String,
) {
    companion object {
        fun global(context: Context): CronScope = CronScope(
            scopeId = GLOBAL_CRON_SCOPE_ID,
            projectId = null,
            projectName = context.applicationContext.getString(R.string.common_global),
            workspacePath = context.applicationContext.filesDir.canonicalPath,
            guestPath = "/workspace/global",
        )
    }
}

enum class CronSchedulingMode {
    Exact,
    FifteenMinuteFallback,
    Unsupported,
}

enum class CronRunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    Skipped,
    ;

    val isTerminal: Boolean
        get() = this in setOf(Succeeded, Failed, TimedOut, Cancelled, Skipped)
}

data class CronRunRecord(
    val runId: String,
    val taskId: String,
    val scopeId: String,
    val projectId: String?,
    val projectName: String,
    val prompt: String,
    val scheduledAtMs: Long,
    val triggeredAtMs: Long,
    val startedAtMs: Long? = null,
    val finishedAtMs: Long? = null,
    val status: CronRunStatus = CronRunStatus.Queued,
    val attempt: Int = 0,
    val resultText: String? = null,
    val errorMessage: String? = null,
    val manual: Boolean = false,
)

data class AndroidCronTask(
    val task: CronTaskDto,
    val scope: CronScope,
    val schedulingMode: CronSchedulingMode,
    val unsupportedReason: String? = null,
    val activeRun: CronRunRecord? = null,
    val lastRun: CronRunRecord? = null,
)

data class AndroidCronRepositoryState(
    val loading: Boolean = true,
    val tasks: List<AndroidCronTask> = emptyList(),
    val scopes: List<CronScope> = emptyList(),
    val activeScopeId: String = GLOBAL_CRON_SCOPE_ID,
    val history: List<CronRunRecord> = emptyList(),
    val exactAlarmAllowed: Boolean = true,
    val schedulingMode: CronSchedulingMode = CronSchedulingMode.Exact,
    val nextScheduledAtMs: Long? = null,
    val activeWorkCount: Int = 0,
    val networkAvailable: Boolean = true,
    val workManagerStateCounts: Map<String, Int> = emptyMap(),
    val lastReconciledAtMs: Long? = null,
    val errorMessage: String? = null,
)

internal data class ScopedCronTask(
    val scope: CronScope,
    val task: CronTaskDto,
)

internal data class ScopedCronOccurrence(
    val scope: CronScope,
    val task: CronTaskDto,
    val scheduledAtMs: Long,
)

internal data class CronTaskExecutionBatch(
    val firedJobs: List<com.lingxi.code.bindings.FiredCronJobDto>,
)
