package com.lingxi.code.conversation

import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.KeyboardArrowUp
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.bindings.client.TaskStatusDto
import com.lingxi.code.components.UiTags
import com.lingxi.code.theme.LingXiTheme

@Composable
internal fun ExecutionStatusPanel(
    tasks: List<BackgroundTaskUi>,
    workflows: List<WorkflowRunUi>,
    agents: List<SessionAgentUi>,
    planTasks: List<PlanTaskUi>,
    planExpanded: Boolean,
    onTogglePlan: () -> Unit,
    onResumeWorkflow: (String) -> Unit,
    resumeState: WorkflowResumeUiState,
    agentRun: AgentRunState? = null,
    expandedToolCalls: Set<String> = emptySet(),
    onToggleToolCall: (String) -> Unit = {},
    modifier: Modifier = Modifier,
) {
    if (tasks.isEmpty() && workflows.isEmpty() && agents.isEmpty() && planTasks.isEmpty() && agentRun == null) return
    val palette = LingXiTheme.palette
    var collapsed by remember { mutableStateOf(false) }
    val taskIds = remember(tasks) { tasks.mapTo(hashSetOf()) { it.taskId } }
    val attentionKey = remember(tasks) {
        tasks.filter { it.status.isAttentionStatus() }
            .joinToString("|") { "${it.taskId}:${it.status}" }
    }
    LaunchedEffect(attentionKey) { if (attentionKey.isNotEmpty()) collapsed = false }
    val active = tasks.count { it.status == TaskStatusDto.PENDING || it.status == TaskStatusDto.RUNNING } +
        workflows.count { it.status == TaskStatusDto.PENDING || it.status == TaskStatusDto.RUNNING }

    Column(
        verticalArrangement = Arrangement.spacedBy(7.dp),
        modifier = modifier
            .testTag(UiTags.EXECUTION_STATUS_PANEL)
            .fillMaxWidth()
            .padding(horizontal = 14.dp, vertical = 4.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(palette.surface)
            .border(0.5.dp, palette.border, RoundedCornerShape(12.dp))
            .padding(horizontal = 12.dp, vertical = 9.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.fillMaxWidth().clickable { collapsed = !collapsed },
        ) {
            if (active > 0) CircularProgressIndicator(Modifier.size(13.dp), strokeWidth = 1.5.dp)
            else Icon(Icons.Rounded.PlayArrow, null, tint = palette.text3, modifier = Modifier.size(15.dp))
            Text(
                text = stringResource(R.string.chat_execution_status),
                color = palette.text,
                fontSize = 12.5f.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.padding(start = 7.dp),
            )
            Text(
                text = "${agents.size} · ${tasks.size + workflows.count { it.taskId !in taskIds }} · ${planTasks.size}",
                color = palette.text3,
                fontSize = 11.sp,
                modifier = Modifier.padding(start = 6.dp),
            )
            Spacer(Modifier.weight(1f))
            Icon(
                if (collapsed) Icons.Rounded.KeyboardArrowDown else Icons.Rounded.KeyboardArrowUp,
                null,
                tint = palette.text3,
                modifier = Modifier.size(18.dp),
            )
        }
        if (!collapsed) {
            if (agentRun != null) {
                SectionLabel(stringResource(R.string.chat_execution_agents))
                AgentRunTimeline(
                    state = agentRun,
                    expandedToolCalls = expandedToolCalls,
                    onToggleToolCall = onToggleToolCall,
                )
            }
            if (agents.isNotEmpty()) {
                SectionLabel(stringResource(R.string.chat_execution_agents))
                agents.sortedBy { it.agentId }.forEach { agent -> AgentSummaryRow(agent) }
            }
            if (tasks.isNotEmpty()) {
                SectionLabel(stringResource(R.string.chat_execution_tasks))
                val workflowByTask = workflows.associateBy { it.taskId }
                tasks.sortedWith(compareBy<BackgroundTaskUi> { it.status.isTerminal() }.thenByDescending { it.startedAtMs ?: 0u })
                    // Keep every active/paused row visible so a Resume action
                    // cannot disappear behind the completed-task cap.
                    .take(3 + tasks.count { !it.status.isTerminal() })
                    .forEach { task ->
                        TaskSummaryRow(
                            task = task,
                            workflow = workflowByTask[task.taskId],
                            onResume = onResumeWorkflow,
                            resuming = resumeState == WorkflowResumeUiState.Resuming,
                        )
                    }
            }
            val workflowOnly = workflows.filter { workflow -> tasks.none { it.taskId == workflow.taskId } }
            if (workflowOnly.isNotEmpty()) {
                SectionLabel(stringResource(R.string.chat_execution_tasks))
                workflowOnly.sortedByDescending { it.lastUpdatedAtMs }.forEach { workflow ->
                    WorkflowOnlyRow(workflow)
                }
            }
            if (planTasks.isNotEmpty()) {
                SectionLabel(stringResource(R.string.chat_execution_plan))
                PlanTasksPanel(planTasks, planExpanded, onTogglePlan)
            }
        }
    }
}

@Composable
private fun WorkflowOnlyRow(workflow: WorkflowRunUi) {
    val palette = LingXiTheme.palette
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        BoxStatusDot(workflow.status.name)
        Column(Modifier.padding(start = 7.dp).weight(1f)) {
            Text(
                workflow.currentPhaseTitle ?: workflow.taskId,
                color = palette.text2,
                fontSize = 11.5f.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                workflow.latestLog ?: "Workflow ${workflow.runId}",
                color = palette.text4,
                fontSize = 10.sp,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
private fun SectionLabel(text: String) {
    Text(text, fontSize = 10.5f.sp, fontWeight = FontWeight.SemiBold, color = LingXiTheme.palette.text3)
}

@Composable
private fun AgentSummaryRow(agent: SessionAgentUi) {
    val palette = LingXiTheme.palette
    val detailHost = LocalConversationDetail.current
    var details by remember(agent.agentId) { mutableStateOf(false) }
    if (details) RuntimeDetails(
        title = agent.name.ifBlank { agent.agentId },
        body = listOfNotNull(agent.agentId, agent.agentType, agent.model, agent.status, agent.latestActivity)
            .filter(String::isNotBlank).joinToString("\n\n"),
        onClose = { details = false },
    )
    Row(Modifier.fillMaxWidth().heightIn(min = 44.dp).clickable {
        if (detailHost != null) detailHost("agent:${agent.agentId}") else details = true
    }, verticalAlignment = Alignment.CenterVertically) {
        com.lingxi.code.theme.AgentAvatar(agentId = agent.agentId, modifier = Modifier.size(28.dp))
        BoxStatusDot(agent.status)
        Column(Modifier.padding(start = 7.dp).weight(1f)) {
            Text(agent.name.ifBlank { agent.agentId }, color = palette.text2, fontSize = 11.5f.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                listOfNotNull(agent.agentType.takeIf(String::isNotBlank), agent.model, agent.latestActivity).joinToString(" · "),
                color = palette.text4,
                fontSize = 10.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

/**
 * Test tag for the terminal failure reason under a FAILED task row. Mirrors the
 * iOS accessibility identifier `chat.tasks-panel.failure-reason`.
 */
internal const val TASK_FAILURE_REASON_TAG: String = "tag.executionStatusTaskFailureReason"

@Composable
private fun TaskSummaryRow(
    task: BackgroundTaskUi,
    workflow: WorkflowRunUi?,
    onResume: (String) -> Unit,
    resuming: Boolean,
) {
    val palette = LingXiTheme.palette
    val detailHost = LocalConversationDetail.current
    var details by remember(task.taskId) { mutableStateOf(false) }
    if (details) RuntimeDetails(
        title = task.description.ifBlank { task.taskId },
        body = listOfNotNull(task.taskId, taskStatusText(task.status), workflow?.currentPhaseTitle,
            workflow?.latestLog, task.error).joinToString("\n\n"),
        onClose = { details = false },
    )
    Row(Modifier.fillMaxWidth().heightIn(min = 44.dp).clickable {
        if (detailHost != null) detailHost("task:${task.taskId}") else details = true
    }, verticalAlignment = Alignment.CenterVertically) {
        BoxStatusDot(task.status.name)
        Column(Modifier.padding(start = 7.dp).weight(1f)) {
            Text(task.description.ifBlank { workflow?.currentPhaseTitle ?: task.taskId }, color = palette.text2, fontSize = 11.5f.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(workflow?.latestLog ?: taskStatusText(task.status), color = palette.text4, fontSize = 10.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
            // The engine's terminal failure reason (`TaskRowDto.error` /
            // `TaskStatusChanged.error`, carried on `BackgroundTaskUi.error`).
            // This is the ONLY surface a user who was in another session when
            // the task failed can still learn WHY from: the transient
            // `statusLine` notice is suppressed for a non-visible origin
            // session (ChatViewModel `TaskStatusChanged`), while the durable
            // row re-delivered as `ClientEvent.TaskRow` still carries `error`.
            // Mirrors iOS TasksStatusPanel.workflowRows (the bare reason in
            // danger under a `.failed` row).
            val failureReason = task.error?.takeIf { it.isNotBlank() }
            if (task.status == TaskStatusDto.FAILED && failureReason != null) {
                Text(
                    failureReason,
                    color = palette.danger,
                    fontSize = 10.sp,
                    maxLines = 3,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(top = 2.dp).testTag(TASK_FAILURE_REASON_TAG),
                )
            }
        }
        if (task.canResume && task.status == TaskStatusDto.PAUSED) {
            IconButton(onClick = { onResume(task.taskId) }, enabled = !resuming) {
                if (resuming) CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 1.5.dp)
                else Icon(Icons.Rounded.PlayArrow, stringResource(R.string.chat_workflow_resume), tint = palette.accent, modifier = Modifier.size(18.dp))
            }
        }
    }
}

@Composable
private fun BoxStatusDot(status: String) {
    val palette = LingXiTheme.palette
    androidx.compose.foundation.layout.Box(
        Modifier.size(7.dp).clip(CircleShape).background(
            when (status.lowercase()) {
                "failed", "error" -> palette.danger
                "completed", "done", "cached" -> palette.ok
                "running", "progress" -> palette.accent
                else -> palette.statusIdle
            },
        ),
    )
}

private fun TaskStatusDto.isTerminal(): Boolean = when (this) {
    TaskStatusDto.PENDING, TaskStatusDto.RUNNING, TaskStatusDto.PAUSED -> false
    TaskStatusDto.COMPLETED, TaskStatusDto.FAILED, TaskStatusDto.CANCELLED -> true
}

@Composable
internal fun taskStatusText(status: TaskStatusDto): String = when (status) {
    TaskStatusDto.PENDING -> stringResource(R.string.settings_linux_task_queued)
    TaskStatusDto.RUNNING -> stringResource(R.string.chat_run_status_running)
    TaskStatusDto.PAUSED -> stringResource(R.string.chat_status_paused)
    TaskStatusDto.COMPLETED -> stringResource(R.string.chat_run_status_completed)
    TaskStatusDto.FAILED -> stringResource(R.string.chat_run_status_failed)
    TaskStatusDto.CANCELLED -> stringResource(R.string.chat_run_status_cancelled)
}

/** Long runtime descriptions remain readable without growing the composer panel. */
@Composable
private fun RuntimeDetails(title: String, body: String, onClose: () -> Unit) {
    val palette = LingXiTheme.palette
    val compact = LocalConfiguration.current.screenWidthDp < 840
    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = !compact)) {
        Column(
            (if (compact) Modifier.fillMaxSize() else Modifier.heightIn(max = 720.dp))
                .background(palette.surface).safeDrawingPadding().padding(20.dp)
                .verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text(title, Modifier.weight(1f), color = palette.text, fontSize = 18.sp,
                    fontWeight = FontWeight.SemiBold)
                IconButton(onClick = onClose) {
                    com.lingxi.code.components.LXIcon(com.lingxi.code.components.LXIconName.X,
                        color = palette.text3, contentDescription = stringResource(R.string.chat_run_collapse))
                }
            }
            Text(body, color = palette.text2, fontSize = 15.sp, lineHeight = 24.sp)
        }
    }
}
