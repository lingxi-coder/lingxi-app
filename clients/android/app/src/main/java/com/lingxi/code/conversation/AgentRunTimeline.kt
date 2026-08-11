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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.components.UiTags

/**
 * Bounded, latest-turn execution trace backed by real engine events.
 *
 * Reasoning, notices, usage and cost are intentionally transient UI state: not
 * reconstructed as chat messages, not written into the session JSONL. The card
 * stays available after completion until the next turn starts.
 *
 * TOOL ROWS ARE THE EXCEPTION. They are the only record of a live turn's tool
 * calls, and the next turn replaces this whole state — so when a turn settles,
 * [ChatState.settleTurn] MOVES its non-shell rows into the transcript message
 * and this card is left without them (a resumed transcript rebuilds the same
 * rows inline, so the two now agree). Shell calls stay: they own a persistent
 * terminal card of their own.
 */
@Composable
internal fun AgentRunTimeline(
    state: AgentRunState,
    modifier: Modifier = Modifier,
    /** Tool-use ids whose result body/diff is expanded — owned by [ChatState]. */
    expandedToolCalls: Set<String> = emptySet(),
    onToggleToolCall: (String) -> Unit = {},
) {
    var expanded by rememberSaveable(state.turnId) { mutableStateOf(true) }
    val title = when (state.outcome) {
        AgentRunOutcome.Running -> stringResource(R.string.chat_run_status_running)
        AgentRunOutcome.Completed -> stringResource(R.string.chat_run_status_completed)
        AgentRunOutcome.Failed -> stringResource(R.string.chat_run_status_failed)
        AgentRunOutcome.Cancelled -> stringResource(R.string.chat_run_status_cancelled)
    }
    val statusColor = runStatusColor(state.outcome)
    val runAccessibilityLabel = stringResource(R.string.chat_run_accessibility_label, title)
    Card(
        // The card-wide `clickable` used to live HERE, which swallowed every tap
        // inside it: a per-tool-call expand toggle nested in the body could never
        // fire, because the card's own gesture consumed it first. The collapse
        // gesture now lives on the header Row below, so the tool rows own their
        // own taps.
        modifier = modifier
            .fillMaxWidth()
            .testTag(UiTags.AGENT_RUN_TIMELINE)
            .semantics { contentDescription = runAccessibilityLabel },
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
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable(role = Role.Button) { expanded = !expanded },
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
                    state.activeWorkers > 0 ->
                        stringResource(R.string.chat_run_active_collaborators, state.activeWorkers)
                    state.tools.any { it.status == AgentToolStatus.Running } ->
                        stringResource(R.string.chat_run_tool_executing)
                    state.reasoningActive -> stringResource(R.string.chat_run_reasoning_active)
                    state.active -> stringResource(R.string.chat_run_generating)
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
                    text = if (expanded) {
                        stringResource(R.string.chat_run_collapse)
                    } else {
                        stringResource(R.string.chat_run_expand)
                    },
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.primary,
                )
            }

            AnimatedVisibility(expanded) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    if (state.reasoning.isNotBlank() || state.reasoningActive) {
                        val reasoningTitle = if (state.reasoningActive) {
                            stringResource(R.string.chat_run_reasoning_active)
                        } else {
                            stringResource(R.string.chat_thinking)
                        }
                        TraceSection(title = reasoningTitle) {
                            Text(
                                text = state.reasoning.ifBlank {
                                    stringResource(R.string.chat_run_analyzing_request)
                                },
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 16,
                                overflow = TextOverflow.Ellipsis,
                            )
                            if (state.reasoningTruncated) {
                                Text(
                                    text = stringResource(R.string.chat_run_reasoning_truncated),
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                        }
                    }

                    // A DISPLAY window, not storage: this card is a plain
                    // `Column`, so every row it lists is composed. The rows it
                    // leaves out are still in `state.tools` and still settle
                    // into the transcript — dropping them from the state was a
                    // permanent transcript loss, this is only a scroll budget.
                    val hiddenTools = state.hiddenToolCount()
                    if (hiddenTools > 0) {
                        Text(
                            text = stringResource(
                                R.string.chat_run_tools_windowed,
                                hiddenTools,
                            ),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    state.toolDisplayWindow().forEach { tool ->
                        if (tool.header != null || tool.display != null) {
                            // The engine derived this row's presentation already —
                            // header, `⎿` headline, diff/body, collapse verdict.
                            ToolCallView(
                                call = tool.toToolCall(),
                                expanded = tool.id in expandedToolCalls,
                                onToggleExpanded = { onToggleToolCall(tool.id) },
                                // Liveness only matters on the RUNNING trace; the
                                // settled transcript has no elapsed column.
                                trailing = tool.elapsedMs?.let(::formatElapsed),
                            )
                        } else {
                            // Older engine: the pre-derivation row.
                            ToolTraceRow(tool)
                        }
                    }

                    state.notices.forEach { notice ->
                        NoticeTraceRow(notice)
                    }

                    if (state.activeWorkers > 0 || state.teamName != null) {
                        val team = state.teamName?.let { " · $it" }.orEmpty()
                        Text(
                            text = stringResource(
                                R.string.chat_run_collaborators_label,
                                state.activeWorkers,
                                team,
                            ),
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
        AgentToolStatus.Running -> stringResource(R.string.chat_status_running)
        AgentToolStatus.Completed -> stringResource(R.string.chat_tool_status_completed)
        AgentToolStatus.Failed -> stringResource(R.string.chat_status_failed)
        AgentToolStatus.Cancelled -> stringResource(R.string.chat_status_cancelled)
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
    val inputLabel = stringResource(R.string.chat_usage_input)
    val outputLabel = stringResource(R.string.chat_usage_output)
    val cacheLabel = stringResource(R.string.chat_usage_cache)
    val parts = buildList {
        usage?.let {
            add("$inputLabel ${it.inputTokens}")
            add("$outputLabel ${it.outputTokens}")
            if (it.cacheReadTokens > 0) add("$cacheLabel ${it.cacheReadTokens}")
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
