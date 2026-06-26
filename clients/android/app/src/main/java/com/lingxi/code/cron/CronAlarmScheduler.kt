package com.lingxi.code.cron

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log

/**
 * Owns the single exact-alarm that drives cron firing. AlarmManager is the
 * phone's substitute for the desktop tick loop: it wakes [CronAlarmReceiver] at
 * the next due cron minute, which starts [CronRunService].
 *
 * One stable [REQUEST_CODE] ⇒ re-arming REPLACES the pending alarm (never stacks
 * a second). [arm] is the cheap path the service uses after a run (it already
 * queried the next fire); [armNext] is the cold path (boot / app-open / UI edit)
 * that builds a transient engine to ask the engine for the next fire.
 *
 * Exact-alarm gating: on Android 13+ `USE_EXACT_ALARM` grants exact alarms with
 * no prompt; on Android 12 (31-32) `SCHEDULE_EXACT_ALARM` is user-revocable, so
 * [canScheduleExact] is checked and a denied state falls back to an inexact
 * (Doze-batched) alarm — late, but it still fires.
 */
object CronAlarmScheduler {

    private const val TAG = "CronAlarmScheduler"
    private const val REQUEST_CODE = 0xC0DE // single stable code ⇒ re-arm replaces
    const val ACTION_CRON_FIRE = "com.lingxi.code.cron.ACTION_CRON_FIRE"

    /**
     * Arm the next exact alarm at `triggerAtMs` (epoch millis). A time already in
     * the past fires (almost) immediately. Falls back to an inexact alarm when
     * exact alarms are not permitted.
     */
    fun arm(context: Context, triggerAtMs: Long) {
        val appContext = context.applicationContext
        val alarmManager =
            appContext.getSystemService(Context.ALARM_SERVICE) as? AlarmManager ?: return
        val pendingIntent = pendingIntent(appContext)
        val trigger = maxOf(triggerAtMs, System.currentTimeMillis())
        try {
            if (canScheduleExact(appContext)) {
                alarmManager.setExactAndAllowWhileIdle(
                    AlarmManager.RTC_WAKEUP,
                    trigger,
                    pendingIntent,
                )
            } else {
                // Doze-batched, approximate — fires late but fires. The UI surfaces
                // a banner prompting the user to grant exact alarms.
                alarmManager.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, trigger, pendingIntent)
            }
            Log.i(TAG, "armed cron alarm at $trigger (exact=${canScheduleExact(appContext)})")
        } catch (se: SecurityException) {
            // Exact-alarm permission revoked between the gate check and the call —
            // degrade to an inexact alarm rather than crash.
            runCatching {
                alarmManager.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, trigger, pendingIntent)
            }
        }
    }

    /** Cancel any pending cron alarm (no jobs left, or cron disabled). */
    fun cancel(context: Context) {
        val appContext = context.applicationContext
        val alarmManager =
            appContext.getSystemService(Context.ALARM_SERVICE) as? AlarmManager ?: return
        alarmManager.cancel(pendingIntent(appContext))
    }

    /**
     * Cold re-arm: build a transient headless engine, ask it for the earliest
     * next fire, and arm (or cancel when there are no jobs). Used by the boot
     * receiver, on app launch, and after a UI create/delete. A missing API key
     * (no engine) leaves any existing alarm in place.
     */
    suspend fun armNext(context: Context) {
        val appContext = context.applicationContext
        val handle = HeadlessEngineFactory.build(appContext) ?: return
        try {
            val nextMs = handle.nextCronFireTime()
            if (nextMs == null) {
                cancel(appContext)
            } else {
                arm(appContext, nextMs.toLong())
            }
        } catch (t: Throwable) {
            Log.w(TAG, "armNext failed: ${t.message}")
        } finally {
            runCatching { handle.destroy() }
        }
    }

    /** Whether exact alarms can be scheduled (always pre-Android 12). */
    fun canScheduleExact(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return true
        val alarmManager =
            context.getSystemService(Context.ALARM_SERVICE) as? AlarmManager ?: return false
        return alarmManager.canScheduleExactAlarms()
    }

    private fun pendingIntent(context: Context): PendingIntent {
        val intent = Intent(context, CronAlarmReceiver::class.java).setAction(ACTION_CRON_FIRE)
        return PendingIntent.getBroadcast(
            context,
            REQUEST_CODE,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }
}
