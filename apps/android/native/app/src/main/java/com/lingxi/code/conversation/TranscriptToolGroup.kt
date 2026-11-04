package com.lingxi.code.conversation

import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.interaction.collectIsHoveredAsState
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.theme.LingXiTheme

/** Presentation grouping never changes durable tool order or removes completed results. */
internal sealed interface TranscriptBlock {
    data class Prose(val text: String) : TranscriptBlock
    data class Plan(val markdown: String, val writing: Boolean, val id: String) : TranscriptBlock
    /** [ordinal] counts the message's visualization slots, keying the row across settle. */
    data class Visualization(
        val status: VisualizationSlotStatus,
        val reference: VisualizationRef?,
        val ordinal: Int,
    ) : TranscriptBlock
    data class Tools(val calls: List<ToolCallUi>, val firstToolId: String = calls.first().id) : TranscriptBlock {
        val id get() = "tool-group:$firstToolId"
        val summary get() = calls.last()
        val running get() = calls.any { it.status == AgentToolStatus.Running }
        val visibleTools get() = if (running) calls.filter { it.status == AgentToolStatus.Running } else calls
    }
}

/** A nil key makes questions, permissions, deliverables and agents standalone. */
internal fun toolGroupingKey(call: ToolCallUi): String? {
    if (call.planMarkdown != null || call.questionAnswers != null || call.localApp != null ||
        call.tool.lowercase() in setOf("agent", "task", "skill", "spawn_agent", "fork_agent", "askuserquestion", "exitplanmode", "enterplanmode", "submitplan") ||
        call.tool.contains("permission", ignoreCase = true) ||
        call.header?.verb in setOf(ToolVerbUi.Task, ToolVerbUi.Todo, ToolVerbUi.Skill)) return null
    if (call.tool.startsWith("mcp", ignoreCase = true)) return call.tool
    return call.header?.verb?.takeIf { it != ToolVerbUi.Generic }?.name ?: call.tool.takeIf { it.isNotEmpty() }
}

internal fun sameToolGroup(first: ToolCallUi, next: ToolCallUi): Boolean =
    toolGroupingKey(first)?.let { it == toolGroupingKey(next) } == true

internal fun consecutiveToolGroups(calls: List<ToolCallUi>): List<List<ToolCallUi>> {
    val groups = mutableListOf<MutableList<ToolCallUi>>()
    calls.forEach { call ->
        val last = groups.lastOrNull()
        if (last != null && sameToolGroup(last.last(), call)) last.add(call)
        else groups.add(mutableListOf(call))
    }
    return groups
}

internal fun transcriptBlocks(blocks: List<MessageContent>): List<TranscriptBlock> = buildList {
    val pending = mutableListOf<ToolCallUi>()
    var visualizations = 0
    fun flush() {
        if (pending.isNotEmpty()) { add(TranscriptBlock.Tools(pending.toList())); pending.clear() }
    }
    blocks.forEach { block ->
        when (block) {
            is MessageContent.Tool -> if (block.call.planMarkdown != null) {
                flush()
                add(TranscriptBlock.Plan(block.call.planMarkdown, block.call.status == AgentToolStatus.Running, block.call.id))
            } else {
                if (pending.lastOrNull()?.let { !sameToolGroup(it, block.call) } == true) flush()
                pending.add(block.call)
            }
            is MessageContent.Text -> if (block.text.isNotBlank()) {
                flush()
                add(TranscriptBlock.Prose(block.text))
            }
            is MessageContent.Visualization -> {
                flush()
                add(TranscriptBlock.Visualization(block.status, block.reference, visualizations++))
            }
        }
    }
    flush()
}

internal fun toolIconName(verb: ToolVerbUi?, tool: String = "") = when {
    tool.startsWith("mcp__", ignoreCase = true) && (verb == null || verb == ToolVerbUi.Generic) -> LXIconName.McpFallback
    else -> when (verb) {
    ToolVerbUi.Read -> LXIconName.Book
    ToolVerbUi.Search -> LXIconName.Search
    ToolVerbUi.Update, ToolVerbUi.Create -> LXIconName.Edit
    ToolVerbUi.Shell, ToolVerbUi.Output, ToolVerbUi.Kill -> LXIconName.Terminal
    ToolVerbUi.Fetch -> LXIconName.Link
    ToolVerbUi.Task -> LXIconName.Workflow
    ToolVerbUi.Todo -> LXIconName.Check
    ToolVerbUi.Skill -> LXIconName.Skill
    else -> LXIconName.Plug
    }
}

@Composable
internal fun ToolGroupView(group: TranscriptBlock.Tools, expandedIds: Set<String>, onToggle: (String) -> Unit) {
    val groups = consecutiveToolGroups(group.calls)
    if (groups.size > 1) {
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            groups.forEach { calls -> ToolGroupView(TranscriptBlock.Tools(calls), expandedIds, onToggle) }
        }
        return
    }
    if (group.calls.size == 1) {
        val call = group.calls.single()
        ToolCallView(call, call.id in expandedIds, { onToggle(call.id) })
        return
    }
    val t = LingXiTheme.palette
    val open = group.id in expandedIds
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val hovered by interaction.collectIsHoveredAsState()
    val first = group.calls.first()
    val label = first.header?.verb?.let { verb -> when (verb) {
        ToolVerbUi.Read -> stringResource(R.string.chat_tool_verb_read)
        ToolVerbUi.Search -> stringResource(R.string.chat_tool_verb_search)
        ToolVerbUi.Update -> stringResource(R.string.chat_tool_verb_update)
        ToolVerbUi.Create -> stringResource(R.string.chat_tool_verb_create)
        ToolVerbUi.Fetch -> stringResource(R.string.chat_tool_verb_fetch)
        ToolVerbUi.Shell -> stringResource(R.string.chat_tool_group_shell)
        else -> first.header.label
    } } ?: first.tool
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp)
                .clickable(interactionSource = interaction, indication = null, role = Role.Button) { onToggle(group.id) }
                .testTag("conversation.tool.group"),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            LXIcon(toolIconName(first.header?.verb, first.tool), size = 18.dp, color = t.text3)
            Text(label + " · " + stringResource(R.string.chat_tool_calls_count, group.calls.size),
                modifier = Modifier.weight(1f), color = if (focused || hovered) t.text else t.text3,
                fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (group.running) Text(stringResource(R.string.chat_tool_running_count, group.calls.count { it.status == AgentToolStatus.Running }), color = t.text3, fontSize = 12.sp)
            else if (group.calls.all { it.status == AgentToolStatus.Failed }) Text(stringResource(R.string.chat_tool_calls_unsuccessful), color = t.text3, fontSize = 12.sp)
            LXIcon(if (open) LXIconName.Chevron else LXIconName.ChevronR,
                modifier = Modifier.alpha(if (open || focused || hovered) 1f else 0f), size = 12.dp, color = t.text4)
        }
        val visible = if (open) group.calls else group.calls.filter { it.status == AgentToolStatus.Running }
        Column(Modifier.padding(start = 24.dp)) {
            visible.forEach { call -> ToolCallView(call, call.id in expandedIds, { onToggle(call.id) }) }
        }
    }
}
