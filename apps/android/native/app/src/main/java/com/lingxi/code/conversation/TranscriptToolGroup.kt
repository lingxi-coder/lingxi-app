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
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
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

internal fun isAgentLaunchTool(tool: String): Boolean = tool.lowercase() in setOf("agent", "task", "spawn_agent", "fork_agent")
internal fun isTaskProgressTool(tool: String): Boolean = Regex("^(Task_?(Create|Get|Update|List)|Todo_?Write|List_?Checks|update_plan)$", RegexOption.IGNORE_CASE).matches(tool)
internal fun isTaskWorkflow(call: ToolCallUi): Boolean {
    fun member(name: String) = isTaskProgressTool(name) || name.lowercase() in setOf("sendmessage", "send_message")
    if (member(call.tool)) return true
    if (call.tool != "ToolSearch") return false
    val targets = call.header?.primary?.trim()?.replace(Regex("^select:\\s*", RegexOption.IGNORE_CASE), "")
        ?.split(Regex("[,\\s]+"))?.map { it.removePrefix("+") } ?: return false
    return targets.isNotEmpty() && targets.all(::member)
}

/** A nil key makes questions, permissions, deliverables and agents standalone. */
internal fun toolGroupingKey(call: ToolCallUi): String? {
    if (isAgentLaunchTool(call.tool) && call.planMarkdown == null && call.questionAnswers == null && call.localApp == null) return "agent-launch"
    if (call.spawnedAgentId != null || call.planMarkdown != null || call.questionAnswers != null || call.localApp != null ||
        call.tool.lowercase() in setOf("agent", "task", "spawn_agent", "fork_agent", "askuserquestion", "exitplanmode", "enterplanmode", "submitplan") ||
        call.tool.contains("permission", ignoreCase = true)) return null
    if (call.tool.equals("Skill", ignoreCase = true) || call.header?.verb == ToolVerbUi.Skill) return "skill"
    if (isTaskWorkflow(call)) return "task-workflow"
    if (call.header?.verb in setOf(ToolVerbUi.Task, ToolVerbUi.Todo)) return null
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
    tool.equals("computer", ignoreCase = true) -> LXIconName.Computer
    tool == "LocalApp" -> LXIconName.Apps
    tool.equals("browser", ignoreCase = true) -> LXIconName.Globe
    isTaskProgressTool(tool) -> LXIconName.ListChecks
    tool.lowercase() in setOf("sendmessage", "send_message") || tool.lowercase().endsWith("__send_message") -> LXIconName.Send
    tool == "ToolSearch" -> LXIconName.Search
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
internal fun ToolGroupView(group: TranscriptBlock.Tools, expandedIds: Set<String>, onToggle: (String) -> Unit, agents: List<SessionAgentUi> = emptyList()) {
    val groups = consecutiveToolGroups(group.calls)
    if (groups.size > 1) {
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            groups.forEach { calls -> ToolGroupView(TranscriptBlock.Tools(calls), expandedIds, onToggle, agents) }
        }
        return
    }
    if (group.calls.all { isAgentLaunchTool(it.tool) }) {
        AgentLaunchGroup(group, expandedIds, onToggle, agents)
        return
    }
    if (group.calls.size == 1) {
        val call = group.calls.single()
        ToolCallView(call, call.id in expandedIds, { onToggle(call.id) })
        return
    }
    val t = LingXiTheme.palette
    val open = group.id in expandedIds
    val disclosureState = stringResource(if (open) R.string.chat_run_collapse else R.string.chat_run_expand)
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val hovered by interaction.collectIsHoveredAsState()
    val first = group.calls.first()
    val workflow = group.calls.all(::isTaskWorkflow)
    val label = if (workflow) stringResource(if (group.calls.all { isTaskProgressTool(it.tool) }) R.string.chat_tool_group_tasks else R.string.chat_tool_group_coordination) else first.header?.verb?.let { verb -> when (verb) {
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
                .semantics { stateDescription = disclosureState }
                .testTag("conversation.tool.group"),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            LXIcon(if (workflow) LXIconName.ListChecks else toolIconName(first.header?.verb, first.tool), size = 18.dp, color = t.text3)
            Text(label + " · " + stringResource(R.string.chat_tool_calls_count_fmt, group.calls.size),
                modifier = Modifier.weight(1f, fill = false), color = if (open || focused || hovered) t.text else t.text3,
                fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (group.running) Text(stringResource(R.string.chat_tool_running_count_fmt, group.calls.count { it.status == AgentToolStatus.Running }), color = t.text3, fontSize = 12.sp)
            else if (group.calls.all { it.status == AgentToolStatus.Failed }) Text(stringResource(R.string.chat_tool_calls_unsuccessful), color = t.text3, fontSize = 12.sp)
            if (workflow && group.calls.any { it.status == AgentToolStatus.Failed }) Text(
                stringResource(R.string.chat_tool_calls_unsuccessful), color = t.danger, fontSize = 12.sp, maxLines = 1)
            LXIcon(if (open) LXIconName.Chevron else LXIconName.ChevronR,
                modifier = Modifier.alpha(if (open || focused || hovered) 1f else 0.75f), size = 12.dp, color = t.text4)
        }
        if (open) Column {
            group.calls.forEach { call -> ToolCallView(call, call.id in expandedIds, { onToggle(call.id) }) }
        }
    }
}

/** Child execution and report delivery use the same single-line disclosure as Desktop. */
@Composable
private fun AgentLaunchGroup(group: TranscriptBlock.Tools, expandedIds: Set<String>, onToggle: (String) -> Unit, agents: List<SessionAgentUi>) {
    val palette = LingXiTheme.palette
    val open = group.id in expandedIds
    fun agent(call: ToolCallUi) = agents.firstOrNull { it.agentId.removePrefix("agent:") == call.spawnedAgentId?.removePrefix("agent:") }
    fun name(call: ToolCallUi) = agent(call)?.name?.takeIf(String::isNotBlank) ?: call.header?.primary ?: call.tool
    @Composable fun status(call: ToolCallUi): String = agent(call)?.transcriptStatusText() ?: toolCallStatusLabel(call.status)
    val statuses = group.calls.map { status(it) }
    val failedStates = group.calls.filter { it.status == AgentToolStatus.Failed || agent(it)?.reportDelivery == "withheld" }.map { status(it) }.toSet()
    val summary = buildAnnotatedString {
        val states = statuses.groupingBy { it }.eachCount().entries.sortedByDescending { it.key in failedStates }
        states.forEachIndexed { index, entry ->
            if (index > 0) append(" · ")
            withStyle(SpanStyle(color = if (entry.key in failedStates) palette.danger else palette.text3)) {
                if (states.size > 1) append("${entry.value} ")
                append(entry.key)
            }
        }
    }
    val detailHost = LocalConversationDetail.current
    val statusMaxWidth = (LocalConfiguration.current.screenWidthDp * 0.45f).dp
    fun select(call: ToolCallUi) {
        val child = agent(call)
        if (detailHost != null) detailHost(if (child != null) "agent:${child.agentId}" else "tool:${call.id}")
        else onToggle(call.id)
    }
    Column {
        Row(Modifier.heightIn(min = 44.dp).clickable(role = Role.Button) { if (group.calls.size == 1 && detailHost != null) select(group.calls.first()) else onToggle(group.id) }.testTag("conversation.agent-group"),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(9.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(2.dp)) {
                group.calls.take(3).forEach { call -> com.lingxi.code.theme.AgentAvatar(agentId = call.spawnedAgentId ?: call.id, modifier = Modifier.size(18.dp)) }
                if (group.calls.size > 3) Text("+${group.calls.size - 3}", color = palette.text3, fontSize = 11.sp)
            }
            Text(group.calls.joinToString(", ", transform = ::name), Modifier.weight(1f, fill = false), color = palette.text3, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(summary, modifier = Modifier.widthIn(max = statusMaxWidth), fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            LXIcon(if (open) LXIconName.Chevron else LXIconName.ChevronR, size = 12.dp, color = palette.text3)
        }
        if (open) Column(Modifier.padding(start = 24.dp)) {
            group.calls.forEach { call ->
                Row(Modifier.heightIn(min = 44.dp).clickable(role = Role.Button) { select(call) }, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(9.dp)) {
                    com.lingxi.code.theme.AgentAvatar(agentId = call.spawnedAgentId ?: call.id, modifier = Modifier.size(18.dp))
                    Text(name(call), Modifier.weight(1f, fill = false), color = palette.text3, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(status(call), Modifier.widthIn(max = statusMaxWidth), color = if (status(call) in failedStates) palette.danger else palette.text3, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    LXIcon(LXIconName.ChevronR, size = 12.dp, color = palette.text3)
                }
                if (call.id in expandedIds) ToolCallView(call, true, { onToggle(call.id) })
            }
        }
    }
}
