package com.lingxi.code.conversation

import android.Manifest
import android.app.ActivityManager
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import com.lingxi.code.MainActivity
import com.lingxi.code.R
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.TaskStatusDto
import com.lingxi.code.bindings.TurnRecoveryStateDto
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.sessionModeFromWireValue
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

data class ConversationBackgroundSnapshot(
    val sessionId: String,
    val turnId: Long?,
    val statusText: String?,
    val recoverySpec: ConversationRecoverySpec? = null,
    val activeTaskIds: Set<String> = emptySet(),
    /** True only while this source still owns a live engine turn executor. */
    val executorActive: Boolean = false,
)

enum class ConversationBackgroundAlert {
    Completed,
    Failed,
    WaitingForUser,
    PausedRecoverable,
}

/** Platform lease for a user-started conversation turn. */
interface ConversationBackgroundExecution {
    fun setTurnActive(active: Boolean)
    fun updateTurn(snapshot: ConversationBackgroundSnapshot?) {}
    fun finishTurn(
        snapshot: ConversationBackgroundSnapshot,
        outcome: ConversationTurnOutcome,
    ) {}
    fun notifyWaitingForUser(snapshot: ConversationBackgroundSnapshot) {}
    fun notifyPausedRecoverable(snapshot: ConversationBackgroundSnapshot) {}
    fun retainAfterUiDestroyed(
        source: ConversationSource,
        snapshot: ConversationBackgroundSnapshot,
    ): Boolean = false

    companion object {
        val None = object : ConversationBackgroundExecution {
            override fun setTurnActive(active: Boolean) = Unit
        }
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
    private var latestSnapshot: ConversationBackgroundSnapshot? = null
    private val deliveredAlertTags = ConcurrentHashMap.newKeySet<String>()
    private val lease = ConversationServiceLease(
        startService = ::startService,
        stopService = ::stopService,
        scheduleRetry = { retry -> mainHandler.postDelayed(retry, RETRY_DELAY_MS) },
    )

    override fun setTurnActive(active: Boolean) {
        lease.setTurnActive(active)
    }

    override fun updateTurn(snapshot: ConversationBackgroundSnapshot?) {
        latestSnapshot = snapshot
        if (snapshot == null || (!lease.isActive && !lease.isStartPending)) return
        // `isStartPending` means the START was submitted but promotion has NOT
        // been acknowledged yet, so the service may not be a foreground service
        // at all. A plain `startService` from a backgrounded process then
        // throws (Android 12+ raises `BackgroundServiceStartNotAllowedException`,
        // an IllegalStateException) — and this is a status refresh, never worth
        // taking the process down for. The next ACTION_START/ACTION_UPDATE
        // carries `latestSnapshot`, which was already stored above.
        runCatching {
            appContext.startService(
                ConversationTurnService.intent(
                    context = appContext,
                    action = ConversationTurnService.ACTION_UPDATE,
                    snapshot = snapshot,
                ),
            )
        }.onFailure { Log.w(TAG, "Unable to refresh conversation foreground notification", it) }
    }

    override fun finishTurn(
        snapshot: ConversationBackgroundSnapshot,
        outcome: ConversationTurnOutcome,
    ) {
        val alert = when (outcome) {
            ConversationTurnOutcome.Completed -> ConversationBackgroundAlert.Completed
            ConversationTurnOutcome.Failed -> ConversationBackgroundAlert.Failed
            ConversationTurnOutcome.Cancelled -> return
        }
        postAlert(snapshot, alert)
    }

    override fun notifyWaitingForUser(snapshot: ConversationBackgroundSnapshot) {
        postAlert(snapshot, ConversationBackgroundAlert.WaitingForUser)
    }

    override fun notifyPausedRecoverable(snapshot: ConversationBackgroundSnapshot) {
        postAlert(snapshot, ConversationBackgroundAlert.PausedRecoverable)
    }

    override fun retainAfterUiDestroyed(
        source: ConversationSource,
        snapshot: ConversationBackgroundSnapshot,
    ): Boolean {
        val engine = (source as? BackgroundRetainableConversationSource)
            ?.engineSourceForBackgroundRetention()
            ?: return false
        ConversationHeadlessRecovery.monitorExisting(appContext, snapshot, engine)
        return true
    }

    private fun postAlert(
        snapshot: ConversationBackgroundSnapshot,
        alert: ConversationBackgroundAlert,
    ) {
        if (isApplicationForeground()) return
        if (
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                ContextCompat.checkSelfPermission(
                    appContext,
                    Manifest.permission.POST_NOTIFICATIONS,
                ) != PackageManager.PERMISSION_GRANTED
        ) {
            Log.i(TAG, "Terminal conversation notification suppressed: permission denied")
            return
        }
        val manager = appContext.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                TERMINAL_CHANNEL_ID,
                appContext.getString(R.string.chat_background_result_channel),
                NotificationManager.IMPORTANCE_DEFAULT,
            ).apply {
                description = appContext.getString(R.string.chat_background_result_channel_description)
            },
        )
        val (title, body) = when (alert) {
            ConversationBackgroundAlert.Completed ->
                R.string.chat_background_completed_title to R.string.chat_background_completed_text
            ConversationBackgroundAlert.Failed ->
                R.string.chat_background_failed_title to R.string.chat_background_failed_text
            ConversationBackgroundAlert.WaitingForUser ->
                R.string.chat_background_waiting_title to R.string.chat_background_waiting_text
            ConversationBackgroundAlert.PausedRecoverable ->
                R.string.chat_background_paused_title to R.string.chat_background_paused_text
        }
        val tag = "${snapshot.sessionId}:${snapshot.turnId}:${alert.name}"
        if (!deliveredAlertTags.add(tag)) return
        val notification = NotificationCompat.Builder(appContext, TERMINAL_CHANNEL_ID)
            .setSmallIcon(R.mipmap.ic_launcher_foreground)
            .setContentTitle(appContext.getString(title))
            .setContentText(appContext.getString(body))
            .setCategory(NotificationCompat.CATEGORY_STATUS)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(
                PendingIntent.getActivity(
                    appContext,
                    tag.hashCode(),
                    ConversationNotificationRoute.openIntent(
                        appContext,
                        snapshot.sessionId,
                        snapshot.turnId,
                        snapshot.recoverySpec,
                    ),
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
                ),
            )
            .build()
        runCatching {
            NotificationManagerCompat.from(appContext)
                .notify(tag, TERMINAL_NOTIFICATION_ID, notification)
        }.onFailure {
            deliveredAlertTags.remove(tag)
            Log.w(TAG, "Unable to post terminal conversation notification", it)
        }
    }

    private fun isApplicationForeground(): Boolean {
        val state = ActivityManager.RunningAppProcessInfo()
        ActivityManager.getMyMemoryState(state)
        return state.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND
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
                ConversationTurnService.intent(
                    context = appContext,
                    action = ConversationTurnService.ACTION_START,
                    snapshot = latestSnapshot,
                ).putExtra(ConversationTurnService.EXTRA_PROMOTION_TOKEN, token),
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
        latestSnapshot = null
        appContext.stopService(Intent(appContext, ConversationTurnService::class.java))
    }

    private companion object {
        const val TAG = "ConversationBackground"
        const val RETRY_DELAY_MS = 1_000L
        const val TERMINAL_CHANNEL_ID = "conversation_result"
        const val TERMINAL_NOTIFICATION_ID = 0x4349
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
    action: String?,
    promoted: Boolean,
    promotionAcknowledged: Boolean,
    snapshot: ConversationBackgroundSnapshot?,
): Boolean {
    if (!promoted) return true
    return when (action) {
        ConversationTurnService.ACTION_START ->
            !promotionAcknowledged && snapshot == null
        ConversationTurnService.ACTION_UPDATE ->
            snapshot == null
        else -> false
    }
}

/** Resolve both the current action field and the legacy extra for redelivery. */
internal fun conversationServiceAction(intent: Intent?): String? =
    intent?.action ?: intent?.getStringExtra(ConversationTurnService.EXTRA_ACTION)

/**
 * Process-level owner used only when Android redelivers the foreground service
 * after reclaiming the app process. It reconstructs the same workspace engine,
 * resumes the session, then lets EngineConversationSource perform Attach/Resume
 * from its durable client cursor. A UI-created source is registered here too,
 * so service-only restarts reuse it instead of creating a second engine.
 */
internal object ConversationHeadlessRecovery {
    internal sealed interface UiSourceClaim {
        data class Existing(val source: EngineConversationSource) : UiSourceClaim
        data class Pending(val source: ConversationSource) : UiSourceClaim
        data object Build : UiSourceClaim
    }

    internal class RecoveryOwner(
        val recoverySpec: ConversationRecoverySpec,
    ) {
        val pending = CompletableDeferred<EngineConversationSource?>()
        @Volatile var source: EngineConversationSource? = null
        @Volatile var monitorJob: Job? = null
        @Volatile var pendingCancellation: ConversationBackgroundSnapshot? = null
        val cancellationInFlight = java.util.concurrent.atomic.AtomicBoolean(false)
        val recoveryReady = CompletableDeferred<Unit>()
        val turnAttached = CompletableDeferred<Unit>()
        @Volatile var awaitingRecoveryAttach = false
        @Volatile var headlessExecutorActive = false
        @Volatile var uiAttachResumeRequired = true
        private val claimedByUi = java.util.concurrent.atomic.AtomicBoolean(false)

        fun claimForUi() {
            val wasAlreadyUiOwned = claimedByUi.getAndSet(true)
            val hadHeadlessExecutor = headlessExecutorActive
            headlessExecutorActive = false
            monitorJob?.cancel()
            monitorJob = null
            if (!wasAlreadyUiOwned) {
                uiAttachResumeRequired = shouldResumeUiAttach(
                    hadHeadlessExecutor = hadHeadlessExecutor,
                    wasAlreadyUiOwned = false,
                )
                source?.setUiAttachResumeRequired(uiAttachResumeRequired)
            }
            source?.claimDurableTurnForUi()
        }

        fun releaseToBackground() {
            claimedByUi.set(false)
            source?.releaseDurableTurnToHeadless()
        }

        fun isClaimedByUi(): Boolean = claimedByUi.get()

        fun complete(source: EngineConversationSource?) {
            this.source = source
            if (!pending.isCompleted) pending.complete(source)
        }
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val owners = ConcurrentHashMap<String, RecoveryOwner>()
    private val pendingCancellations = ConcurrentHashMap<String, ConversationBackgroundSnapshot>()

    fun existing(scopeKey: String): EngineConversationSource? = owners[scopeKey]?.source

    /**
     * Atomically acquire the process source for a UI owner. Without this
     * reservation two Compose/reconnect callers can both observe an empty map
     * and build independent native engines before either one registers.
     */
    @Synchronized
    fun acquireForUi(
        recoverySpec: ConversationRecoverySpec,
        strings: ConversationStrings,
    ): UiSourceClaim {
        val scopeKey = recoverySpec.scopeKey
        val owner = owners[scopeKey]
        owner?.source?.let {
            owner.claimForUi()
            return UiSourceClaim.Existing(it)
        }
        if (owner != null && !owner.pending.isCompleted) {
            owner.claimForUi()
            return UiSourceClaim.Pending(
                RecoveringConversationSource(
                    pendingSource = owner.pending,
                    strings = strings,
                    recoverySpec = owner.recoverySpec,
                ),
            )
        }
        if (owner != null) owners.remove(scopeKey, owner)
        val reservation = RecoveryOwner(recoverySpec)
        reservation.claimForUi()
        owners[scopeKey] = reservation
        return UiSourceClaim.Build
    }

    fun pendingForUi(
        scopeKey: String,
        strings: ConversationStrings,
    ): ConversationSource? {
        val owner = owners[scopeKey] ?: return null
        if (owner.source != null || owner.pending.isCompleted) return null
        owner.claimForUi()
        return RecoveringConversationSource(
            pendingSource = owner.pending,
            strings = strings,
            recoverySpec = owner.recoverySpec,
        )
    }

    fun claimForUi(scopeKey: String) {
        owners[scopeKey]?.claimForUi()
    }

    /** A failed Attach/Resume leaves no headless executor to suppress UI Resume. */
    fun markDurableAttachFailed(scopeKey: String, source: EngineConversationSource) {
        owners[scopeKey]?.takeIf { it.source == source }?.let { owner ->
            owner.headlessExecutorActive = false
        }
    }

    /** Consume the first UI Resume requirement after its command succeeds. */
    fun markDurableUiExecutorActive(scopeKey: String, source: EngineConversationSource) {
        owners[scopeKey]?.takeIf {
            it.source == source && it.isClaimedByUi()
        }?.let { owner ->
            owner.uiAttachResumeRequired = false
        }
    }

    @Synchronized
    fun releaseUiReservation(scopeKey: String) {
        val owner = owners[scopeKey] ?: return
        if (owner.source == null && owner.isClaimedByUi()) {
            owners.remove(scopeKey, owner)
        }
    }

    fun register(scopeKey: String, source: EngineConversationSource) {
        val owner = owners.computeIfAbsent(scopeKey) { RecoveryOwner(source.recoverySpec) }
        owner.pendingCancellation = pendingCancellations.remove(scopeKey)
            ?: owner.pendingCancellation
        owner.complete(source)
        if (owner.isClaimedByUi()) {
            // A pending UI claim happens before the headless source is
            // registered. Reapply the dynamic disposition here; the source's
            // construction flag is false for the headless implementation, but
            // a cold UI takeover still owns the first Resume.
            source.setUiAttachResumeRequired(owner.uiAttachResumeRequired)
            source.claimDurableTurnForUi()
        }
    }

    fun unregister(scopeKey: String, source: EngineConversationSource) {
        val owner = owners[scopeKey] ?: return
        if (owner.source == source && owners.remove(scopeKey, owner)) {
            owner.monitorJob?.cancel()
        }
    }

    fun recover(context: Context, snapshot: ConversationBackgroundSnapshot) {
        val spec = snapshot.recoverySpec ?: return
        val owner = RecoveryOwner(spec)
        val pendingCancellation = pendingCancellations.remove(spec.scopeKey)
        owner.pendingCancellation = pendingCancellation
        if (owners.putIfAbsent(spec.scopeKey, owner) != null) {
            owners[spec.scopeKey]?.let { existing ->
                existing.pendingCancellation = pendingCancellation
                    ?: pendingCancellations.remove(spec.scopeKey)
                    ?: existing.pendingCancellation
                existing.source?.let { source ->
                    submitPendingCancellationIfMonitored(existing, source, context)
                }
            }
            return
        }
        scope.launch {
            try {
                val source = EngineConversationSource.create(
                    context = context.applicationContext,
                    projectWorkspace = spec.projectWorkspace(),
                    workspaceKey = spec.workspaceKey,
                    sessionMode = spec.sessionMode,
                    linuxRuntimeMode = spec.linuxRuntimeMode,
                    reuseProcessSource = false,
                ) as? EngineConversationSource ?: run {
                    owners.remove(spec.scopeKey, owner)
                    owner.complete(null)
                    context.stopService(Intent(context, ConversationTurnService::class.java))
                    return@launch
                }
                startMonitor(
                    context = context,
                    snapshot = snapshot,
                    source = source,
                    minimumActionStateIndex = 2L,
                    resumeSession = true,
                )
                submitPendingCancellationIfMonitored(owner, source, context)
            } catch (error: Throwable) {
                owners.remove(spec.scopeKey, owner)
                owner.complete(null)
                Log.w("ConversationRecovery", "Unable to rebuild engine after service redelivery", error)
                context.stopService(Intent(context, ConversationTurnService::class.java))
            }
        }
    }

    fun monitorExisting(
        context: Context,
        snapshot: ConversationBackgroundSnapshot,
        source: EngineConversationSource,
    ) {
        owners[source.recoverySpec.scopeKey]?.releaseToBackground()
        startMonitor(
            context = context,
            snapshot = snapshot,
            source = source,
            minimumActionStateIndex = 1L,
            resumeSession = false,
            retainedExecutorActive = snapshot.executorActive,
        )
    }

    /**
     * Submit a notification Stop through the engine and keep the service alive
     * until the correlated recovery terminal event reaches the monitor. The
     * request is held if service redelivery is still rebuilding the source.
     */
    fun requestCancellation(
        context: Context,
        snapshot: ConversationBackgroundSnapshot,
    ): Boolean {
        val turnId = snapshot.turnId ?: return false
        val spec = snapshot.recoverySpec ?: return false
        val owner = owners[spec.scopeKey]
        if (owner == null) {
            pendingCancellations[spec.scopeKey] = snapshot
            recover(context, snapshot)
            return true
        }
        owner.pendingCancellation = snapshot
        owner.source?.let { source ->
            if (owner.monitorJob?.isActive != true) {
                startMonitor(
                    context = context,
                    snapshot = snapshot,
                    source = source,
                    minimumActionStateIndex = 1L,
                    resumeSession = false,
                    allowUiOwned = true,
                )
            }
            submitPendingCancellationIfMonitored(owner, source, context)
        }
        return true
    }

    private fun submitPendingCancellationIfMonitored(
        owner: RecoveryOwner,
        source: EngineConversationSource,
        context: Context,
    ) {
        val snapshot = owner.pendingCancellation ?: return
        if (owner.monitorJob?.isActive != true) {
            startMonitor(
                context = context,
                snapshot = snapshot,
                source = source,
                minimumActionStateIndex = 1L,
                resumeSession = false,
                allowUiOwned = true,
            )
        }
        if (!owner.recoveryReady.isCompleted) {
            scope.launch {
                runCatching {
                    owner.recoveryReady.await()
                    if (owner.awaitingRecoveryAttach) owner.turnAttached.await()
                }
                    .onSuccess { submitPendingCancellation(owner, source) }
            }
            return
        }
        submitPendingCancellation(owner, source)
    }

    private fun submitPendingCancellation(
        owner: RecoveryOwner,
        source: EngineConversationSource,
    ) {
        val snapshot = owner.pendingCancellation ?: return
        val turnId = snapshot.turnId ?: return
        if (!owner.cancellationInFlight.compareAndSet(false, true)) return
        scope.launch {
            runCatching { source.discardDurableTurn(turnId) }
                .onSuccess {
                    owner.pendingCancellation = null
                }
                .onFailure {
                    // Keep the request for a subsequent notification tap or
                    // source recovery; the foreground service must not be
                    // stopped while the engine has not acknowledged Cancel.
                    Log.w("ConversationRecovery", "Unable to cancel notification turn", it)
                }
            owner.cancellationInFlight.set(false)
        }
    }

    private fun startMonitor(
        context: Context,
        snapshot: ConversationBackgroundSnapshot,
        source: EngineConversationSource,
        minimumActionStateIndex: Long,
        resumeSession: Boolean,
        allowUiOwned: Boolean = false,
        retainedExecutorActive: Boolean = false,
    ) {
        val turnId = snapshot.turnId
        if (turnId == null && snapshot.activeTaskIds.isEmpty()) return
        val spec = snapshot.recoverySpec ?: source.recoverySpec
        val owner = owners[spec.scopeKey] ?: return
        if (owner.isClaimedByUi() && !allowUiOwned) {
            context.stopService(Intent(context, ConversationTurnService::class.java))
            return
        }
        if (owner.monitorJob?.isActive == true) return
        if (!owner.isClaimedByUi() && retainedExecutorActive) {
            // This is the UI's still-live executor being handed to the
            // background owner, not a speculative cold-service lease.
            owner.headlessExecutorActive = true
        }
        if (resumeSession) owner.awaitingRecoveryAttach = true
        val alerts = AndroidConversationBackgroundExecution(context)
        val observedRecoverySignal = java.util.concurrent.atomic.AtomicBoolean(false)
        val recoveryStateCount = AtomicLong(0L)
        val activeTaskIds = ConcurrentHashMap.newKeySet<String>().apply {
            addAll(snapshot.activeTaskIds)
        }
        val job = scope.launch(start = CoroutineStart.UNDISPATCHED) {
            val collector = launch(start = CoroutineStart.UNDISPATCHED) {
                source.clientEvents.collect { event ->
                    val current = snapshot.copy(
                        statusText = context.getString(R.string.chat_background_service_text),
                    )
                    if (event is ClientEvent.TurnRecoveryState && turnId != null) {
                        val recovery = event.snapshot
                        if (recovery.turnId.toLong() != turnId) return@collect
                        owner.turnAttached.complete(Unit)
                        observedRecoverySignal.set(true)
                        val stateIndex = recoveryStateCount.incrementAndGet()
                        if (stateIndex >= minimumActionStateIndex) {
                            owner.headlessExecutorActive =
                                headlessExecutorActiveAfterRecoveryState(
                                    currentActive = owner.headlessExecutorActive,
                                    recoveryState = recovery.state,
                                    resumeSession = resumeSession,
                                    stateIndex = stateIndex,
                                    minimumActionStateIndex = minimumActionStateIndex,
                                )
                        }
                        when (recovery.state) {
                            TurnRecoveryStateDto.RUNNING -> {
                                // Headless recovery runs with the process in the
                                // background. When the ACTION_CANCEL path reached
                                // here after a FAILED foreground promotion (it
                                // deliberately does not stop the service), this
                                // plain `startService` throws
                                // `BackgroundServiceStartNotAllowedException` —
                                // inside the `clientEvents` collector, which would
                                // tear down the whole recovery monitor over a
                                // notification-text refresh.
                                runCatching {
                                    context.startService(
                                        ConversationTurnService.intent(
                                            context,
                                            ConversationTurnService.ACTION_UPDATE,
                                            current,
                                        ),
                                    )
                                }.onFailure {
                                    Log.w(
                                        "ConversationRecovery",
                                        "Unable to refresh recovery notification",
                                        it,
                                    )
                                }
                            }
                            TurnRecoveryStateDto.WAITING_FOR_USER ->
                                if (stateIndex >= minimumActionStateIndex) {
                                    alerts.notifyWaitingForUser(current)
                                    settle(
                                        spec.scopeKey,
                                        context,
                                        retainExecutorOwnership = owner.headlessExecutorActive,
                                    )
                                }
                            TurnRecoveryStateDto.PAUSED_RECOVERABLE ->
                                if (stateIndex >= minimumActionStateIndex) {
                                    alerts.notifyPausedRecoverable(current)
                                    settle(spec.scopeKey, context)
                                }
                            TurnRecoveryStateDto.COMPLETED ->
                                if (stateIndex >= minimumActionStateIndex) {
                                    alerts.finishTurn(current, ConversationTurnOutcome.Completed)
                                    if (activeTaskIds.isEmpty()) settle(spec.scopeKey, context)
                                }
                            TurnRecoveryStateDto.FAILED ->
                                if (stateIndex >= minimumActionStateIndex) {
                                    alerts.finishTurn(current, ConversationTurnOutcome.Failed)
                                    if (activeTaskIds.isEmpty()) settle(spec.scopeKey, context)
                                }
                            TurnRecoveryStateDto.CANCELLED ->
                                if (stateIndex >= minimumActionStateIndex && activeTaskIds.isEmpty()) {
                                    settle(spec.scopeKey, context)
                                }
                        }
                        return@collect
                    }
                    val taskStatus = when (event) {
                        is ClientEvent.TaskStatusChanged -> event.taskId to event.status
                        is ClientEvent.TaskRow -> event.task.taskId to event.task.status
                        else -> null
                    } ?: return@collect
                    if (taskStatus.first !in activeTaskIds) return@collect
                    observedRecoverySignal.set(true)
                    when (taskStatus.second) {
                        TaskStatusDto.PENDING, TaskStatusDto.RUNNING -> Unit
                        TaskStatusDto.COMPLETED -> {
                            activeTaskIds.remove(taskStatus.first)
                            alerts.finishTurn(current, ConversationTurnOutcome.Completed)
                        }
                        TaskStatusDto.FAILED -> {
                            activeTaskIds.remove(taskStatus.first)
                            alerts.finishTurn(current, ConversationTurnOutcome.Failed)
                        }
                        TaskStatusDto.PAUSED -> {
                            activeTaskIds.remove(taskStatus.first)
                            alerts.notifyPausedRecoverable(current)
                        }
                        TaskStatusDto.CANCELLED -> activeTaskIds.remove(taskStatus.first)
                    }
                    if (activeTaskIds.isEmpty() && (turnId == null || recoveryStateCount.get() > 0L)) {
                        settle(spec.scopeKey, context)
                    }
                }
            }
            if (resumeSession) {
                runCatching { source.resumeSession(snapshot.sessionId) }
                    .onFailure {
                        Log.w("ConversationRecovery", "Unable to resume session after service redelivery", it)
                        owner.recoveryReady.completeExceptionally(it)
                        discard(spec.scopeKey, source, context)
                        return@launch
                    }
                runCatching { source.refreshExecutionStatus() }
                owner.recoveryReady.complete(Unit)
                // A user-requested stop clears the client recovery record. If no
                // Attach state arrives, do not leave an orphan ongoing notification.
                delay(10_000L)
                if (!observedRecoverySignal.get()) {
                    discard(spec.scopeKey, source, context)
                    return@launch
                }
            } else if (!owner.recoveryReady.isCompleted) {
                owner.recoveryReady.complete(Unit)
            }
            collector.join()
        }
        job.invokeOnCompletion {
            if (owner.isClaimedByUi()) {
                context.stopService(Intent(context, ConversationTurnService::class.java))
            }
        }
        owner.monitorJob = job
    }

    private fun settle(
        scopeKey: String,
        context: Context,
        retainExecutorOwnership: Boolean = false,
    ) {
        owners[scopeKey]?.let { owner ->
            if (!retainExecutorOwnership) owner.headlessExecutorActive = false
            owner.monitorJob?.cancel()
            owner.monitorJob = null
        }
        context.stopService(Intent(context, ConversationTurnService::class.java))
    }

    private fun discard(scopeKey: String, source: EngineConversationSource, context: Context) {
        val owner = owners.remove(scopeKey) ?: return run {
            runCatching { source.close() }
            context.stopService(Intent(context, ConversationTurnService::class.java))
        }
        owner.headlessExecutorActive = false
        owner.monitorJob?.cancel()
        owner.recoveryReady.completeExceptionally(
            IllegalStateException("conversation recovery discarded"),
        )
        owner.turnAttached.completeExceptionally(
            IllegalStateException("conversation recovery discarded"),
        )
        owner.complete(null)
        runCatching { source.close() }
        context.stopService(Intent(context, ConversationTurnService::class.java))
    }
}

/** A UI takeover resumes only when no other owner already has the executor. */
internal fun shouldResumeUiAttach(
    hadHeadlessExecutor: Boolean,
    wasAlreadyUiOwned: Boolean,
): Boolean = !hadHeadlessExecutor && !wasAlreadyUiOwned

/**
 * Cold recovery's first state belongs to AttachTurn's snapshot. Only a later
 * authoritative Running state proves that ResumeTurn really owns execution;
 * a Waiting/terminal disposition must remain discardable instead.
 */
internal fun shouldMarkHeadlessExecutorActive(
    resumeSession: Boolean,
    recoveryState: TurnRecoveryStateDto,
    stateIndex: Long,
    minimumActionStateIndex: Long,
): Boolean = recoveryState == TurnRecoveryStateDto.RUNNING &&
    (!resumeSession || stateIndex >= minimumActionStateIndex)

/** Fold each authoritative recovery state into the owner lease. */
internal fun headlessExecutorActiveAfterRecoveryState(
    currentActive: Boolean,
    recoveryState: TurnRecoveryStateDto,
    resumeSession: Boolean,
    stateIndex: Long,
    minimumActionStateIndex: Long,
): Boolean {
    if (stateIndex < minimumActionStateIndex) return currentActive
    return when (recoveryState) {
        TurnRecoveryStateDto.RUNNING -> currentActive || shouldMarkHeadlessExecutorActive(
            resumeSession = resumeSession,
            recoveryState = recoveryState,
            stateIndex = stateIndex,
            minimumActionStateIndex = minimumActionStateIndex,
        )
        // Waiting can be reached after a live recovered executor resumes. Do
        // not confuse that parked logical owner with a cold Waiting checkpoint.
        TurnRecoveryStateDto.WAITING_FOR_USER -> currentActive
        TurnRecoveryStateDto.PAUSED_RECOVERABLE,
        TurnRecoveryStateDto.COMPLETED,
        TurnRecoveryStateDto.FAILED,
        TurnRecoveryStateDto.CANCELLED,
        -> false
    }
}

/**
 * Foreground lease and process-redelivery entrypoint. Service shutdown never
 * cancels or settles the LLM; explicit Stop still routes through ClientCommand.
 */
class ConversationTurnService : Service() {
    private var latestSnapshot: ConversationBackgroundSnapshot? = null

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
        // Notification Stop is a private explicit service Intent and carries
        // its verb in Intent.action. Keep reading the legacy extra as a
        // compatibility fallback for redelivered starts from older builds.
        val action = conversationServiceAction(intent)
        val promotionToken = intent?.getLongExtra(EXTRA_PROMOTION_TOKEN, NO_PROMOTION_TOKEN)
            ?: NO_PROMOTION_TOKEN
        if (action == ACTION_CANCEL) {
            val snapshot = intent?.snapshot() ?: latestSnapshot
            if (snapshot != null) latestSnapshot = snapshot
        } else {
            latestSnapshot = intent?.snapshot()
        }
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
        if (action == ACTION_CANCEL) {
            val snapshot = latestSnapshot
            if (snapshot?.turnId != null) {
                // Do not stop the service here, even if foreground promotion
                // failed. The private service action must still submit
                // ClientCommand.Cancel, and ConversationHeadlessRecovery keeps
                // the request alive until the matching terminal checkpoint.
                ConversationHeadlessRecovery.requestCancellation(
                    context = applicationContext,
                    snapshot = snapshot,
                )
            } else {
                // A Stop action without a correlated durable turn cannot safely
                // cancel engine work. There is no active turn to retain for, so
                // only this orphaned service instance is eligible to stop.
                stopSelf(startId)
            }
            // Redelivery preserves the correlated Stop request if Android
            // reclaims the process before the terminal recovery event arrives.
            return START_REDELIVER_INTENT
        }
        if (
            promoted &&
            flags and START_FLAG_REDELIVERY != 0 &&
            latestSnapshot != null
        ) {
            ConversationHeadlessRecovery.recover(applicationContext, latestSnapshot!!)
        }
        if (shouldStopConversationService(action, promoted, promotionAcknowledged, latestSnapshot)) {
            stopSelf(startId)
        }
        return START_REDELIVER_INTENT
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun buildNotification() = NotificationCompat.Builder(this, CHANNEL_ID)
        .setSmallIcon(R.mipmap.ic_launcher_foreground)
        .setContentTitle(getString(R.string.chat_background_service_title))
        .setContentText(latestSnapshot?.statusText ?: getString(R.string.chat_background_service_text))
        .setCategory(NotificationCompat.CATEGORY_SERVICE)
        .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
        .setOngoing(true)
        .setSilent(true)
        .setOnlyAlertOnce(true)
        .setContentIntent(contentIntent())
        .addAction(
            0,
            getString(R.string.common_cancel),
            PendingIntent.getService(
                this,
                1,
                cancelIntent(),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            ),
        )
        .build()

    private fun contentIntent(): PendingIntent =
        PendingIntent.getActivity(
            this,
            0,
            latestSnapshot
                ?.let {
                    ConversationNotificationRoute.openIntent(
                        this,
                        it.sessionId,
                        it.turnId,
                        it.recoverySpec,
                    )
                }
                ?: Intent(this, MainActivity::class.java)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    private fun cancelIntent(): Intent {
        val snapshot = latestSnapshot
        return ConversationNotificationRoute.cancelIntent(
            this,
            sessionId = snapshot?.sessionId.orEmpty(),
            turnId = snapshot?.turnId,
            recoverySpec = snapshot?.recoverySpec,
        )
    }

    companion object {
        internal const val ACTION_START = "start"
        internal const val ACTION_UPDATE = "update"
        internal const val ACTION_CANCEL = "cancel"
        internal const val EXTRA_ACTION = "conversation_action"
        internal const val EXTRA_PROMOTION_TOKEN = "conversation_promotion_token"
        internal const val EXTRA_SESSION_ID = "conversation_session_id"
        internal const val EXTRA_TURN_ID = "conversation_turn_id"
        internal const val EXTRA_STATUS_TEXT = "conversation_status_text"
        internal const val EXTRA_PROJECT_ID = "conversation_project_id"
        internal const val EXTRA_HOST_PATH = "conversation_host_path"
        internal const val EXTRA_SESSION_MODE = "conversation_session_mode"
        internal const val EXTRA_WORKSPACE_KEY = "conversation_workspace_key"
        internal const val EXTRA_LINUX_RUNTIME_MODE = "conversation_linux_runtime_mode"
        internal const val EXTRA_ACTIVE_TASK_IDS = "conversation_active_task_ids"
        private const val NO_PROMOTION_TOKEN = -1L
        private const val TAG = "ConversationTurnService"
        private const val CHANNEL_ID = "conversation_turn"
        private const val NOTIFICATION_ID = 0x4348

        internal fun intent(
            context: Context,
            action: String,
            snapshot: ConversationBackgroundSnapshot?,
        ): Intent = Intent(context, ConversationTurnService::class.java)
            .setAction(action)
            .putExtra(EXTRA_ACTION, action)
            .apply {
                if (snapshot != null) {
                    putExtra(EXTRA_SESSION_ID, snapshot.sessionId)
                    putExtra(EXTRA_STATUS_TEXT, snapshot.statusText)
                    if (snapshot.turnId != null) {
                        putExtra(EXTRA_TURN_ID, snapshot.turnId)
                    } else {
                        removeExtra(EXTRA_TURN_ID)
                    }
                    snapshot.recoverySpec?.let { spec ->
                        putExtra(EXTRA_PROJECT_ID, spec.projectId)
                        putExtra(EXTRA_HOST_PATH, spec.hostPath)
                        putExtra(EXTRA_SESSION_MODE, spec.sessionMode.wireValue)
                        putExtra(EXTRA_LINUX_RUNTIME_MODE, spec.linuxRuntimeMode.name)
                        putExtra(EXTRA_WORKSPACE_KEY, spec.workspaceKey)
                    }
                    putExtra(EXTRA_ACTIVE_TASK_IDS, snapshot.activeTaskIds.toTypedArray())
                }
            }
    }
}

private fun Intent.snapshot(): ConversationBackgroundSnapshot? {
    val sessionId = getStringExtra(ConversationTurnService.EXTRA_SESSION_ID)
        ?.takeIf(String::isNotBlank)
        ?: return null
    val turnId = if (hasExtra(ConversationTurnService.EXTRA_TURN_ID)) {
        getLongExtra(ConversationTurnService.EXTRA_TURN_ID, 0L)
    } else {
        null
    }
    val status = getStringExtra(ConversationTurnService.EXTRA_STATUS_TEXT)
    val sessionMode = sessionModeFromWireValue(
        getStringExtra(ConversationTurnService.EXTRA_SESSION_MODE),
    )
    val runtimeMode = getStringExtra(ConversationTurnService.EXTRA_LINUX_RUNTIME_MODE)
        ?.let { encoded -> LinuxRuntimeMode.entries.firstOrNull { it.name == encoded } }
        ?: LinuxRuntimeMode.Legacy
    val recoverySpec = if (
        hasExtra(ConversationTurnService.EXTRA_LINUX_RUNTIME_MODE) ||
        hasExtra(ConversationTurnService.EXTRA_HOST_PATH) ||
        hasExtra(ConversationTurnService.EXTRA_SESSION_MODE) ||
        hasExtra(ConversationTurnService.EXTRA_WORKSPACE_KEY)
    ) {
        ConversationRecoverySpec(
            projectId = getStringExtra(ConversationTurnService.EXTRA_PROJECT_ID),
            hostPath = getStringExtra(ConversationTurnService.EXTRA_HOST_PATH),
            sessionMode = sessionMode,
            linuxRuntimeMode = runtimeMode,
            workspaceKey = getStringExtra(ConversationTurnService.EXTRA_WORKSPACE_KEY),
        )
    } else {
        null
    }
    return ConversationBackgroundSnapshot(
        sessionId = sessionId,
        turnId = turnId,
        statusText = status,
        recoverySpec = recoverySpec,
        activeTaskIds = getStringArrayExtra(ConversationTurnService.EXTRA_ACTIVE_TASK_IDS)
            ?.toSet()
            .orEmpty(),
    )
}
