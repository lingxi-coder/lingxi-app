package com.lingxi.code.cron

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

/**
 * Re-arms the next cron alarm after a reboot or app update — exact alarms do not
 * survive either. It ONLY arms the alarm (a cold [CronAlarmScheduler.armNext]);
 * it never starts the foreground service directly, which Android 14 forbids from
 * a BOOT_COMPLETED receiver. The actual firing happens later when the armed exact
 * alarm goes off.
 *
 * Uses `goAsync()` because building a transient engine to query the next fire may
 * exceed the synchronous receiver window.
 */
class CronBootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            Intent.ACTION_BOOT_COMPLETED, Intent.ACTION_MY_PACKAGE_REPLACED -> {
                val pending = goAsync()
                val appContext = context.applicationContext
                CoroutineScope(Dispatchers.Default).launch {
                    try {
                        CronAlarmScheduler.armNext(appContext)
                    } finally {
                        pending.finish()
                    }
                }
            }
        }
    }
}
