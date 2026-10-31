package com.lingxi.code.teams

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

@Composable
internal fun TeamBotCardView(bot: TeamBotCard, ownerName: String, uid: String, enabled: Boolean, onMessage: () -> Unit, onUnbind: () -> Unit) {
    var confirm by remember(bot.key, bot.revision, bot.enabled) { mutableStateOf(false) }
    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(bot.name, style = MaterialTheme.typography.titleMedium)
            Text(bot.description)
            Text("Owner: $ownerName · ${if (bot.enabled && bot.online) "Online (heartbeat)" else "Offline"}", style = MaterialTheme.typography.bodySmall)
            Text("${if (bot.use == "all") "All team members" else "Selected members"} · ${if (bot.selectable) "You can use this Bot" else "Use not permitted"}", style = MaterialTheme.typography.bodySmall)
            if (!bot.enabled && bot.ownerUid == uid) Text(bot.inactiveLabel, style = MaterialTheme.typography.bodySmall)
            Text("Private conversation memory is scoped to the original human and membership. The Bot owner is a conversation reader. Presence does not guarantee availability.", style = MaterialTheme.typography.bodySmall)
            Row {
                TextButton(enabled = enabled && bot.selectable, onClick = onMessage) { Text("Message Bot") }
                if (bot.canUnbind(uid)) TextButton(enabled = enabled, onClick = { confirm = true }) { Text("Unbind") }
            }
        }
    }
    if (confirm && enabled && bot.canUnbind(uid)) AlertDialog(onDismissRequest = { confirm = false }, title = { Text("Unbind ${bot.name}?") }, text = { Text("This revokes this binding and its pending work. Bind and configure Bots on Desktop.") }, confirmButton = { TextButton(onClick = { confirm = false; onUnbind() }) { Text("Unbind") } }, dismissButton = { TextButton(onClick = { confirm = false }) { Text("Cancel") } })
}

@Composable
internal fun TeamBotQuestionView(message: TeamMessage, run: TeamBotRun?, uid: String, generation: Long, now: Long, offline: Boolean, pending: Boolean, onAnswer: (Map<String, String>) -> Unit) {
    val question = message.question ?: return
    // Other readers see the question text, never a proxy-answer or approval control.
    if (message.originUid != uid || message.originGeneration != generation) return
    var values by remember(message.id, question.id) { mutableStateOf(question.questions.associateWith { "" }) }
    if (pending || (run?.answered == true && run.questionId == question.id)) { Text(if (pending) "Answer saved; confirming delivery." else "Answered", style = MaterialTheme.typography.bodySmall); return }
    if (run?.canAnswer(message, uid, generation, now) != true || offline) {
        Text(if (run == null || offline) "Connect to check whether this question is still open." else "This question is closed or expired.", style = MaterialTheme.typography.bodySmall)
        return
    }
    question.questions.forEach { prompt -> OutlinedTextField(value = values[prompt].orEmpty(), onValueChange = { if (it.length <= 2000) values = values + (prompt to it) }, label = { Text(prompt) }, modifier = Modifier.fillMaxWidth()) }
    Button(enabled = values.values.all { it.isNotBlank() }, onClick = { onAnswer(values.toMap()) }) { Text("Answer") }
}

@Composable
internal fun TeamBotRecoveryView(triggerId: String, run: TeamBotRun, uid: String, now: Long, enabled: Boolean, onRecover: (String) -> Unit) {
    if (!run.canRecover(uid, now)) return
    var showing by remember(triggerId) { mutableStateOf(false) }
    var text by remember(triggerId) { mutableStateOf("") }
    TextButton(enabled = enabled, onClick = { showing = true }) { Text("Settle interrupted run") }
    if (showing) AlertDialog(onDismissRequest = { showing = false }, title = { Text("Settle interrupted run") }, text = {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Publish a manual settlement for this expired run. This does not execute the Bot or retry tools. Check any effects on Desktop before settling.")
            OutlinedTextField(value = text, onValueChange = { text = it }, label = { Text("Settlement message") })
        }
    }, confirmButton = { TextButton(enabled = text.isNotBlank() && text.toByteArray(Charsets.UTF_8).size <= 8192, onClick = { showing = false; onRecover(text) }) { Text("Publish settlement") } }, dismissButton = { TextButton(onClick = { showing = false }) { Text("Cancel") } })
}

internal fun teamBotStatus(state: String) = when (state) {
    "queued" -> "Queued"
    "claimed" -> "Claimed"
    "running" -> "Running"
    "recovery" -> "Interrupted; owner settlement required"
    "completed" -> "Completed"
    "failed" -> "Failed"
    "missed" -> "Missed"
    "cancelled" -> "Cancelled"
    "recovered" -> "Manually settled"
    "revoked" -> "Revoked"
    else -> "Checking status"
}
