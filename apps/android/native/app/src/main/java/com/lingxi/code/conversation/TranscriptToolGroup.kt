package com.lingxi.code.conversation

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
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
            } else pending.add(block.call)
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

internal fun toolIconName(verb: ToolVerbUi?) = when (verb) {
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

@Composable
internal fun ToolGroupView(group: TranscriptBlock.Tools, expandedIds: Set<String>, onToggle: (String) -> Unit) {
    if (group.calls.any { it.questionAnswers != null }) {
        Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
            group.calls.forEach { call -> ToolCallView(call, call.id in expandedIds, { onToggle(call.id) }) }
        }
        return
    }
    val t = LingXiTheme.palette
    val open = group.id in expandedIds
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (!group.running) {
            val last = group.summary
            val title = toolCallTitle(last)
            val detail = last.header?.subLine?.let { " · ${it.prefix}${it.text}" }.orEmpty()
            val failed = group.calls.count { it.status == AgentToolStatus.Failed }
            Row(
                modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp)
                    .clickable(role = Role.Button) { onToggle(group.id) }
                    .testTag("conversation.tool.group"),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                LXIcon(toolIconName(last.header?.verb), size = 18.dp, color = t.text3)
                Text(title + detail, modifier = Modifier.weight(1f), color = t.text3, fontSize = 13.sp,
                    maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (failed > 0) Text("! $failed", color = t.danger, fontSize = 12.sp)
                LXIcon(if (open) LXIconName.Chevron else LXIconName.ChevronR, size = 12.dp, color = t.text4)
            }
        }
        if (group.running || open) {
            group.visibleTools.forEach { call ->
                ToolCallView(call, call.id in expandedIds, { onToggle(call.id) })
            }
        }
    }
}
