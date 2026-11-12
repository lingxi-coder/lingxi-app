package com.lingxi.code.conversation

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.components.UiTags
import com.lingxi.code.theme.LingXiTheme

/**
 * Bounded, latest-turn execution trace backed by real engine events.
 *
 * Reasoning, notices, usage and cost are UI state rather than session JSONL.
 * While running the card is pinned above the composer; once terminal it is
 * anchored after the assistant message so a later turn cannot hide the result.
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
    agents: List<SessionAgentUi> = emptyList(),
) {
    // Keep one fixed header; execution phases only replace its content.
    var expanded by rememberSaveable(state.turnId) { mutableStateOf(false) }
    val title = when (state.outcome) {
        AgentRunOutcome.Running -> stringResource(R.string.chat_run_status_running)
        AgentRunOutcome.Completed -> stringResource(R.string.chat_run_status_completed)
        AgentRunOutcome.Failed -> stringResource(R.string.chat_run_status_failed)
        AgentRunOutcome.Cancelled -> stringResource(R.string.chat_run_status_cancelled)
        AgentRunOutcome.Finished -> stringResource(R.string.chat_run_status_finished)
    }
    val activity = state.activitySummaryText() ?: title
    val runAccessibilityLabel = stringResource(R.string.chat_run_accessibility_label, title)
    // One persistent disclosure header owns both reasoning and tool activity.
    // No conditional status/body is inserted when the execution phase changes.
    Column(modifier.fillMaxWidth().testTag(UiTags.AGENT_RUN_TIMELINE)
        .semantics { contentDescription = runAccessibilityLabel }) {
        Row(Modifier.fillMaxWidth().heightIn(min = 44.dp)
            .clickable(role = Role.Button) { expanded = !expanded }
            .testTag("conversation.activity.header"),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(9.dp)) {
            LXIcon(
                name = state.tools.lastOrNull()?.let { toolIconName(it.header?.verb, it.tool) }
                    ?: LXIconName.Workflow,
                size = 18.dp, color = LingXiTheme.palette.text3)
            Text(activity, Modifier.weight(1f), color = LingXiTheme.palette.text3,
                style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (state.tools.any { it.status == AgentToolStatus.Failed }) {
                Text(stringResource(R.string.chat_tool_calls_unsuccessful), color = LingXiTheme.palette.danger,
                    style = MaterialTheme.typography.labelSmall, maxLines = 1)
            }
            LXIcon(
                if (expanded) LXIconName.Chevron else LXIconName.ChevronR,
                size = 12.dp, color = LingXiTheme.palette.text4)
        }
        if (expanded) {
            Column(Modifier.padding(start = 24.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                val hiddenTools = state.hiddenToolCount()
                if (hiddenTools > 0) Text(stringResource(R.string.chat_run_tools_windowed, hiddenTools),
                    style = MaterialTheme.typography.labelSmall)
                val tools = state.toolDisplayWindow().map { it.toToolCall() }
                if (tools.isNotEmpty()) ToolGroupView(TranscriptBlock.Tools(tools, state.tools.first().id),
                    expandedToolCalls, onToggleToolCall, agents)
                // Retain the reasoning body across tool starts and completion;
                // it never appears as a separate collapsed transcript row.
                if (state.reasoning.isNotBlank()) Text(state.reasoning,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                if (state.reasoningTruncated) Text(stringResource(R.string.chat_run_reasoning_truncated),
                    style = MaterialTheme.typography.labelSmall)
                state.notices.forEach { NoticeTraceRow(it) }
                if (state.activeWorkers > 0 || state.teamName != null) Text(
                    stringResource(R.string.chat_run_collaborators_label, state.activeWorkers, state.teamName?.let { " · $it" }.orEmpty()),
                    style = MaterialTheme.typography.labelMedium)
                TraceFooter(state)
            }
        }
    }
}

/** Header content changes; the caller keeps one stable row for the whole turn. */
internal fun AgentRunState.activitySummaryText(): String? {
    val running = tools.lastOrNull { it.status == AgentToolStatus.Running }
    if (running != null) return running.header?.title ?: running.tool
    if (active && reasoningActive && reasoning.isNotBlank()) return reasoning.takeLast(240).replace('\n', ' ').replace('\r', ' ')
    if (tools.isNotEmpty()) return tools.map { it.header?.label ?: it.tool }.distinct().joinToString(" · ")
    return null
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
