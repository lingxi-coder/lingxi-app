package com.lingxi.code.push

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import com.lingxi.code.MainActivity
import com.lingxi.code.BuildConfig
import com.lingxi.code.settings.NotificationPrefsStore

internal object PushMessagePolicy {
    fun accepts(data: Map<String, String>, currentUid: String?, enabled: Boolean, now: Long): Boolean =
        enabled && currentUid != null && data["recipientUid"] == currentUid &&
            (data["expiresAt"]?.toLongOrNull() ?: Long.MIN_VALUE) > now &&
            !data["title"].isNullOrBlank() && !data["body"].isNullOrBlank()
}

class LingxiMessagingService : FirebaseMessagingService() {
    override fun onRegistered(installationId: String) { PushRegistration.registered(this, installationId) }
    override fun onMessageReceived(message: RemoteMessage) {
        val enabled = NotificationPrefsStore(this).load().enabled && NotificationManagerCompat.from(this).areNotificationsEnabled()
        val accepted = PushMessagePolicy.accepts(message.data, FirebaseAuth.getInstance().currentUser?.uid, enabled, System.currentTimeMillis())
        // Correlate opt-in device checks without logging users, content or FIDs.
        if (BuildConfig.DEBUG) {
            message.data["verificationId"]?.takeIf { it.matches(Regex("[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}")) }?.let {
                Log.d("LingxiPush", "verification=$it result=${if (accepted) "accepted" else "rejected"}")
            }
        }
        if (!accepted) return
        PushNotifications.show(this, message)
    }
}

internal object PushNotifications {
    private const val CHANNEL = "lingxi_push"
    fun clear(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.activeNotifications.filter { it.tag == CHANNEL }.forEach { manager.cancel(it.tag, it.id) }
    }
    fun show(context: Context, message: RemoteMessage) {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL, "LingXi", NotificationManager.IMPORTANCE_DEFAULT))
        val open = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP)
        // A Remote Control notification opens its session; MainActivity vets the link like any other.
        PushRoute.link(message.data["route"])?.let { open.data = android.net.Uri.parse(it) }
        val intent = PendingIntent.getActivity(context, 0, open,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentTitle(message.data["title"]).setContentText(message.data["body"])
            .setStyle(NotificationCompat.BigTextStyle().bigText(message.data["body"]))
            .setContentIntent(intent).setAutoCancel(true)
            .setTimeoutAfter((message.data.getValue("expiresAt").toLong() - System.currentTimeMillis()).coerceAtLeast(1))
            .build()
        try { manager.notify(CHANNEL, (message.messageId ?: message.sentTime.toString()).hashCode(), notification) }
        catch (_: SecurityException) { /* Permission can be revoked between the check and notify. */ }
    }
}
