package com.lingxi.code.cron

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Receives the exact cron alarm and performs no engine or network work. The
 * unique expedited dispatch WorkRequest survives process loss and deduplicates
 * repeated broadcasts for the same scheduled instant.
 */
class CronAlarmReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != CronAlarmScheduler.ACTION_CRON_FIRE) return
        val scheduledAtMs = intent.getLongExtra(
            CronWorkKeys.SCHEDULED_AT_MS,
            System.currentTimeMillis(),
        )
        CronWorkScheduler.enqueueDispatch(
            context = context.applicationContext,
            scheduledAtMs = scheduledAtMs,
            expedited = true,
        )
    }
}
