package com.lingxi.code.cron

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log

/**
 * Owns the single exact-alarm that drives cron firing. AlarmManager is the
 * phone's substitute for the desktop tick loop: it wakes [CronAlarmReceiver] at
 * the next due cron minute. The receiver only enqueues durable WorkManager work.
 *
 * One stable [REQUEST_CODE] ⇒ re-arming REPLACES the pending alarm (never stacks
 * a second). [arm] is the cheap path when the next time is already known;
 * [armNext] is the compatibility path used by app-open and existing UI wiring.
 *
 * Exact-alarm gating uses user-revocable `SCHEDULE_EXACT_ALARM`. Denied devices
 * are handled by the unique 15-minute WorkManager watchdog; this class never
 * creates a second inexact AlarmManager schedule.
 */
object CronAlarmScheduler {

    private const val TAG = "CronAlarmScheduler"
    private const val REQUEST_CODE = 0xC0DE // single stable code ⇒ re-arm replaces
    const val ACTION_CRON_FIRE = "com.lingxi.code.cron.ACTION_CRON_FIRE"

    /**
     * Arm the next exact alarm at `triggerAtMs` (epoch millis). A time already in
     * the past fires (almost) immediately. A denied permission cancels the exact
     * alarm because the watchdog is the only degraded-mode scheduler.
     */
    fun arm(context: Context, triggerAtMs: Long) {
        val appContext = context.applicationContext
        CronWorkScheduler.ensureWatchdog(appContext)
        val alarmManager =
            appContext.getSystemService(Context.ALARM_SERVICE) as? AlarmManager ?: return
        val trigger = maxOf(triggerAtMs, System.currentTimeMillis())
        if (!canScheduleExact(appContext)) {
            alarmManager.cancel(pendingIntent(appContext, trigger))
            Log.i(TAG, "exact alarm permission unavailable; watchdog owns scheduling")
            return
        }
        try {
            alarmManager.setExactAndAllowWhileIdle(
                AlarmManager.RTC_WAKEUP,
                trigger,
                pendingIntent(appContext, trigger),
            )
            Log.i(TAG, "armed exact cron alarm at $trigger")
        } catch (se: SecurityException) {
            Log.w(TAG, "exact alarm permission revoked while arming", se)
            alarmManager.cancel(pendingIntent(appContext, trigger))
        }
    }

    /** Cancel any pending cron alarm (no jobs left, or cron disabled). */
    fun cancel(context: Context) {
        val appContext = context.applicationContext
        val alarmManager =
            appContext.getSystemService(Context.ALARM_SERVICE) as? AlarmManager ?: return
        alarmManager.cancel(pendingIntent(appContext, 0L))
    }

    /**
     * Compatibility entry used by existing app/UI wiring. Reconciliation scans
     * global and Project scopes, ensures the watchdog, and arms the single global
     * earliest alarm.
     */
    suspend fun armNext(context: Context) {
        CronCoordinator(context.applicationContext).reconcile("legacy-arm-next")
    }

    /** Whether exact alarms can be scheduled (always pre-Android 12). */
    fun canScheduleExact(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return true
        val alarmManager =
            context.getSystemService(Context.ALARM_SERVICE) as? AlarmManager ?: return false
        return alarmManager.canScheduleExactAlarms()
    }

    /** System settings intent shown only after an explanatory UI affordance. */
    fun permissionSettingsIntent(context: Context): Intent =
        Intent(
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM
            } else {
                Settings.ACTION_APPLICATION_DETAILS_SETTINGS
            },
            Uri.parse("package:${context.applicationContext.packageName}"),
        ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    private fun pendingIntent(context: Context, scheduledAtMs: Long): PendingIntent {
        val intent = Intent(context, CronAlarmReceiver::class.java)
            .setAction(ACTION_CRON_FIRE)
            .putExtra(CronWorkKeys.SCHEDULED_AT_MS, scheduledAtMs)
        return PendingIntent.getBroadcast(
            context,
            REQUEST_CODE,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }
}
