package com.lingxi.code.localapps

import android.content.Context
import android.util.Log
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import com.lingxi.code.cron.HeadlessEngineFactory
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CancellationException

/**
 * Android's durable wake-up for Host-journaled local-app flows. The Rust Host
 * remains the scheduler source of truth; this watchdog only wakes it often
 * enough to honor the contract's 15-minute minimum interval.
 */
internal object LocalAppBackgroundScheduler {
    private const val WORK_NAME = "local-app-background-watchdog-15m"
    private const val WATCHDOG_MINUTES = 15L

    fun ensureWatchdog(context: Context) {
        val request = PeriodicWorkRequestBuilder<LocalAppBackgroundWorker>(
            WATCHDOG_MINUTES,
            TimeUnit.MINUTES,
        )
            .addTag(WORK_NAME)
            .build()
        WorkManager.getInstance(context.applicationContext).enqueueUniquePeriodicWork(
            WORK_NAME,
            ExistingPeriodicWorkPolicy.KEEP,
            request,
        )
    }
}

private const val LOCAL_APP_BACKGROUND_TAG = "LocalAppBackground"

internal class LocalAppBackgroundWorker(
    appContext: Context,
    params: WorkerParameters,
) : CoroutineWorker(appContext, params) {
    override suspend fun doWork(): Result {
        val engine = HeadlessEngineFactory.build(applicationContext)
            ?: return Result.retry()
        return try {
            engine.runDueLocalAppBackgroundTasks(System.currentTimeMillis().toULong())
            Result.success()
        } catch (cancel: CancellationException) {
            throw cancel
        } catch (error: Throwable) {
            Log.w(LOCAL_APP_BACKGROUND_TAG, "local-app background wake failed", error)
            Result.retry()
        } finally {
            runCatching { engine.destroy() }
        }
    }
}
