package com.lingxi.code.cron

import android.app.AlarmManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * System broadcasts only enqueue reconciliation. WorkManager performs storage
 * scans and alarm changes outside BroadcastReceiver's short execution window.
 */
class CronBootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            Intent.ACTION_BOOT_COMPLETED,
            Intent.ACTION_MY_PACKAGE_REPLACED,
            Intent.ACTION_TIME_CHANGED,
            Intent.ACTION_TIMEZONE_CHANGED,
            AlarmManager.ACTION_SCHEDULE_EXACT_ALARM_PERMISSION_STATE_CHANGED,
            -> CronWorkScheduler.enqueueReconcile(
                context.applicationContext,
                intent.action ?: "system",
            )
        }
    }
}
