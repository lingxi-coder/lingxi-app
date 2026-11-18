package com.lingxi.code.inbox

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import com.google.firebase.auth.FirebaseAuth
import com.lingxi.code.MainActivity
import com.lingxi.code.R
import com.lingxi.code.settings.NotificationPrefsStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

internal enum class InboxAlertDisposition { Inline, Banner, System }

internal data class InboxAlert(
    val ref: InboxRef, val event: String, val kind: String, val through: Long,
    val title: String, val body: String, val expiresAt: Long, val pendingActionRevision: Long? = null, val deliverAfter: Long = 0,
    val failureRevision: Long? = null,
    val messageId: String? = null,
    val taskId: String? = null,
) {
    fun target() = InboxNoticeTarget(ref, event, kind, through, expiresAt, messageId, taskId)

    companion object {
        const val OPEN_ACTION = "com.lingxi.code.action.OPEN_INBOX_CONVERSATION"
        const val EXTRA_KEY = "inbox_key"
        const val EXTRA_GENERATION = "inbox_generation"
        private const val EXTRA_EVENT = "inbox_event"
        private const val EXTRA_KIND = "inbox_kind"
        private const val EXTRA_THROUGH = "inbox_through"
        private const val EXTRA_EXPIRY = "inbox_expires"
        private const val EXTRA_MESSAGE = "inbox_message"
        private const val EXTRA_TASK = "inbox_task"

        fun addTarget(intent: Intent, target: InboxNoticeTarget): Intent = intent
            .putExtra(EXTRA_KEY, target.ref.key).putExtra(EXTRA_GENERATION, target.ref.generation)
            .putExtra(EXTRA_EVENT, target.id).putExtra(EXTRA_KIND, target.kind)
            .putExtra(EXTRA_THROUGH, target.through).putExtra(EXTRA_EXPIRY, target.expiresAt)
            .putExtra(EXTRA_MESSAGE, target.messageId).putExtra(EXTRA_TASK, target.taskId)

        fun targetFrom(intent: Intent, now: Long): InboxNoticeTarget? {
            val key = intent.getStringExtra(EXTRA_KEY).orEmpty()
            val ref = InboxRef.fromKey(key, intent.getLongExtra(EXTRA_GENERATION, -1)) ?: return null
            return InboxNoticeTarget(ref, intent.getStringExtra(EXTRA_EVENT).orEmpty(), intent.getStringExtra(EXTRA_KIND).orEmpty(),
                intent.getLongExtra(EXTRA_THROUGH, -1), intent.getLongExtra(EXTRA_EXPIRY, 0),
                intent.getStringExtra(EXTRA_MESSAGE), intent.getStringExtra(EXTRA_TASK)).takeIf { it.isValid(now) }
        }

        fun fromPush(data: Map<String, String>): InboxAlert? {
            if (data["kind"] != "inbox") return null
            val ref = InboxRef.fromKey(data["key"].orEmpty(), data["generation"]?.toLongOrNull() ?: return null) ?: return null
            val expiresAt = data["expiresAt"]?.toLongOrNull() ?: return null
            val event = data["notificationId"]?.takeIf { it.length in 1..128 } ?: return null
            val kind = data["eventKind"]?.takeIf { it in alertKinds } ?: return null
            val through = data["through"]?.toLongOrNull()?.takeIf { it >= 0 } ?: return null
            return InboxAlert(ref, event, kind, through, "", "", expiresAt, data["pendingActionRevision"]?.toLongOrNull(),
                failureRevision = data["failureRevision"]?.toLongOrNull(),
                messageId = data["messageId"], taskId = data["taskId"]).takeIf { it.target().isValid(System.currentTimeMillis()) }
        }

        private val alertKinds = setOf("dm", "mention", "reply", "input", "approval", "failure", "completion", "message")
    }
}

/** Cloud deliveries arrive after the server's delay. Local-only Assistant events use the same delay on their source device. */
internal object AssistantInboxAlerts {
    private const val CHANNEL = "lingxi_inbox"
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private data class Scheduled(val alert: InboxAlert, val job: Job)
    private val scheduled = mutableMapOf<String, Scheduled>()

    fun receive(context: Context, uid: String, alert: InboxAlert) {
        scope.launch { deliver(context.applicationContext, uid, alert) }
    }

    fun local(context: Context, uid: String, alert: InboxAlert) {
        val slot = if (alert.kind == "completion") "completion:${alert.ref.key}" else alert.event
        scheduled.remove(slot)?.job?.cancel()
        val job = scope.launch {
            delay((alert.deliverAfter - System.currentTimeMillis()).coerceAtLeast(1))
            scheduled.remove(slot)
            deliver(context, uid, alert)
        }
        scheduled[slot] = Scheduled(alert, job)
    }

    fun cancelRead(ref: InboxRef, through: Long, observedActionRevision: Long?, observedFailureRevision: Long?) {
        for ((key, event) in scheduled.toMap()) {
            val action = event.alert.kind in setOf("input", "approval")
            val observedAction = event.alert.pendingActionRevision?.let { observedActionRevision != null && observedActionRevision >= it } == true
            val observedFailure = event.alert.failureRevision?.let { observedFailureRevision == it } ?: true
            if (event.alert.ref == ref && event.alert.through <= through && (!action || observedAction) && observedFailure) {
                event.job.cancel()
                scheduled.remove(key)
            }
        }
    }

    fun cancelPending(ref: InboxRef) {
        for ((key, event) in scheduled.toMap()) {
            if (event.alert.ref == ref && event.alert.kind in setOf("input", "approval")) {
                event.job.cancel()
                scheduled.remove(key)
            }
        }
    }

    fun clear(context: Context) {
        scope.launch {
            scheduled.values.forEach { it.job.cancel() }
            scheduled.clear()
        }
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.activeNotifications.filter { it.tag == CHANNEL }.forEach { manager.cancel(it.tag, it.id) }
    }

    private suspend fun deliver(context: Context, uid: String, alert: InboxAlert) {
        val now = System.currentTimeMillis()
        val inbox = InboxController.get(context)
        val prefs = NotificationPrefsStore(context).load()
        if (!inbox.isNotificationOwner(uid) || alert.expiresAt <= now || !prefs.enabled) return
        if (alert.kind in setOf("input", "approval") && !prefs.inputNeededNotifEnabled) return
        if (alert.kind == "completion" && !prefs.taskCompleteNotifEnabled) return
        if (alert.kind !in setOf("input", "approval") && alert.failureRevision == null && inbox.isRead(alert.ref, alert.through)) return
        val first = runCatching { withContext(Dispatchers.IO) { inbox.store.admitDelivery(uid, alert.target().ledgerKey(), now) } }.getOrDefault(false)
        if (!first || !inbox.isNotificationOwner(uid)) return
        // The account or foreground conversation may have changed while the event was claimed.
        when (inbox.alertDisposition(alert.ref)) {
            InboxAlertDisposition.Inline -> Unit
            InboxAlertDisposition.Banner -> inbox.showBanner(alert)
            InboxAlertDisposition.System -> if (NotificationManagerCompat.from(context).areNotificationsEnabled()) show(context, alert)
        }
    }

    private fun show(context: Context, alert: InboxAlert) {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL, context.getString(R.string.inbox_notification_title), NotificationManager.IMPORTANCE_DEFAULT))
        val id = alert.event.hashCode()
        val intent = InboxAlert.addTarget(
            Intent(context, MainActivity::class.java).setAction(InboxAlert.OPEN_ACTION)
                .setData(Uri.Builder().scheme("lingxi-internal").authority("inbox").appendPath(alert.ref.key).appendPath(alert.event).build())
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP),
            alert.target(),
        )
        val open = PendingIntent.getActivity(context, id, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val title = alert.title.ifBlank { context.getString(R.string.inbox_notification_title) }
        val body = alert.body.ifBlank { context.getString(R.string.inbox_notification_body) }
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentTitle(title).setContentText(body).setContentIntent(open).setAutoCancel(true)
            .setTimeoutAfter((alert.expiresAt - System.currentTimeMillis()).coerceAtLeast(1)).build()
        try { manager.notify(CHANNEL, id, notification) } catch (_: SecurityException) { }
    }
}
