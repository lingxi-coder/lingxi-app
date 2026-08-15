package com.lingxi.code.conversation

import com.lingxi.code.bindings.SessionAgentSummaryDto
import com.lingxi.code.bindings.TaskRowDto
import com.lingxi.code.bindings.TaskStatusDto

data class BackgroundTaskUi(
    val taskId: String,
    val taskType: String,
    val description: String,
    val status: TaskStatusDto,
    val canResume: Boolean,
    val startedAtMs: ULong?,
)

data class SessionAgentUi(
    val agentId: String,
    val name: String,
    val agentType: String,
    val model: String?,
    val status: String,
    val latestActivity: String?,
)

enum class WorkflowResumeUiState {
    Idle,
    Resuming,
    Succeeded,
    Failed,
}

fun TaskRowDto.toBackgroundTaskUi(): BackgroundTaskUi = BackgroundTaskUi(
    taskId = taskId,
    taskType = taskType,
    description = description,
    status = status,
    canResume = canResume,
    startedAtMs = startedAtMs,
)

fun SessionAgentSummaryDto.toSessionAgentUi(): SessionAgentUi = SessionAgentUi(
    agentId = agentId,
    name = name,
    agentType = agentType,
    model = model,
    status = status,
    latestActivity = latestActivity,
)

internal fun TaskStatusDto.isAttentionStatus(): Boolean =
    this == TaskStatusDto.PAUSED || this == TaskStatusDto.FAILED
