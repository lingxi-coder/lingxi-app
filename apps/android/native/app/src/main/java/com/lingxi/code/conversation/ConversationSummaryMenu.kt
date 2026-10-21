package com.lingxi.code.conversation

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.MoreHoriz
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName

internal enum class ConversationSummaryCategory(val title: Int, val empty: Int, val icon: LXIconName) {
    Changes(R.string.chat_summary_changes, R.string.chat_summary_changes_empty, LXIconName.Edit),
    Tools(R.string.chat_summary_tools, R.string.chat_summary_tools_empty, LXIconName.Cog),
    Agents(R.string.chat_summary_agents, R.string.chat_summary_agents_empty, LXIconName.Workflow),
    Resources(R.string.chat_summary_resources, R.string.chat_summary_resources_empty, LXIconName.Paperclip),
    Plan(R.string.chat_summary_plan, R.string.chat_summary_plan_empty, LXIconName.Book),
}

/** Latest state wins for a tool use shared by the transcript and live run. */
internal fun summaryToolCalls(state: ChatState): List<ToolCallUi> = buildList {
    (state.messages + listOfNotNull(state.streamingMessage)).forEach { message ->
        message.blocks.filterIsInstance<MessageContent.Tool>().forEach { add(it.call) }
    }
    state.agentRunsByMessageId.values.forEach { run -> run.tools.forEach { add(it.toToolCall()) } }
    state.agentRun?.tools?.forEach { add(it.toToolCall()) }
}.associateBy { it.id }.values.toList()

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ConversationSummaryMenu(
    state: ChatState,
    onResumeWorkflow: (String) -> Unit = {},
    onToggleToolCall: (String) -> Unit = {},
) {
    var expanded by remember { mutableStateOf(false) }
    var selected by rememberSaveable(state.session.id) { mutableStateOf<ConversationSummaryCategory?>(null) }
    val detail = LocalConversationDetail.current
    Box {
        IconButton(onClick = { expanded = true }, modifier = Modifier.testTag("conversation.more")) {
            Icon(Icons.Outlined.MoreHoriz, contentDescription = stringResource(R.string.chat_summary_more),
                modifier = Modifier.size(22.dp), tint = MaterialTheme.colorScheme.onSurface)
        }
        DropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }) {
            ConversationSummaryCategory.entries.forEach { category ->
                DropdownMenuItem(text = { Text(stringResource(category.title)) },
                    leadingIcon = { LXIcon(category.icon) },
                    modifier = Modifier.testTag("conversation.summary.${category.name}"),
                    onClick = { expanded = false; selected = category })
            }
        }
    }
    selected?.let { category ->
        val calls = remember(state) { summaryToolCalls(state) }
        ModalBottomSheet(onDismissRequest = { selected = null },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
            LazyColumn(Modifier.fillMaxWidth().weight(1f, fill = false)
                .testTag("conversation.summary-sheet.${category.name}"),
                contentPadding = PaddingValues(start = 18.dp, end = 18.dp, bottom = 24.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp)) {
                item {
                    Row(Modifier.fillMaxWidth()) {
                        Column(Modifier.weight(1f)) {
                            Text(stringResource(category.title), style = MaterialTheme.typography.titleLarge)
                            Text(state.session.title, style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        IconButton(onClick = { selected = null }, modifier = Modifier.testTag("conversation.summary.close")) {
                            LXIcon(LXIconName.X, contentDescription = stringResource(R.string.common_close))
                        }
                    }
                }
                when (category) {
                    ConversationSummaryCategory.Changes -> {
                        val changes = calls.mapNotNull { it.display?.diff }
                        if (changes.isEmpty()) item { SummaryEmpty(category) }
                        items(changes) { diff ->
                            Text("${diff.filePath.orEmpty()}  +${diff.additions}  −${diff.removals}",
                                style = MaterialTheme.typography.labelLarge)
                            DiffView(diff)
                        }
                    }
                    ConversationSummaryCategory.Tools -> {
                        if (calls.isEmpty()) item { SummaryEmpty(category) }
                        items(calls, key = { it.id }) { call ->
                            var open by rememberSaveable(call.id) { mutableStateOf(false) }
                            CompositionLocalProvider(LocalConversationDetail provides null) {
                                ToolCallView(call, open, { open = !open })
                            }
                        }
                    }
                    ConversationSummaryCategory.Agents -> {
                        val agentRun = agentRunForBottomPanel(state)
                        if (state.sessionAgents.isEmpty() && state.backgroundTasks.isEmpty() &&
                            state.workflowRuns.isEmpty() && agentRun == null) {
                            item { SummaryEmpty(category) }
                        } else {
                            item {
                                CompositionLocalProvider(LocalConversationDetail provides { key ->
                                    selected = null
                                    detail?.invoke(key)
                                }) {
                                    ExecutionStatusPanel(
                                        tasks = state.backgroundTasks.values.toList(),
                                        workflows = state.workflowRuns.values.toList(),
                                        agents = state.sessionAgents,
                                        planTasks = emptyList(),
                                        planExpanded = false,
                                        onTogglePlan = {},
                                        onResumeWorkflow = onResumeWorkflow,
                                        resumeState = state.workflowResumeState,
                                        agentRun = agentRun,
                                        expandedToolCalls = state.expandedToolCalls,
                                        onToggleToolCall = onToggleToolCall,
                                    )
                                }
                            }
                        }
                    }
                    ConversationSummaryCategory.Resources -> {
                        val attachments = state.messages.filter { it.images.isNotEmpty() }
                        val apps = calls.mapNotNull { it.localApp }.distinctBy { it.id }
                        val visuals = state.messages.flatMap { it.blocks }.filterIsInstance<MessageContent.Visualization>()
                        if (attachments.isEmpty() && apps.isEmpty() && visuals.isEmpty()) item { SummaryEmpty(category) }
                        items(attachments, key = { it.id }) { MessageBubble(it.copy(text = "", blocks = emptyList())) }
                        items(apps, key = { it.id }) { LocalAppCard(it) }
                        items(visuals) { visualization ->
                            VisualizationCard(visualization.status, visualization.reference)
                        }
                    }
                    ConversationSummaryCategory.Plan -> {
                        val documents = calls.mapNotNull { it.planMarkdown }.distinct()
                        if (state.planTasks.isEmpty() && documents.isEmpty()) item { SummaryEmpty(category) }
                        item { PlanTasksPanel(state.planTasks, true, {}) }
                        items(documents) { AIText(it) }
                    }
                }
            }
        }
    }
}

@Composable
private fun SummaryEmpty(category: ConversationSummaryCategory) {
    Text(stringResource(category.empty), style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(vertical = 24.dp))
}
