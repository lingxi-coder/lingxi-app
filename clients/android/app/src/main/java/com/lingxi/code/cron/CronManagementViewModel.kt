package com.lingxi.code.cron

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.lingxi.code.bindings.CronTaskDto
import com.lingxi.code.bindings.MobileEngineHandle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** Render state for the cron management screen. */
data class CronUiState(
    val loading: Boolean = true,
    val jobs: List<CronTaskDto> = emptyList(),
    /** `false` when no API key is configured (the engine can't be built). */
    val engineAvailable: Boolean = true,
    /** `false` when exact alarms are not permitted (Android 12 revoked). */
    val canScheduleExact: Boolean = true,
)

/**
 * Drives the cron management screen. Builds a transient headless
 * [MobileEngineHandle] (the cron FFI is cheap file I/O over the same
 * `scheduled_tasks.json` the background service uses) and exposes
 * list/create/delete, re-arming the exact alarm after any mutation so a new or
 * removed job's schedule takes effect immediately. The handle is released in
 * [onCleared].
 */
class CronManagementViewModel(app: Application) : AndroidViewModel(app) {

    private val _state = MutableStateFlow(CronUiState())
    val state: StateFlow<CronUiState> = _state.asStateFlow()

    private var handle: MobileEngineHandle? = null

    /** Serializes handle construction so two coroutines can't each build (and leak)
     *  a native engine when `handle` is still null. */
    private val handleMutex = Mutex()

    init {
        refresh()
    }

    private fun ctx() = getApplication<Application>().applicationContext

    private suspend fun ensureHandle(): MobileEngineHandle? = handleMutex.withLock {
        handle ?: withContext(Dispatchers.Default) { HeadlessEngineFactory.build(ctx()) }
            .also { handle = it }
    }

    /** Reload the cron list + exact-alarm permission state. */
    fun refresh() {
        viewModelScope.launch {
            _state.value = _state.value.copy(
                loading = true,
                canScheduleExact = CronAlarmScheduler.canScheduleExact(ctx()),
            )
            val engine = ensureHandle()
            if (engine == null) {
                _state.value = _state.value.copy(
                    loading = false,
                    engineAvailable = false,
                    jobs = emptyList(),
                )
                return@launch
            }
            val jobs = runCatching { engine.cronList() }.getOrDefault(emptyList())
            _state.value = _state.value.copy(loading = false, engineAvailable = true, jobs = jobs)
        }
    }

    /**
     * Create a job. `onResult(null)` on success, `onResult(message)` on a
     * validation / write failure (e.g. a malformed cron expression).
     */
    fun create(cron: String, prompt: String, recurring: Boolean, onResult: (String?) -> Unit) {
        viewModelScope.launch {
            val engine = ensureHandle()
            if (engine == null) {
                onResult("引擎不可用（未配置 API Key）")
                return@launch
            }
            val error = try {
                engine.cronCreate(cron.trim(), prompt.trim(), recurring)
                null
            } catch (t: Throwable) {
                t.message ?: "创建失败"
            }
            if (error == null) {
                armFrom(engine)
                refresh()
            }
            onResult(error)
        }
    }

    /** Delete a job by id, then re-arm the next alarm. */
    fun delete(id: String) {
        viewModelScope.launch {
            val engine = ensureHandle() ?: return@launch
            runCatching { engine.cronDelete(id) }
            armFrom(engine)
            refresh()
        }
    }

    /** Arm the next exact alarm using the handle we already hold (no rebuild). */
    private suspend fun armFrom(engine: MobileEngineHandle) {
        val nextMs = runCatching { engine.nextCronFireTime() }.getOrNull()
        if (nextMs == null) {
            CronAlarmScheduler.cancel(ctx())
        } else {
            CronAlarmScheduler.arm(ctx(), nextMs.toLong())
        }
    }

    override fun onCleared() {
        runCatching { handle?.destroy() }
        handle = null
        super.onCleared()
    }
}
