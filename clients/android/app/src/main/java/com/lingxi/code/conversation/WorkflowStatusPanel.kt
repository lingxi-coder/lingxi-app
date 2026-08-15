package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
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
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
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
import com.lingxi.code.bindings.TaskStatusDto
import com.lingxi.code.components.UiTags
import com.lingxi.code.theme.LingXiTheme

/** Event-driven workflow footer. It never polls task or transcript state. */
@Composable
fun WorkflowStatusPanel(
    runs: List<WorkflowRunUi>,
    modifier: Modifier = Modifier,
) {
    if (runs.isEmpty()) return
    val t = LingXiTheme.palette
    var collapsed by remember { mutableStateOf(false) }
    val ordered = remember(runs) { runs.sortedByDescending { it.lastUpdatedAtMs } }
    val active = ordered.count { it.status.isActivelyExecutingWorkflow() }

    Column(
        verticalArrangement = Arrangement.spacedBy(7.dp),
        modifier = modifier
            .testTag(UiTags.WORKFLOW_STATUS_PANEL)
            .fillMaxWidth()
            .padding(horizontal = 14.dp, vertical = 4.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(t.surface)
            .border(0.5.dp, t.border, RoundedCornerShape(12.dp))
            .padding(horizontal = 12.dp, vertical = 9.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(7.dp),
            modifier = Modifier
                .fillMaxWidth()
                .clickable { collapsed = !collapsed },
        ) {
            if (active > 0) {
                CircularProgressIndicator(modifier = Modifier.size(13.dp), strokeWidth = 1.5.dp)
            } else {
                Box(Modifier.size(8.dp).clip(CircleShape).background(t.ok))
            }
            Text(
                text = stringResource(R.string.settings_title_workflows),
                color = t.text,
                fontSize = 12.5f.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = "${runs.size} · $active ${stringResource(R.string.chat_agent_group_running)}",
                color = t.text3,
                fontSize = 11.sp,
            )
            Spacer(Modifier.weight(1f))
            Icon(
                imageVector = if (collapsed) Icons.Rounded.KeyboardArrowDown else Icons.Rounded.KeyboardArrowUp,
                contentDescription = null,
                tint = t.text3,
                modifier = Modifier.size(18.dp),
            )
        }
        if (!collapsed) {
            ordered.forEach { run -> WorkflowRunRow(run) }
        }
    }
}

@Composable
private fun WorkflowRunRow(run: WorkflowRunUi) {
    val t = LingXiTheme.palette
    Column(
        verticalArrangement = Arrangement.spacedBy(5.dp),
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(9.dp))
            .background(t.windowBg)
            .padding(horizontal = 10.dp, vertical = 8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                text = run.currentPhaseTitle ?: run.taskId,
                color = t.text,
                fontSize = 12.sp,
                fontWeight = FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Text(
                text = workflowTaskStatusLabel(run.status),
                color = if (run.status == TaskStatusDto.FAILED) t.danger else t.text3,
                fontSize = 10.5f.sp,
            )
        }
        run.latestLog?.takeIf(String::isNotBlank)?.let { log ->
            Text(
                text = log,
                color = t.text3,
                fontSize = 10.5f.sp,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
        run.agents.sortedBy { it.index }.forEach { agent ->
            WorkflowAgentRow(agent, run.status.isActivelyExecutingWorkflow())
        }
    }
}

@Composable
private fun WorkflowAgentRow(
    agent: WorkflowAgentUi,
    activelyExecuting: Boolean,
) {
    val t = LingXiTheme.palette
    Row(
        verticalAlignment = Alignment.Top,
        horizontalArrangement = Arrangement.spacedBy(7.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        if (agent.status.isTerminal || !activelyExecuting) {
            Box(
                Modifier
                    .padding(top = 4.dp)
                    .size(7.dp)
                    .clip(CircleShape)
                    .background(
                        when {
                            agent.status == WorkflowAgentStatus.Error -> t.danger
                            !activelyExecuting -> t.statusIdle
                            else -> t.ok
                        },
                    ),
            )
        } else {
            CircularProgressIndicator(
                modifier = Modifier.padding(top = 1.dp).size(10.dp),
                strokeWidth = 1.3.dp,
            )
        }
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = agent.displayTitle,
                    color = t.text2,
                    fontSize = 11.5f.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                Text(
                    text = workflowAgentStatusLabel(agent.status),
                    color = t.text4,
                    fontSize = 9.5f.sp,
                )
            }
            listOfNotNull(
                workflowModelLine(agent),
                agent.activity?.takeIf(String::isNotBlank),
            ).forEach { detail ->
                Text(
                    text = detail,
                    color = t.text4,
                    fontSize = 10.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

internal fun workflowModelLine(agent: WorkflowAgentUi): String? =
    agent.model?.takeIf(String::isNotBlank)?.let { model ->
        agent.fallbackModel
            ?.takeIf { it.isNotBlank() && it != model }
            ?.let { "$model → $it" }
            ?: model
    }

@Composable
private fun workflowAgentStatusLabel(status: WorkflowAgentStatus): String = when (status) {
    WorkflowAgentStatus.Start -> stringResource(R.string.settings_linux_task_queued)
    WorkflowAgentStatus.Progress -> stringResource(R.string.chat_run_status_running)
    WorkflowAgentStatus.Done -> stringResource(R.string.chat_run_status_completed)
    WorkflowAgentStatus.Error -> stringResource(R.string.chat_run_status_failed)
    WorkflowAgentStatus.Cached -> stringResource(R.string.common_done)
}

@Composable
private fun workflowTaskStatusLabel(status: TaskStatusDto): String = when (status) {
    TaskStatusDto.PENDING -> stringResource(R.string.settings_linux_task_queued)
    TaskStatusDto.RUNNING -> stringResource(R.string.chat_run_status_running)
    TaskStatusDto.PAUSED -> stringResource(R.string.settings_status_paused)
    TaskStatusDto.COMPLETED -> stringResource(R.string.chat_run_status_completed)
    TaskStatusDto.FAILED -> stringResource(R.string.chat_run_status_failed)
    TaskStatusDto.CANCELLED -> stringResource(R.string.chat_run_status_cancelled)
}
