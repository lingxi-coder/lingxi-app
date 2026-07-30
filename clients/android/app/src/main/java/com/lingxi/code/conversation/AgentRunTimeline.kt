package com.lingxi.code.conversation

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.components.UiTags

/**
 * Bounded, latest-turn execution trace backed by real engine events.
 *
 * Reasoning and tool summaries are intentionally transient UI state. They are
 * not reconstructed as chat messages and are not written into the session
 * JSONL. The card stays available after completion until the next turn starts.
 */
@Composable
internal fun AgentRunTimeline(
    state: AgentRunState,
    modifier: Modifier = Modifier,
) {
    var expanded by rememberSaveable(state.turnId) { mutableStateOf(true) }
    val title = when (state.outcome) {
        AgentRunOutcome.Running -> "正在运行"
        AgentRunOutcome.Completed -> "运行完成"
        AgentRunOutcome.Failed -> "运行失败"
        AgentRunOutcome.Cancelled -> "已停止"
    }
    val statusColor = runStatusColor(state.outcome)
    Card(
        modifier = modifier
            .fillMaxWidth()
            .testTag(UiTags.AGENT_RUN_TIMELINE)
            .semantics { contentDescription = "Agent 运行过程，$title" }
            .clickable(role = Role.Button) { expanded = !expanded },
        shape = RoundedCornerShape(16.dp),
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
        ),
    ) {
        Column(
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (state.active) {
                    CircularProgressIndicator(
                        modifier = Modifier.size(15.dp),
                        strokeWidth = 2.dp,
                        color = statusColor,
                    )
                } else {
                    Box(
                        modifier = Modifier
                            .size(9.dp)
                            .background(statusColor, CircleShape),
                    )
                }
                Spacer(Modifier.width(9.dp))
                Text(
                    text = title,
                    style = MaterialTheme.typography.titleSmall,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.weight(1f),
                )
                val activity = when {
                    state.activeWorkers > 0 -> "${state.activeWorkers} 个协作者"
                    state.tools.any { it.status == AgentToolStatus.Running } -> "工具执行中"
                    state.reasoningActive -> "思考中"
                    state.active -> "生成中"
                    else -> null
                }
                activity?.let {
                    Text(
                        text = it,
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Spacer(Modifier.width(8.dp))
                Text(
                    text = if (expanded) "收起" else "展开",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.primary,
                )
            }

            AnimatedVisibility(expanded) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    if (state.reasoning.isNotBlank() || state.reasoningActive) {
                        TraceSection(title = if (state.reasoningActive) "思考中" else "思考") {
                            Text(
                                text = state.reasoning.ifBlank { "正在分析请求…" },
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 16,
                                overflow = TextOverflow.Ellipsis,
                            )
                            if (state.reasoningTruncated) {
                                Text(
                                    text = "较早的思考内容已省略",
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                        }
                    }

                    state.tools.forEach { tool -> ToolTraceRow(tool) }

                    state.notices.forEach { notice ->
                        NoticeTraceRow(notice)
                    }

                    if (state.activeWorkers > 0 || state.teamName != null) {
                        val team = state.teamName?.let { " · $it" }.orEmpty()
                        Text(
                            text = "协作者 ${state.activeWorkers}$team",
                            style = MaterialTheme.typography.labelMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }

                    TraceFooter(state)
                }
            }
        }
    }
}

@Composable
private fun TraceSection(
    title: String,
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(5.dp)) {
        Text(
            text = title,
            style = MaterialTheme.typography.labelMedium,
            fontWeight = FontWeight.SemiBold,
        )
        content()
    }
}

@Composable
private fun ToolTraceRow(tool: AgentToolRunState) {
    val statusLabel = when (tool.status) {
        AgentToolStatus.Running -> "运行中"
        AgentToolStatus.Completed -> "完成"
        AgentToolStatus.Failed -> "失败"
        AgentToolStatus.Cancelled -> "已取消"
    }
    val color = toolStatusColor(tool.status)
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.Top,
    ) {
        Box(
            modifier = Modifier
                .padding(top = 5.dp)
                .size(7.dp)
                .background(color, CircleShape),
        )
        Spacer(Modifier.width(9.dp))
        Column(modifier = Modifier.weight(1f)) {
            Text(
                text = tool.tool,
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.Medium,
            )
            tool.summary?.let {
                Text(
                    text = it,
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 3,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        Spacer(Modifier.width(8.dp))
        Text(
            text = buildString {
                append(statusLabel)
                tool.elapsedMs?.let { append(" · ${formatElapsed(it)}") }
            },
            style = MaterialTheme.typography.labelSmall,
            color = color,
        )
    }
}

@Composable
private fun NoticeTraceRow(notice: AgentRunNotice) {
    val color = when (notice.kind) {
        AgentRunNoticeKind.Info -> MaterialTheme.colorScheme.onSurfaceVariant
        AgentRunNoticeKind.Warning -> MaterialTheme.colorScheme.tertiary
        AgentRunNoticeKind.Error -> MaterialTheme.colorScheme.error
    }
    Text(
        text = notice.text,
        style = MaterialTheme.typography.bodySmall,
        color = color,
    )
}

@Composable
private fun TraceFooter(state: AgentRunState) {
    val usage = state.usage
    if (usage == null && state.formattedCost == null) return
    val parts = buildList {
        usage?.let {
            add("输入 ${it.inputTokens}")
            add("输出 ${it.outputTokens}")
            if (it.cacheReadTokens > 0) add("缓存 ${it.cacheReadTokens}")
        }
        state.formattedCost?.takeIf(String::isNotBlank)?.let(::add)
    }
    Text(
        text = parts.joinToString(" · "),
        style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun runStatusColor(outcome: AgentRunOutcome): Color = when (outcome) {
    AgentRunOutcome.Running -> MaterialTheme.colorScheme.primary
    AgentRunOutcome.Completed -> MaterialTheme.colorScheme.tertiary
    AgentRunOutcome.Failed -> MaterialTheme.colorScheme.error
    AgentRunOutcome.Cancelled -> MaterialTheme.colorScheme.onSurfaceVariant
}

@Composable
private fun toolStatusColor(status: AgentToolStatus): Color = when (status) {
    AgentToolStatus.Running -> MaterialTheme.colorScheme.primary
    AgentToolStatus.Completed -> MaterialTheme.colorScheme.tertiary
    AgentToolStatus.Failed -> MaterialTheme.colorScheme.error
    AgentToolStatus.Cancelled -> MaterialTheme.colorScheme.onSurfaceVariant
}

private fun formatElapsed(elapsedMs: Long): String {
    if (elapsedMs < 1_000) return "${elapsedMs}ms"
    val tenths = (elapsedMs + 50) / 100
    return "${tenths / 10}.${tenths % 10}s"
}
