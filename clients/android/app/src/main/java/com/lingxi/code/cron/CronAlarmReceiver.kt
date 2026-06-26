package com.lingxi.code.cron

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Receives the exact cron alarm and hands off to [CronRunService]. A
 * BroadcastReceiver has only ~10s and cannot do network work, so it does the
 * minimum: start the foreground service, which builds the engine and runs the
 * due jobs. (Starting a `dataSync` foreground service from an exact-alarm
 * broadcast is an allowed exemption, unlike from BOOT_COMPLETED.)
 */
class CronAlarmReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        CronRunService.start(context.applicationContext)
    }
}
