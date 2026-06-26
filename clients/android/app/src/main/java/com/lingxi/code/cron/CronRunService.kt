package com.lingxi.code.cron

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import com.lingxi.code.bindings.CronFireStatusDto
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout

/**
 * The foreground service that runs a fired cron pass. Woken by [CronAlarmReceiver]
 * at the due cron minute, it:
 *
 *  1. enters the foreground (mandatory within 5s) with the running notification;
 *  2. holds a partial wakelock so a network turn completes if the device idles;
 *  3. builds a headless engine, calls `runDueCronNow()` (each due job runs a
 *     fresh, isolated turn to completion), posts a result notification per job;
 *  4. queries the next fire and re-arms the exact alarm;
 *  5. tears the engine down, releases the wakelock, and stops.
 *
 * `START_NOT_STICKY`: the ALARM (not the service) is the source of truth, so a
 * killed service is not auto-restarted with a stale intent.
 */
class CronRunService : Service() {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private var wakeLock: PowerManager.WakeLock? = null

    /** Guards against a second alarm firing a concurrent pass on this instance. */
    private val running = AtomicBoolean(false)

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Enter the foreground FIRST (mandatory within 5s of startForegroundService).
        // The platform can refuse the start (e.g. the inexact-alarm fallback path on
        // Android 12 has no FGS-start exemption, or background restrictions apply) —
        // a throw here must NOT crash the process; bail gracefully (the job is caught
        // up on the next successful fire).
        try {
            startForegroundCompat()
        } catch (t: Throwable) {
            Log.w(TAG, "startForeground refused: ${t.message}")
            stopSelf(startId)
            return START_NOT_STICKY
        }

        // Re-entrancy guard: a redundant alarm delivered while a pass is already
        // running is safely dropped — `runDueCronNow()` catches up ALL due jobs in
        // one pass, so a second concurrent engine would only race the tasks file and
        // leak a wakelock. We already satisfied the FGS-start contract above; let the
        // active pass own teardown.
        if (!running.compareAndSet(false, true)) {
            return START_NOT_STICKY
        }
        acquireWakeLock()
        scope.launch {
            try {
                withTimeout(RUN_BUDGET_MS) { runDueAndReschedule() }
            } catch (timeout: TimeoutCancellationException) {
                Log.w(TAG, "cron run timed out")
                CronNotifications.postResult(
                    applicationContext,
                    "定时任务",
                    "执行超时。",
                    "cron-error",
                )
                runCatching { CronAlarmScheduler.armNext(applicationContext) }
            } catch (cancel: CancellationException) {
                // Genuine teardown (service destroy / swipe-away) — propagate so we
                // don't post a spurious failure notification or break cooperative
                // cancellation. The `finally` still releases the wakelock + stops.
                throw cancel
            } catch (t: Throwable) {
                Log.w(TAG, "cron run failed: ${t.message}")
                CronNotifications.postResult(
                    applicationContext,
                    "定时任务",
                    "执行失败：${t.message ?: "未知错误"}",
                    "cron-error",
                )
                // Best-effort cold re-arm so the schedule keeps advancing.
                runCatching { CronAlarmScheduler.armNext(applicationContext) }
            } finally {
                running.set(false)
                releaseWakeLock()
                stopForegroundCompat()
                stopSelf()
            }
        }
        return START_NOT_STICKY
    }

    private suspend fun runDueAndReschedule() {
        val handle = HeadlessEngineFactory.build(applicationContext)
        if (handle == null) {
            CronNotifications.postResult(
                applicationContext,
                "定时任务已跳过",
                "未配置 API Key 或引擎不可用。配置后将自动恢复。",
                "cron-skip",
            )
            // Re-arm anyway so a key added later starts firing without a reboot.
            CronAlarmScheduler.armNext(applicationContext)
            return
        }
        try {
            val fired = handle.runDueCronNow()
            for (job in fired) {
                val body = when (val status = job.status) {
                    is CronFireStatusDto.Ok ->
                        job.resultText?.takeIf { it.isNotBlank() } ?: "已完成"
                    is CronFireStatusDto.Failed -> "失败：${status.message}"
                }
                val title = job.prompt.lineSequence().firstOrNull()?.take(40)?.ifBlank { null }
                    ?: "定时任务"
                CronNotifications.postResult(applicationContext, title, body, "cron-${job.id}")
            }
            // Re-arm the next exact alarm from the (now-updated) tasks file.
            val nextMs = handle.nextCronFireTime()
            if (nextMs == null) {
                CronAlarmScheduler.cancel(applicationContext)
            } else {
                CronAlarmScheduler.arm(applicationContext, nextMs.toLong())
            }
        } finally {
            runCatching { handle.destroy() }
        }
    }

    private fun startForegroundCompat() {
        val notification = CronNotifications.runningNotification(this)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ServiceCompat.startForeground(
                this,
                CronNotifications.FGS_NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
            )
        } else {
            startForeground(CronNotifications.FGS_NOTIFICATION_ID, notification)
        }
    }

    private fun stopForegroundCompat() {
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
    }

    private fun acquireWakeLock() {
        // Never overwrite a still-held lock (would orphan it until its timeout).
        if (wakeLock?.isHeld == true) return
        val powerManager = getSystemService(Context.POWER_SERVICE) as? PowerManager ?: return
        wakeLock = powerManager.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, WAKELOCK_TAG).apply {
            setReferenceCounted(false)
            acquire(WAKELOCK_TIMEOUT_MS)
        }
    }

    private fun releaseWakeLock() {
        wakeLock?.let { if (it.isHeld) runCatching { it.release() } }
        wakeLock = null
    }

    override fun onDestroy() {
        scope.cancel()
        releaseWakeLock()
        super.onDestroy()
    }

    companion object {
        private const val TAG = "CronRunService"
        private const val WAKELOCK_TAG = "lingxi:cron"

        /** Per-pass wall-clock budget (must exceed the Rust per-turn timeout). */
        private const val RUN_BUDGET_MS = 4L * 60L * 1000L

        /** Wakelock safety timeout (outlives the run budget). */
        private const val WAKELOCK_TIMEOUT_MS = 5L * 60L * 1000L

        /** Start the service in the foreground (from the alarm receiver). */
        fun start(context: Context) {
            ContextCompat.startForegroundService(
                context,
                Intent(context, CronRunService::class.java),
            )
        }
    }
}
