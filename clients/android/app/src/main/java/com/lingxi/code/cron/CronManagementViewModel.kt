package com.lingxi.code.cron

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.lingxi.code.R
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

/**
 * UI adapter for the process-wide Android cron repository.
 *
 * Task CRUD is storage-only and therefore remains available before an API key
 * is configured. A provider is required only when WorkManager actually executes
 * a task.
 */
class CronManagementViewModel(app: Application) : AndroidViewModel(app) {
    private val repository = AndroidCronRepository.get(app)

    val state: StateFlow<AndroidCronRepositoryState> = repository.state

    init {
        repository.refresh()
    }

    fun refresh() {
        repository.refresh()
    }

    fun reconcile(reason: String = "ui") {
        viewModelScope.launch {
            runCatching { repository.reconcile(reason) }
        }
    }

    fun create(
        scopeId: String,
        cron: String,
        prompt: String,
        recurring: Boolean,
        automation: CronAutomation,
        onResult: (String?) -> Unit,
    ) {
        viewModelScope.launch {
            val error = runCatching {
                repository.create(scopeId, cron, prompt, recurring, automation)
            }.exceptionOrNull()?.message
            onResult(error)
        }
    }

    fun update(
        scopeId: String,
        taskId: String,
        cron: String,
        prompt: String,
        recurring: Boolean,
        automation: CronAutomation,
        onResult: (String?) -> Unit,
    ) {
        viewModelScope.launch {
            val error = runCatching {
                repository.update(scopeId, taskId, cron, prompt, recurring, automation)
            }.exceptionOrNull()?.message
            onResult(error)
        }
    }

    fun delete(scopeId: String, taskId: String, onResult: (String?) -> Unit = {}) {
        viewModelScope.launch {
            val result = runCatching { repository.delete(scopeId, taskId) }
            onResult(
                result.exceptionOrNull()?.message
                    ?: if (result.getOrDefault(false)) {
                        null
                    } else {
                        getApplication<Application>().getString(R.string.cron_task_delete_failed_message)
                    },
            )
        }
    }

    fun runNow(scopeId: String, taskId: String, onResult: (String?) -> Unit = {}) {
        viewModelScope.launch {
            val error = runCatching { repository.runNow(scopeId, taskId) }
                .exceptionOrNull()
                ?.message
            onResult(error)
        }
    }
}
