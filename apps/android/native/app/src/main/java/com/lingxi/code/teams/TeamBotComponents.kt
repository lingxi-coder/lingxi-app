package com.lingxi.code.teams

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Surface
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.theme.BotAvatar

internal fun teamTaskStateRes(state: String): Int = when (state) {
    "queued" -> R.string.cron_status_queued
    "running" -> R.string.teams_run_processing
    "completed" -> R.string.chat_status_completed
    "failed" -> R.string.chat_status_failed
    "expired" -> R.string.teams_task_expired
    "interrupted" -> R.string.teams_task_interrupted
    "authorization_required" -> R.string.teams_task_authorization_required
    "revoked" -> R.string.teams_run_revoked
    else -> R.string.chat_status_unknown
}

@Composable
internal fun TeamTaskCard(task: TeamTask, botName: String, onArtifact: (String) -> Unit, onRefresh: (String) -> Unit = {}) {
    Column(Modifier.fillMaxWidth().padding(12.dp).testTag("teams.task.${task.taskId}"), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(botName, style = MaterialTheme.typography.titleSmall)
        TextButton(onClick = { onRefresh(task.taskId) }) { Text(stringResource(teamTaskStateRes(task.state)), style = MaterialTheme.typography.bodySmall) }
        task.artifactId?.let { id -> TextButton(onClick = { onArtifact(id) }) { Text(stringResource(R.string.teams_artifact_open)) } }
    }
}

@Composable
internal fun TeamArtifactViewer(artifact: TeamArtifact, onClose: () -> Unit) {
    androidx.compose.ui.window.Dialog(onDismissRequest = onClose, properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) {
            Column(Modifier.fillMaxSize().padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(artifact.title, style = MaterialTheme.typography.headlineSmall)
                androidx.compose.foundation.text.selection.SelectionContainer(Modifier.weight(1f).verticalScroll(rememberScrollState())) { Text(artifact.body) }
                Text("v${artifact.revision} · ${artifact.digest}", style = MaterialTheme.typography.labelSmall)
                TextButton(onClick = onClose) { Text(stringResource(R.string.common_close)) }
            }
        }
    }
}

@Composable
internal fun TeamBotCardView(bot: TeamBotCard, ownerName: String, uid: String, generation: Long, enabled: Boolean, onMessage: () -> Unit, onPause: () -> Unit, modifier: Modifier = Modifier) {
    var confirm by remember(bot.sharedBotId, bot.revision, bot.state) { mutableStateOf(false) }
    Column(modifier.fillMaxWidth().padding(vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            BotAvatar(bot.avatarId, 40.dp)
            Column {
                Text(bot.name, style = MaterialTheme.typography.titleMedium)
                if (bot.description.isNotBlank()) Text(bot.description, style = MaterialTheme.typography.bodyMedium)
                Text("${botAvailabilityText(bot)} · ${stringResource(R.string.teams_bot_owner_fmt, ownerName)}", style = MaterialTheme.typography.bodySmall)
                Text(stringResource(if (bot.usePolicy.getString("mode") == "all") R.string.teams_bot_use_all else R.string.teams_bot_use_listed), style = MaterialTheme.typography.bodySmall)
                Row {
                    TextButton(enabled = enabled && bot.selectable, onClick = onMessage) { Text(stringResource(R.string.teams_bot_message)) }
                    if (bot.canPause(uid, generation)) TextButton(enabled = enabled, onClick = { confirm = true }) { Text(stringResource(R.string.teams_bot_pause)) }
                }
            }
        }
    }
    if (confirm) AlertDialog(onDismissRequest = { confirm = false }, title = { Text(bot.name) }, text = { Text(stringResource(R.string.teams_bot_pause)) },
        confirmButton = { TextButton(onClick = { confirm = false; onPause() }) { Text(stringResource(R.string.teams_bot_pause)) } },
        dismissButton = { TextButton(onClick = { confirm = false }) { Text(stringResource(R.string.common_cancel)) } })
}
