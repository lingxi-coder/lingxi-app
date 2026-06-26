package com.lingxi.code.cron

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import com.lingxi.code.MainActivity

/**
 * The two notification channels the cron subsystem owns, kept SEPARATE from the
 * engine's `lingxi_agent` channel (driven by `tool-notification`) so the user can
 * control them independently:
 *
 *  - [RUNNING_CHANNEL] (low importance, silent, ongoing): the mandatory
 *    foreground-service notification shown while [CronRunService] runs a fired
 *    job. Auto-removed when the service stops.
 *  - [RESULT_CHANNEL] (default importance): one per fired job, summarizing the
 *    engine's result. Tapping opens the app.
 *
 * A result post is gated on POST_NOTIFICATIONS (Android 13+) and is best-effort —
 * a denied notification never crashes the service.
 */
object CronNotifications {

    private const val RUNNING_CHANNEL = "lingxi_cron_running"
    private const val RESULT_CHANNEL = "lingxi_cron_result"

    /** Stable id for the foreground-service notification. */
    const val FGS_NOTIFICATION_ID = 0xC401

    /** The ongoing, low-importance notification the foreground service shows. */
    fun runningNotification(context: Context): Notification {
        ensureChannels(context)
        return NotificationCompat.Builder(context, RUNNING_CHANNEL)
            .setContentTitle("正在运行定时任务…")
            .setSmallIcon(android.R.drawable.ic_popup_sync)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    /**
     * Post one cron-result notification. `tag` keys the post so a re-fire of the
     * same job replaces its prior result (stable id from `tag.hashCode()`). A
     * no-op when POST_NOTIFICATIONS is not granted.
     */
    fun postResult(context: Context, title: String, body: String, tag: String) {
        ensureChannels(context)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(
                context,
                Manifest.permission.POST_NOTIFICATIONS,
            ) != PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val contentIntent = PendingIntent.getActivity(
            context,
            0,
            Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(context, RESULT_CHANNEL)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setAutoCancel(true)
            .setContentIntent(contentIntent)
            .build()
        runCatching {
            NotificationManagerCompat.from(context).notify(tag, tag.hashCode(), notification)
        }
    }

    /** Create both channels on first use (Android 8+/O); a no-op afterwards. */
    private fun ensureChannels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (manager.getNotificationChannel(RUNNING_CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(
                    RUNNING_CHANNEL,
                    "定时任务运行中",
                    NotificationManager.IMPORTANCE_LOW,
                ),
            )
        }
        if (manager.getNotificationChannel(RESULT_CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(
                    RESULT_CHANNEL,
                    "定时任务结果",
                    NotificationManager.IMPORTANCE_DEFAULT,
                ),
            )
        }
    }
}
