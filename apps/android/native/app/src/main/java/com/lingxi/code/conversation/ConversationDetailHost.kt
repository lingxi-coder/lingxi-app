package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.lingxi.code.R
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.theme.LingXiTheme

/** Detail navigation belongs to the workspace, not to recycled transcript rows. */
internal val LocalConversationDetail = staticCompositionLocalOf<((String) -> Unit)?> { null }

@Composable
internal fun ConversationDetailHost(state: ChatState, content: @Composable () -> Unit) {
    var selected by rememberSaveable(state.session.id) { mutableStateOf<String?>(null) }
    val close = { selected = null }
    androidx.activity.compose.BackHandler(enabled = selected != null, onBack = close)
    CompositionLocalProvider(LocalConversationDetail provides { selected = it }) {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            val wide = maxWidth >= 840.dp
            Row(Modifier.fillMaxSize()) {
                Box(Modifier.weight(1f).fillMaxHeight()) { content() }
                if (wide && selected != null) {
                    Box(Modifier.width(360.dp).fillMaxHeight()) {
                        ConversationDetailContent(selected!!, state, close)
                    }
                }
            }
            if (!wide && selected != null) {
                Dialog(onDismissRequest = close, properties = DialogProperties(usePlatformDefaultWidth = false)) {
                    ConversationDetailContent(selected!!, state, close)
                }
            }
        }
    }
}

@Composable
private fun ConversationDetailContent(key: String, state: ChatState, onClose: () -> Unit) {
    val id = key.substringAfter(':')
    val call = if (key.startsWith("tool:")) {
        state.messages.asSequence().flatMap { it.blocks.asSequence() }
            .filterIsInstance<MessageContent.Tool>().firstOrNull { it.call.id == id }?.call
            ?: state.agentRun?.tools?.firstOrNull { it.id == id }?.toToolCall()
            ?: state.agentRunsByMessageId.values.asSequence().flatMap { it.tools.asSequence() }
                .firstOrNull { it.id == id }?.toToolCall()
    } else null
    val agent = state.sessionAgents.firstOrNull { key == "agent:${it.agentId}" }
    val task = state.backgroundTasks[id].takeIf { key.startsWith("task:") }
    val workflow = state.workflowRuns.values.firstOrNull { it.taskId == id }
    val title = when {
        call != null -> toolCallTitle(call)
        agent != null -> agent.name.ifBlank { agent.agentId }
        task != null -> task.description.ifBlank { task.taskId }
        else -> id
    }
    val palette = LingXiTheme.palette
    Column(Modifier.fillMaxSize().background(palette.surface).safeDrawingPadding().padding(20.dp)
        .verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Row(Modifier.fillMaxWidth()) {
            Text(title, Modifier.weight(1f), color = palette.text, fontSize = 18.sp)
            IconButton(onClick = onClose) {
                LXIcon(LXIconName.X, color = palette.text3,
                    contentDescription = stringResource(R.string.chat_run_collapse))
            }
        }
        when {
            call != null -> ToolResultContent(call)
            agent != null -> {
                com.lingxi.code.theme.AgentAvatar(agent.agentId, Modifier.size(48.dp))
                Text(listOfNotNull(agent.agentId, agent.agentType, agent.model, agent.status, agent.latestActivity)
                    .filter(String::isNotBlank).joinToString("\n\n"), color = palette.text2, fontSize = 15.sp)
            }
            task != null -> Text(listOfNotNull(task.taskId, taskStatusText(task.status),
                workflow?.currentPhaseTitle, workflow?.latestLog, task.error).joinToString("\n\n"),
                color = palette.text2, fontSize = 15.sp, lineHeight = 24.sp)
        }
    }
}
