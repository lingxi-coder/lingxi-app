package com.lingxi.code.cron

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import androidx.work.WorkManager
import kotlinx.coroutines.CancellationException
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
                val scopes = scopeScanner.scan()
                val scopedTasks = loadScopedTasks(scopes)
                val history = historyStore.records()
                val exactAllowed = CronAlarmScheduler.canScheduleExact(appContext)
                val activeByTask = history
                    .filterNot { it.status.isTerminal }
                    .associateBy { it.scopeId to it.taskId }
                val supportedNext = scopedTasks
                    .filter { it.task.mobileSupported }
                    .filterNot { activeByTask.containsKey(it.scope.scopeId to it.task.id) }
                    .mapNotNull { it.task.nextFireMs?.toLong() }
                    .minOrNull()
                val latestByTask = history
                    .filter { it.status.isTerminal }
                    .sortedByDescending(CronRunRecord::triggeredAtMs)
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
                    errorMessage = error.message ?: "定时任务读取失败",
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

    suspend fun create(
        scopeId: String,
        cron: String,
        prompt: String,
        recurring: Boolean,
    ): com.lingxi.code.bindings.CronTaskDto = mutateAndReconcile(
        reason = "create",
        scopeId = scopeId,
    ) { scope ->
        gateway.create(scope, cron.trim(), prompt.trim(), recurring)
    }

    suspend fun update(
        scopeId: String,
        taskId: String,
        cron: String,
        prompt: String,
        recurring: Boolean,
    ): com.lingxi.code.bindings.CronTaskDto = mutateAndReconcile(
        reason = "update",
        scopeId = scopeId,
    ) { scope ->
        require(gateway.list(scope).any { it.id == taskId }) {
            "任务不属于当前 Project 或已被删除"
        }
        gateway.update(scope, taskId, cron.trim(), prompt.trim(), recurring)
    }

    suspend fun delete(scopeId: String, taskId: String): Boolean =
        mutateAndReconcile(
            reason = "delete",
            scopeId = scopeId,
        ) { scope ->
            gateway.delete(scope, taskId).also { deleted ->
                if (deleted) {
                    historyStore.cancelUnfinished(scope.scopeId, taskId, "任务已删除")
                }
            }
        }

    suspend fun runNow(scopeId: String, taskId: String): CronRunRecord =
        withContext(Dispatchers.IO) {
            mutationMutex.withLock {
                val scope = requireScope(scopeId)
                val task = gateway.list(scope).firstOrNull { it.id == taskId }
                    ?: throw IllegalArgumentException("任务不属于当前 Project 或已被删除")
                val now = System.currentTimeMillis()
                val record = historyStore.enqueue(
                    scope = scope,
                    taskId = taskId,
                    prompt = task.prompt,
                    scheduledAtMs = now,
                    triggeredAtMs = now,
                    manual = true,
                ) ?: throw IllegalStateException("该任务已有排队或运行中的执行")
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
                            ?.takeIf { it.mobileSupported }
                            ?.let { ScopedCronOccurrence(scope, it, scheduledAtMs) }
                    }
            }
        }

    internal fun scope(scopeId: String): CronScope? = scopeScanner.find(scopeId)

    private fun requireScope(scopeId: String): CronScope =
        scope(scopeId) ?: throw IllegalArgumentException("Project 工作区不存在或已失效")

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
