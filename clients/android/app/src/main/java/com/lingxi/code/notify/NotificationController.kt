package com.lingxi.code.notify

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat

private const val TAG = "NotificationController"

/** Channel the engine-driven notifications are posted on. */
private const val CHANNEL_ID = "lingxi_agent"
private const val CHANNEL_NAME = "Agent notifications"

/** Why a notify() failed; mapped onto `NotificationFfiException` by the adapter. */
sealed interface NotifyFailure {
    /** POST_NOTIFICATIONS not granted (Android 13+), or no attached context. */
    data object PermissionDenied : NotifyFailure
    data class Other(val message: String) : NotifyFailure
}

/** Raised by the controller; the adapter fans it onto `NotificationFfiException`. */
class NotifyException(val failure: NotifyFailure) : Exception(
    when (failure) {
        is NotifyFailure.Other -> failure.message
        else -> failure::class.simpleName ?: "notify error"
    },
)

/**
 * Process-global bridge between the (Rust-driven)
 * `com.lingxi.code.bindings.AndroidNotification` callback interface and the
 * system [NotificationManager].
 *
 * Mirrors [com.lingxi.code.share.ShareController]: because the engine has no
 * [Context] handle, the host Activity attaches the application context in
 * `onCreate` ([attach]) and clears it in `onDestroy` ([detach]). The
 * notification channel is created lazily on the first successful post.
 *
 * The post is engine-driven (`tool-notification`), so there is no user-facing
 * affordance — the model posts a notification by calling [notify].
 */
object NotificationController {

    /** Context wired up by the host Activity; null when nothing is attached. */
    @Volatile
    private var context: Context? = null

    /** Whether the notification channel has been created this process. */
    @Volatile
    private var channelReady = false

    /** Register the host Activity's (application) context. */
    fun attach(context: Context) {
        this.context = context.applicationContext
    }

    fun detach() {
        context = null
    }

    /**
     * Post a single local notification built from [title] / [body]. When [tag]
     * is present it is used as the notification id/tag so a later post with the
     * same tag replaces the earlier one; otherwise a time-based id is used.
     *
     * Throws [NotifyException] with [NotifyFailure.PermissionDenied] when no
     * context is attached or, on Android 13+ (TIRAMISU), when the
     * `POST_NOTIFICATIONS` runtime permission is not granted; with
     * [NotifyFailure.Other] on any other native failure.
     */
    fun notify(title: String, body: String, tag: String?) {
        val ctx = context ?: throw NotifyException(NotifyFailure.PermissionDenied)

        // Android 13+ gates posting on the POST_NOTIFICATIONS runtime permission.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val granted = ContextCompat.checkSelfPermission(
                ctx,
                Manifest.permission.POST_NOTIFICATIONS,
            ) == PackageManager.PERMISSION_GRANTED
            if (!granted) {
                throw NotifyException(NotifyFailure.PermissionDenied)
            }
        }

        try {
            ensureChannel(ctx)
            val notification = androidx.core.app.NotificationCompat.Builder(ctx, CHANNEL_ID)
                .setContentTitle(title)
                .setContentText(body)
                .setSmallIcon(android.R.drawable.ic_dialog_info)
                .setAutoCancel(true)
                .build()

            val manager = NotificationManagerCompat.from(ctx)
            // tag → stable id so a later post replaces an earlier one; otherwise unique.
            val id = tag?.hashCode() ?: System.currentTimeMillis().toInt()
            manager.notify(tag, id, notification)
        } catch (e: NotifyException) {
            throw e
        } catch (se: SecurityException) {
            // Defensive: NotificationManagerCompat can still raise on permission.
            Log.w(TAG, "notify denied: ${se.message}")
            throw NotifyException(NotifyFailure.PermissionDenied)
        } catch (t: Throwable) {
            Log.w(TAG, "notify failed: ${t.message}")
            throw NotifyException(NotifyFailure.Other(t.message ?: "notify failed"))
        }
    }

    /** Create the notification channel on first use (Android 8+/O). */
    private fun ensureChannel(ctx: Context) {
        if (channelReady) return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                CHANNEL_NAME,
                NotificationManager.IMPORTANCE_DEFAULT,
            )
            val manager = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            manager.createNotificationChannel(channel)
        }
        channelReady = true
    }
}
