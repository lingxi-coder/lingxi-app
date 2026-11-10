package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.SessionAgentSummaryDto
import com.lingxi.code.bindings.client.TaskRowDto
import com.lingxi.code.bindings.client.TaskStatusDto
import androidx.compose.runtime.Composable
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R

data class BackgroundTaskUi(
    val taskId: String,
    val taskType: String,
    val description: String,
    val status: TaskStatusDto,
    val canResume: Boolean,
    val startedAtMs: ULong?,
    /**
     * Terminal failure reason reported by the engine (`TaskRowDto.error` /
     * `TaskStatusChanged.error`). Only a FAILED row carries one. This is the
     * only surface a user who has moved to another session can still learn the
     * reason from: the transient status line is suppressed for a non-visible
     * origin session, while the refreshed row list still carries `error`.
     */
    val error: String? = null,
)

data class SessionAgentUi(
    val agentId: String,
    val name: String,
    val agentType: String,
    val model: String?,
    val status: String,
    val latestActivity: String?,
    val reportDelivery: String? = null,
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
    description = description.displayText(),
    status = status,
    canResume = canResume,
    startedAtMs = startedAtMs,
    error = error,
)

fun SessionAgentSummaryDto.toSessionAgentUi(): SessionAgentUi = SessionAgentUi(
    agentId = agentId,
    name = name.displayText(),
    agentType = agentType,
    model = model,
    status = status,
    latestActivity = latestActivity?.displayText(),
    reportDelivery = reportDelivery,
)

internal fun TaskStatusDto.isAttentionStatus(): Boolean =
    this == TaskStatusDto.PAUSED || this == TaskStatusDto.FAILED

/** Execution and report receipt are separate facts. */
@Composable
fun SessionAgentUi.reportStatusText(): String? = when (reportDelivery) {
    "withheld" -> stringResource(R.string.chat_agent_report_withheld)
    "flagged" -> stringResource(R.string.chat_agent_report_flagged)
    "pending" -> stringResource(R.string.chat_agent_report_pending)
    "delivered" -> stringResource(R.string.chat_agent_report_delivered)
    else -> if (status == "completed") stringResource(R.string.chat_agent_execution_ended) else null
}

/** Execution outcome and delivery are separate, matching Desktop's transcript. */
@Composable
internal fun SessionAgentUi.transcriptStatusText(): String = when {
    status in listOf("running", "working") -> stringResource(R.string.chat_agent_started_working)
    reportDelivery in listOf("withheld", "flagged", "pending") -> reportStatusText() ?: status
    status == "completed" && reportDelivery == "delivered" -> stringResource(R.string.chat_agent_finished)
    status == "completed" && reportDelivery == null -> stringResource(R.string.chat_agent_execution_ended)
    status in listOf("failed", "error") -> stringResource(R.string.chat_status_failed)
    status == "cancelled" -> stringResource(R.string.chat_status_cancelled)
    else -> status
}
