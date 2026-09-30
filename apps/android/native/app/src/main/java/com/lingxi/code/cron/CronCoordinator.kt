package com.lingxi.code.cron

import android.content.Context
import android.util.Log
import kotlinx.coroutines.CancellationException

/**
 * Recomputes the one global earliest exact alarm across global and Project
 * scopes. Every entry point also ensures the 15-minute recovery watchdog.
 */
internal class CronCoordinator(
    context: Context,
    private val repository: AndroidCronRepository = AndroidCronRepository.get(context),
    private val now: () -> Long = System::currentTimeMillis,
) {
    private val appContext = context.applicationContext

    suspend fun reconcile(reason: String) {
        CronWorkScheduler.ensureWatchdog(appContext)
        try {
            repository.refreshNow(lastReconciledAtMs = now())
            repository.state.value.errorMessage?.let { message ->
                error(message)
            }
            val next = repository.state.value.nextScheduledAtMs
            if (next == null || !CronAlarmScheduler.canScheduleExact(appContext)) {
                CronAlarmScheduler.cancel(appContext)
            } else {
                CronAlarmScheduler.arm(appContext, next)
            }
            Log.i(TAG, "cron reconciled: reason=$reason next=$next")
        } catch (cancel: CancellationException) {
            throw cancel
        } catch (error: Throwable) {
            Log.w(TAG, "cron reconcile failed: reason=$reason", error)
            repository.refresh()
            throw error
        }
    }

    companion object {
        private const val TAG = "CronCoordinator"
    }
}
