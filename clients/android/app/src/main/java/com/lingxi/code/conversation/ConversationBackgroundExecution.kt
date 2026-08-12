package com.lingxi.code.conversation

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import com.lingxi.code.MainActivity
import com.lingxi.code.R
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

/** Platform lease for a user-started conversation turn. */
fun interface ConversationBackgroundExecution {
    fun setTurnActive(active: Boolean)

    companion object {
        val None = ConversationBackgroundExecution { }
    }
}

/**
 * Starts the foreground service while the Activity is visible, at the same
 * instant the user starts a turn. This satisfies Android 12+'s background-start
 * restriction and keeps the existing ViewModel coroutine eligible to run after
 * Home/app switching.
 */
class AndroidConversationBackgroundExecution(context: Context) : ConversationBackgroundExecution {
    private val appContext = context.applicationContext
    private val mainHandler = Handler(Looper.getMainLooper())
    private var promotionToken: Long? = null
    private val lease = ConversationServiceLease(
        startService = ::startService,
        stopService = ::stopService,
        scheduleRetry = { retry -> mainHandler.postDelayed(retry, RETRY_DELAY_MS) },
    )

    override fun setTurnActive(active: Boolean) {
        lease.setTurnActive(active)
    }

    private fun startService(): Boolean {
        promotionToken?.let(ConversationTurnServicePromotion::unregister)
        val token = ConversationTurnServicePromotion.nextToken()
        promotionToken = token
        ConversationTurnServicePromotion.register(token) { success ->
            if (promotionToken != token) return@register
            ConversationTurnServicePromotion.unregister(token)
            promotionToken = null
            lease.onPromotionResult(success)
        }
        return runCatching {
            ContextCompat.startForegroundService(
                appContext,
                Intent(appContext, ConversationTurnService::class.java)
                    .putExtra(ConversationTurnService.EXTRA_PROMOTION_TOKEN, token),
            )
        }.onFailure {
            ConversationTurnServicePromotion.unregister(token)
            if (promotionToken == token) promotionToken = null
            Log.w(TAG, "Unable to start conversation foreground service", it)
        }.isSuccess
    }

    private fun stopService() {
        promotionToken?.let(ConversationTurnServicePromotion::unregister)
        promotionToken = null
        appContext.stopService(Intent(appContext, ConversationTurnService::class.java))
    }

    private companion object {
        const val TAG = "ConversationBackground"
        const val RETRY_DELAY_MS = 1_000L
    }
}

/**
 * State machine for a foreground-service lease. A successful start request is
 * only "pending"; the lease becomes active after the service confirms that
 * [ServiceCompat.startForeground] succeeded. Failed promotion is retried a
 * small, bounded number of times and never leaves a false active state behind.
 */
internal class ConversationServiceLease(
    private val startService: () -> Boolean,
    private val stopService: () -> Unit,
    private val scheduleRetry: (() -> Unit) -> Unit,
    private val maxPromotionRetries: Int = 2,
) : ConversationBackgroundExecution {
    private var desiredActive = false
    private var retryCount = 0

    var isActive: Boolean = false
        private set
    var isStartPending: Boolean = false
        private set

    override fun setTurnActive(active: Boolean) {
        if (!active) {
            desiredActive = false
            retryCount = 0
            isActive = false
            isStartPending = false
            stopService()
            return
        }
        if (!desiredActive) retryCount = 0
        desiredActive = true
        ensureStarted()
    }

    fun onPromotionResult(success: Boolean) {
        if (!isStartPending) return
        isStartPending = false
        if (!desiredActive) {
            if (success) stopService()
            return
        }
        isActive = success
        if (success) {
            retryCount = 0
        } else {
            scheduleRetry()
        }
    }

    private fun ensureStarted() {
        if (!desiredActive || isActive || isStartPending) return
        // Mark pending before the request so even a synchronous test/fake (or
        // an unusually fast callback) cannot race the acknowledgement.
        isStartPending = true
        if (!startService() && isStartPending) {
            isStartPending = false
            scheduleRetry()
        }
    }

    private fun scheduleRetry() {
        if (!desiredActive || retryCount >= maxPromotionRetries) return
        retryCount++
        scheduleRetry {
            if (desiredActive && !isActive && !isStartPending) ensureStarted()
        }
    }
}

/** One-shot, process-local acknowledgement channel keyed by a start request. */
private object ConversationTurnServicePromotion {
    private val nextToken = AtomicLong(1L)
    private val listeners = ConcurrentHashMap<Long, (Boolean) -> Unit>()

    fun nextToken(): Long = nextToken.getAndIncrement()

    fun register(token: Long, listener: (Boolean) -> Unit) {
        listeners[token] = listener
    }

    fun unregister(token: Long) {
        listeners.remove(token)
    }

    fun report(token: Long, success: Boolean): Boolean {
        val listener = listeners.remove(token) ?: return false
        listener(success)
        return true
    }
}

/** A promoted service without its requesting lease is an orphan and must stop. */
internal fun shouldStopConversationService(
    promoted: Boolean,
    promotionAcknowledged: Boolean,
): Boolean = !promoted || !promotionAcknowledged

/**
 * Process-liveness service only: the turn and its explicit Stop semantics remain
 * owned by [ChatViewModel]. Service shutdown never cancels or settles the LLM.
 */
class ConversationTurnService : Service() {
    override fun onCreate() {
        super.onCreate()
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_ID,
                getString(R.string.chat_background_service_channel),
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = getString(R.string.chat_background_service_channel_description)
                setShowBadge(false)
            },
        )
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val promotionToken = intent?.getLongExtra(EXTRA_PROMOTION_TOKEN, NO_PROMOTION_TOKEN)
            ?: NO_PROMOTION_TOKEN
        val foregroundType = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
        } else {
            0
        }
        val promoted = runCatching {
            ServiceCompat.startForeground(
                this,
                NOTIFICATION_ID,
                buildNotification(),
                foregroundType,
            )
        }.onFailure {
            Log.w(TAG, "Unable to promote conversation foreground service", it)
        }.isSuccess
        val promotionAcknowledged = promotionToken != NO_PROMOTION_TOKEN &&
            ConversationTurnServicePromotion.report(promotionToken, promoted)
        if (shouldStopConversationService(promoted, promotionAcknowledged)) stopSelf(startId)
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun buildNotification() = NotificationCompat.Builder(this, CHANNEL_ID)
        .setSmallIcon(R.mipmap.ic_launcher_foreground)
        .setContentTitle(getString(R.string.chat_background_service_title))
        .setContentText(getString(R.string.chat_background_service_text))
        .setCategory(NotificationCompat.CATEGORY_SERVICE)
        .setOngoing(true)
        .setSilent(true)
        .setContentIntent(
            PendingIntent.getActivity(
                this,
                0,
                Intent(this, MainActivity::class.java)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            ),
        )
        .build()

    companion object {
        internal const val EXTRA_PROMOTION_TOKEN = "conversation_promotion_token"
        private const val NO_PROMOTION_TOKEN = -1L
        private const val TAG = "ConversationTurnService"
        private const val CHANNEL_ID = "conversation_turn"
        private const val NOTIFICATION_ID = 0x4348
    }
}
