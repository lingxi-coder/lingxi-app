package com.lingxi.code.cron

import android.Manifest
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
import com.lingxi.code.notify.NotificationKind
import com.lingxi.code.notify.allows
import com.lingxi.code.settings.NotificationPrefsStore
import com.lingxi.code.R

/**
 * The result notification channel is separate from the
 * engine's `lingxi_agent` channel (driven by `tool-notification`) so the user can
 * control them independently:
 *
 *  - [RESULT_CHANNEL] (default importance): one per fired job, summarizing the
 *    engine's result. Tapping opens the app.
 *
 * A result post is gated on POST_NOTIFICATIONS (Android 13+) and is best-effort —
 * a denied notification never affects execution or durable history.
 */
object CronNotifications {

    private const val RESULT_CHANNEL = "lingxi_cron_result"

    const val EXTRA_CRON_RUN_ID = "com.lingxi.code.cron.RUN_ID"

    /**
     * Post one cron-result notification. `tag` keys the post so a re-fire of the
     * same job replaces its prior result (stable id from `tag.hashCode()`). A
     * no-op when POST_NOTIFICATIONS is not granted, or when the user has turned
     * scheduled-task reports off.
     *
     * The preference is checked HERE rather than at each call site: there are
     * several (`postPendingNotifications`, `recoverNativeTerminal`, the
     * archived-chat pause notice), and a toggle that only some of them honour is
     * worse than no toggle.
     *
     * ⚠️ Returning `false` for a preference-off post is deliberate and matches
     * the POST_NOTIFICATIONS branch below: callers release their durable claim
     * on `false`, so the result stays recoverable if the user turns reports back
     * on. Returning `true` here would consume the claim and lose it.
     */
    fun postResult(
        context: Context,
        title: String,
        body: String,
        tag: String,
        runId: String = tag,
    ): Boolean {
        if (!NotificationPrefsStore(context).load().allows(NotificationKind.ScheduledRun)) return false
        ensureChannels(context)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(
                context,
                Manifest.permission.POST_NOTIFICATIONS,
            ) != PackageManager.PERMISSION_GRANTED
        ) {
            // Not delivered. The caller must not record it as delivered, or the
            // result is lost even after the user grants the permission.
            return false
        }
        val contentIntent = PendingIntent.getActivity(
            context,
            runId.hashCode(),
            Intent(context, MainActivity::class.java)
                .putExtra(EXTRA_CRON_RUN_ID, runId)
                .setData(android.net.Uri.parse("lingxi://cron/run/$runId"))
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
        return runCatching {
            NotificationManagerCompat.from(context).notify(tag, tag.hashCode(), notification)
        }.isSuccess
    }

    /** Create the result channel on first use (Android 8+/O). */
    private fun ensureChannels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (manager.getNotificationChannel(RESULT_CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(
                    RESULT_CHANNEL,
                    context.getString(R.string.cron_notification_channel_name),
                    NotificationManager.IMPORTANCE_DEFAULT,
                ),
            )
        }
    }
}
