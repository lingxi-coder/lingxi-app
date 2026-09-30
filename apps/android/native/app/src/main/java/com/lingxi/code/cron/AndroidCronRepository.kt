package com.lingxi.code.cron

import android.util.Log
import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import androidx.work.WorkManager
import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.cancel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File

/**
 * Process-level Android cron source of truth for UI surfaces. Durable tasks stay
 * in each Engine scope; durable run state stays in [CronRunHistoryStore].
 */
class AndroidCronRepository private constructor(
    context: Context,
    private val scopeScanner: CronScopeScanner,
    private val gateway: CronEngineGateway,
    internal val historyStore: CronRunHistoryStore,
) {
    private val appContext = context.applicationContext
    private val repositoryScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val refreshMutex = Mutex()
    private val mutationMutex = Mutex()
    private val _state = MutableStateFlow(AndroidCronRepositoryState())
    val state: StateFlow<AndroidCronRepositoryState> = _state.asStateFlow()

    init {
        refresh()
        repositoryScope.launch {
            WorkManager.getInstance(appContext)
                .getWorkInfosByTagFlow(CronWorkNames.TAG_EXECUTION)
                .map { infos ->
                    infos
                        .groupingBy { it.state.name }
                        .eachCount()
                }
                .distinctUntilChanged()
                .collectLatest { counts ->
                    _state.update { current ->
                        if (current.workManagerStateCounts == counts) {
                            current
                        } else {
                            current.copy(workManagerStateCounts = counts)
                        }
                    }
                }
        }
    }

    /** Release watchers owned by a repository whose host lifecycle has ended. */
    internal fun dispose() { repositoryScope.cancel() }

    fun refresh() {
        repositoryScope.launch { refreshNow() }
    }

    /**
     * Reconcile from process scope so a Compose destination leaving the
     * composition cannot cancel alarm/work scheduling halfway through.
     */
    fun requestReconcile(reason: String = "ui") {
        repositoryScope.launch {
            try {
                reconcile(reason)
            } catch (cancel: CancellationException) {
                throw cancel
            } catch (_: Throwable) {
                // CronCoordinator already logs the failure and refreshes the
                // repository state so UI surfaces can display the error.
            }
        }
    }

    internal suspend fun refreshNow(lastReconciledAtMs: Long? = null) {
        refreshMutex.withLock {
            _state.value = _state.value.copy(loading = true, errorMessage = null)
            val next = runCronCatching {
                // Terminal results are self-contained. Recover their pending notifications
                // even when the original project or its current task is unavailable.
                postPendingNotifications()
                val scopes = scopeScanner.scan()
                val scopedTasks = loadScopedTasks(scopes)
                historyStore.records().filterNot { it.status.isTerminal }.forEach { record ->
                    scopedTasks.firstOrNull { it.scope.scopeId == record.scopeId && it.task.id == record.taskId }?.let {
                        recoverNativeTerminal(record, it.task)
                    }
                }
                val history = historyStore.records()
                val exactAllowed = CronAlarmScheduler.canScheduleExact(appContext)
                val activeByTask = history
                    .filterNot { it.status.isTerminal }
                    .associateBy { it.scopeId to it.taskId }
                val supportedNext = scopedTasks
                    .filter { it.task.mobileSupported && it.task.isActive() }
                    .filterNot { activeByTask.containsKey(it.scope.scopeId to it.task.id) }
                    .mapNotNull { it.task.nextFireMs?.toLong() }
                    .minOrNull()
                val latestByTask = history
                    .filter { it.status.isTerminal }
                    .sortedBy(CronRunRecord::triggeredAtMs)
                    .associateBy { it.scopeId to it.taskId }
                AndroidCronRepositoryState(
                    loading = false,
                    scopes = scopes,
                    activeScopeId = scopeScanner.activeScopeId(scopes),
                    tasks = scopedTasks.map { scoped ->
                        val supported = scoped.task.mobileSupported
                        AndroidCronTask(
                            task = scoped.task,
                            scope = scoped.scope,
                            schedulingMode = when {
                                !supported -> CronSchedulingMode.Unsupported
                                exactAllowed -> CronSchedulingMode.Exact
                                else -> CronSchedulingMode.FifteenMinuteFallback
                            },
                            unsupportedReason = scoped.task.unsupportedReason,
                            activeRun = activeByTask[scoped.scope.scopeId to scoped.task.id],
                            lastRun = latestByTask[scoped.scope.scopeId to scoped.task.id],
                        )
                    }.sortedWith(
                        compareBy<AndroidCronTask> { it.scope.scopeId }
                            .thenBy { it.task.nextFireMs?.toLong() ?: Long.MAX_VALUE },
                    ),
                    history = history.sortedByDescending(CronRunRecord::triggeredAtMs),
                    generatedSessions = historyStore.generatedSessions(),
                    exactAlarmAllowed = exactAllowed,
                    schedulingMode = if (exactAllowed) {
                        CronSchedulingMode.Exact
                    } else {
                        CronSchedulingMode.FifteenMinuteFallback
                    },
                    nextScheduledAtMs = supportedNext,
                    activeWorkCount = history.count { !it.status.isTerminal },
                    networkAvailable = hasConnectedNetwork(),
                    workManagerStateCounts = _state.value.workManagerStateCounts,
                    lastReconciledAtMs = lastReconciledAtMs
                        ?: _state.value.lastReconciledAtMs,
                )
            }.getOrElse { error ->
                _state.value.copy(
                    loading = false,
                    errorMessage = error.message
                        ?: appContext.getString(R.string.cron_load_failed_default),
                    exactAlarmAllowed = CronAlarmScheduler.canScheduleExact(appContext),
                    networkAvailable = hasConnectedNetwork(),
                    workManagerStateCounts = _state.value.workManagerStateCounts,
                    lastReconciledAtMs = lastReconciledAtMs
                        ?: _state.value.lastReconciledAtMs,
                )
            }
            _state.value = next
        }
    }

    internal fun recoverNativeTerminal(record: CronRunRecord, task: com.lingxi.code.bindings.runtime.CronTaskDto): Boolean {
        val automation = CronAutomation.from(task)
        val terminal = automation.terminalFor(record) ?: return false
        historyStore.attachExecution(record.runId, terminal.sessionId, terminal.model)
        historyStore.markTerminal(record.runId, terminal.status, terminal.summary, terminal.error, terminal.finishedAtMs)
        postPendingNotifications(record.runId)
        return true
    }

    internal fun postPendingNotifications(runId: String? = null) {
        historyStore.postPendingNotifications(runId, onFailure = { error ->
            if (error is CancellationException) throw error
            Log.w("AndroidCronRepository", "Pending cron notification could not be delivered", error)
        }) { record ->
            val delivered = CronNotifications.postResult(
                context = appContext,
                title = record.prompt.lineSequence().firstOrNull()?.take(40)?.ifBlank { null }
                    ?: appContext.getString(R.string.cron_result_title_default),
                body = record.resultText ?: record.errorMessage
                    ?: appContext.getString(R.string.chat_status_completed),
                tag = "cron-${record.runId}",
                runId = record.runId,
            )
            // POST_NOTIFICATIONS can be denied (Android 13+), in which case
            // nothing was shown. Give the durable claim back so the result is
            // still recoverable once the user grants the permission.
            if (!delivered) historyStore.releaseNotificationClaim(record.runId)
        }
    }

    private fun defaultAutomation(): CronAutomation =
        com.lingxi.code.settings.ProviderSettingsRepository(appContext).use {
            CronAutomation.defaults(it.engineLaunchConfig().defaultModel)
        }

    suspend fun create(
        scopeId: String,
        cron: String,
        prompt: String,
        recurring: Boolean,
        automation: CronAutomation = defaultAutomation(),
    ): com.lingxi.code.bindings.runtime.CronTaskDto = mutateAndReconcile(
        reason = "create",
        scopeId = scopeId,
    ) { scope ->
        gateway.create(scope, cron.trim(), prompt.trim(), recurring, automation)
    }

    suspend fun update(
        scopeId: String,
        taskId: String,
        cron: String,
        prompt: String,
        recurring: Boolean,
        automation: CronAutomation,
    ): com.lingxi.code.bindings.runtime.CronTaskDto = mutateAndReconcile(
        reason = "update",
        scopeId = scopeId,
    ) { scope ->
        require(gateway.list(scope).any { it.id == taskId }) {
            appContext.getString(R.string.cron_task_not_in_project)
        }
        gateway.update(scope, taskId, cron.trim(), prompt.trim(), recurring, automation).also {
            if (!it.isActive()) historyStore.cancelQueued(scope.scopeId, taskId)
        }
    }

    suspend fun pauseForArchivedSession(scopeId: String, sessionId: String) {
        val scope = requireScope(scopeId)
        gateway.list(scope).filter { task ->
            val config = CronAutomation.from(task)
            config.status == "active" && (
                (config.runMode == "selected_session" && config.targetSessionId.removePrefix("sess:") == sessionId.removePrefix("sess:")) ||
                    (config.runMode == "task_session" && org.json.JSONObject(config.json).optString("ownedSessionId").removePrefix("sess:") == sessionId.removePrefix("sess:")))
        }.forEach { task ->
            val automation = CronAutomation.from(task)
            update(scopeId, task.id, task.cron, task.prompt, task.recurring,
                automation.change("status", "paused").change("statusReason", "The associated chat was archived"))
            if (automation.notificationPolicy != "none") {
                CronNotifications.postResult(appContext,
                    automation.name.ifBlank { "Scheduled task paused" },
                    "The associated chat was archived. Select another chat to resume this task.",
                    tag = "cron-paused-$scopeId-${task.id}")
            }
        }
    }

    suspend fun delete(scopeId: String, taskId: String): Boolean =
        mutateAndReconcile(
            reason = "delete",
            scopeId = scopeId,
        ) { scope ->
            gateway.delete(scope, taskId).also { deleted ->
                if (deleted) {
                    historyStore.cancelUnfinished(
                        scope.scopeId,
                        taskId,
                        appContext.getString(R.string.cron_task_deleted_message),
                    )
                }
            }
        }

    suspend fun runNow(scopeId: String, taskId: String): CronRunRecord =
        withContext(Dispatchers.IO) {
            mutationMutex.withLock {
                val scope = requireScope(scopeId)
                val task = gateway.list(scope).firstOrNull { it.id == taskId }
                    ?: throw IllegalArgumentException(
                        appContext.getString(R.string.cron_task_not_in_project),
                    )
                require(task.isActive()) { "Resume this task before running it" }
                val now = System.currentTimeMillis()
                val record = historyStore.enqueue(
                    scope = scope,
                    taskId = taskId,
                    prompt = task.prompt,
                    scheduledAtMs = now,
                    triggeredAtMs = now,
                    manual = true,
                ) ?: throw IllegalStateException(
                    appContext.getString(R.string.cron_task_already_running),
                )
                CronWorkScheduler.enqueueExecutionChain(appContext, listOf(record))
                CronCoordinator(appContext, this@AndroidCronRepository).reconcile("run-now")
                record
            }
        }

    suspend fun reconcile(reason: String = "ui") {
        CronCoordinator(appContext, this).reconcile(reason)
    }

    private suspend fun <T> mutateAndReconcile(
        reason: String,
        scopeId: String,
        operation: suspend (CronScope) -> T,
    ): T = withContext(Dispatchers.IO) {
        mutationMutex.withLock {
            val scope = requireScope(scopeId)
            val result = operation(scope)
            CronCoordinator(appContext, this@AndroidCronRepository).reconcile(reason)
            result
        }
    }

    internal suspend fun loadScopedTasks(
        scopes: List<CronScope> = scopeScanner.scan(),
    ): List<ScopedCronTask> = withContext(Dispatchers.IO) {
        scopes.flatMap { scope ->
            runCatching { gateway.list(scope) }
                .getOrDefault(emptyList())
                .map { task -> ScopedCronTask(scope, task) }
        }
    }

    internal suspend fun loadDueOccurrences(nowMs: Long): List<ScopedCronOccurrence> =
        withContext(Dispatchers.IO) {
            scopeScanner.scan().flatMap { scope ->
                val tasks = runCatching { gateway.list(scope) }
                    .getOrDefault(emptyList())
                    .associateBy { it.id }
                runCatching { gateway.dueOccurrences(scope, nowMs) }
                    .getOrDefault(emptyList())
                    .mapNotNull { (taskId, scheduledAtMs) ->
                        tasks[taskId]
                            ?.takeIf { it.mobileSupported && it.isActive() }
                            ?.let { ScopedCronOccurrence(scope, it, scheduledAtMs) }
                    }
            }
        }

    internal fun scope(scopeId: String): CronScope? = scopeScanner.find(scopeId)

    private fun requireScope(scopeId: String): CronScope =
        scope(scopeId) ?: throw IllegalArgumentException(
            appContext.getString(R.string.cron_workspace_unavailable),
        )

    private fun hasConnectedNetwork(): Boolean {
        val manager = appContext.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
            ?: return false
        val active = manager.activeNetwork ?: return false
        val capabilities = manager.getNetworkCapabilities(active) ?: return false
        return capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }

    companion object {
        @Volatile
        private var instance: AndroidCronRepository? = null

        fun get(context: Context): AndroidCronRepository =
            instance ?: synchronized(this) {
                instance ?: create(context.applicationContext).also { instance = it }
            }

        internal fun create(
            context: Context,
            scopeScanner: CronScopeScanner = CronScopeScanner(context),
            gateway: CronEngineGateway = MobileCronEngineGateway(context),
            historyStore: CronRunHistoryStore = CronRunHistoryStore(
                File(context.applicationContext.filesDir, "cron/history"),
            ),
        ): AndroidCronRepository = AndroidCronRepository(
            context.applicationContext,
            scopeScanner,
            gateway,
            historyStore,
        )
    }
}

/**
 * `runCatching` catches [CancellationException], which must never be converted
 * into repository error state. Doing so changes normal Compose cancellation
 * into a later non-cancellation exception on the main thread.
 */
internal inline fun <T> runCronCatching(block: () -> T): Result<T> = try {
    Result.success(block())
} catch (cancel: CancellationException) {
    throw cancel
} catch (error: Throwable) {
    Result.failure(error)
}
