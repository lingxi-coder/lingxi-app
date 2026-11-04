package com.lingxi.code.teams

import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.theme.BotAvatar
import org.json.JSONObject

/** The state label of a Bot run (SPEC-mobile-team-bot-ui section 6.3). */
@StringRes
internal fun teamBotRunStateRes(state: String): Int = when (state) {
    "queued", "pending" -> R.string.cron_status_queued
    "claimed" -> R.string.teams_run_preparing
    "running" -> R.string.teams_run_processing
    "recovery" -> R.string.teams_run_recovery
    "recovered" -> R.string.teams_run_recovered
    "completed" -> R.string.chat_status_completed
    "failed" -> R.string.chat_status_failed
    "missed" -> R.string.teams_run_missed
    "cancelled" -> R.string.chat_status_cancelled
    "revoked" -> R.string.teams_run_revoked
    else -> R.string.teams_loading
}

/** What the owner's Desktop says about a run (only the owner sees it). */
@StringRes
internal fun teamBotHostStatusRes(status: String): Int? = when (status) {
    "retrying" -> R.string.teams_run_host_retrying
    "result_pending" -> R.string.teams_run_host_result_pending
    "recovery_required" -> R.string.teams_run_host_recovery_required
    "model_unavailable" -> R.string.teams_run_host_model_unavailable
    "runtime_unavailable" -> R.string.teams_run_host_runtime_unavailable
    "storage_error" -> R.string.teams_run_host_storage_error
    else -> null
}

/**
 * The run card under the latest message of a Bot trigger: `"<Bot name> - <state>"`, the owner's host-status line and, for the
 * owner of an interrupted run, the recovery form (or the saved / rejected settlement that replaced it).
 */
@Composable
internal fun TeamBotRunCard(
    botName: String, run: TeamBotRun, trigger: String, uid: String, now: Long, online: Boolean, settlement: JSONObject?,
    onRecover: (String) -> Unit, onDiscardSettlement: (String) -> Unit, modifier: Modifier = Modifier,
) {
    Column(modifier.testTag(TeamsTags.runCard(trigger)).padding(top = 4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text("$botName · ${stringResource(teamBotRunStateRes(run.state))}", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        val host = run.hostStatus?.takeIf { uid == run.ownerUid }?.let(::teamBotHostStatusRes)
        if (host != null) Text(stringResource(host), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (settlement != null) {
            val failed = settlement.has("error")
            Text(stringResource(if (failed) R.string.teams_recover_rejected else R.string.teams_recover_saved), style = MaterialTheme.typography.bodySmall, color = if (failed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
            Text(settlement.getJSONObject("body").getString("text"), style = MaterialTheme.typography.bodyMedium)
            if (failed || canDiscardTeamBotIntent(settlement, now)) TextButton(onClick = { onDiscardSettlement(settlement.getString("settlementId")) }) { Text(stringResource(R.string.teams_recover_discard)) }
        } else if (run.canRecover(uid, now)) RecoveryForm(trigger, online, onRecover)
    }
}

@Composable
private fun RecoveryForm(trigger: String, online: Boolean, onRecover: (String) -> Unit) {
    var open by rememberSaveable(trigger) { mutableStateOf(false) }
    var text by rememberSaveable(trigger) { mutableStateOf("") }
    var confirmed by rememberSaveable(trigger) { mutableStateOf(false) }
    TextButton(onClick = { open = !open }) { Text(stringResource(R.string.teams_recover_summary)) }
    if (!open) return
    Text(stringResource(R.string.teams_recover_explain), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    OutlinedTextField(value = text, onValueChange = { text = it }, label = { Text(stringResource(R.string.teams_recover_result)) }, isError = TeamsComposeLogic.byteLength(text) > TeamsComposeLogic.MESSAGE_BYTES, modifier = Modifier.fillMaxWidth())
    Row(verticalAlignment = Alignment.CenterVertically) {
        Checkbox(checked = confirmed, onCheckedChange = { confirmed = it })
        Text(stringResource(R.string.teams_recover_confirm), style = MaterialTheme.typography.bodySmall)
    }
    Button(enabled = online && confirmed && TeamsComposeLogic.canSendText(text), onClick = { onRecover(text); open = false; text = ""; confirmed = false }) { Text(stringResource(R.string.teams_recover_submit)) }
}

/**
 * The question a Bot asked. The original requester (same membership generation) answers while the run is `running` with a live
 * lease; everyone else only reads. [pending] is the journaled answer, if one is waiting or was rejected.
 */
@Composable
internal fun TeamBotQuestionCard(
    message: TeamMessage, run: TeamBotRun?, uid: String, generation: Long, now: Long, offline: Boolean, pending: JSONObject?,
    onAnswer: (Map<String, String>) -> Unit, onDiscard: (String) -> Unit, modifier: Modifier = Modifier,
) {
    val question = message.question ?: return
    var values by remember(message.id, question.id) { mutableStateOf(question.questions.associateWith { "" }) }
    val recipient = message.authorKind == "bot" && !message.recalled && message.triggerId != null && message.originUid == uid && message.originGeneration == generation
    val answered = run?.answered == true && run.questionId == question.id
    val active = run?.canAnswer(message, uid, generation, now) == true && !offline
    Column(modifier.fillMaxWidth().padding(top = 4.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(stringResource(R.string.teams_question_title), style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold)
        when {
            pending != null -> {
                val failed = pending.has("error")
                Text(stringResource(if (failed) R.string.teams_question_rejected else R.string.teams_question_pending), style = MaterialTheme.typography.bodySmall, color = if (failed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
                val saved = pending.getJSONObject("answers")
                question.questions.forEach { prompt -> Text(prompt, style = MaterialTheme.typography.labelMedium); Text(saved.optString(prompt), style = MaterialTheme.typography.bodyMedium) }
                if (failed) TextButton(onClick = { onDiscard(pending.getString("answerId")) }) { Text(stringResource(R.string.teams_question_discard)) }
            }
            answered -> Text(stringResource(R.string.teams_question_received), style = MaterialTheme.typography.bodySmall)
            recipient -> {
                question.questions.forEach { prompt ->
                    OutlinedTextField(
                        value = values[prompt].orEmpty(), onValueChange = { if (it.length <= 2000) values = values + (prompt to it) }, enabled = active,
                        label = { Text(prompt) }, modifier = Modifier.fillMaxWidth(),
                    )
                }
                if (!active) Text(stringResource(if (run == null || offline) R.string.teams_question_verifying else R.string.teams_question_inactive), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Button(enabled = active && values.values.all { it.isNotBlank() }, onClick = { onAnswer(values.toMap()) }) { Text(stringResource(R.string.teams_question_send)) }
            }
            else -> {
                question.questions.forEach { prompt -> Text("• $prompt", style = MaterialTheme.typography.bodyMedium) }
                Text(stringResource(R.string.teams_question_requester_only), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

/** A Bot shared with the team (Team details, Bots segment): availability, audience and the Message / Unbind actions. */
@Composable
internal fun TeamBotCardView(bot: TeamBotCard, ownerName: String, uid: String, enabled: Boolean, onMessage: () -> Unit, onUnbind: () -> Unit, modifier: Modifier = Modifier) {
    var confirm by remember(bot.key, bot.revision, bot.enabled) { mutableStateOf(false) }
    OutlinedCard(modifier.fillMaxWidth()) {
        Row(Modifier.padding(14.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            BotAvatar(bot.avatarId, 40.dp)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(bot.name, style = MaterialTheme.typography.titleMedium)
                if (bot.description.isNotBlank()) Text(bot.description, style = MaterialTheme.typography.bodyMedium)
                Text("${botAvailabilityText(bot)} · ${stringResource(R.string.teams_bot_owner_fmt, ownerName)}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Text(
                    stringResource(if (bot.use == "all") R.string.teams_bot_use_all else R.string.teams_bot_use_listed) + if (!bot.permitted) " · " + stringResource(R.string.teams_bot_not_permitted) else "",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (!bot.enabled) Text(stringResource(R.string.teams_bot_not_shared), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Row {
                    TextButton(enabled = enabled && bot.selectable, onClick = onMessage) { Text(stringResource(R.string.teams_bot_message)) }
                    if (bot.canUnbind(uid)) TextButton(enabled = enabled, onClick = { confirm = true }) { Text(stringResource(R.string.teams_bot_unbind), color = MaterialTheme.colorScheme.error) }
                }
            }
        }
    }
    if (confirm && enabled && bot.canUnbind(uid)) AlertDialog(
        onDismissRequest = { confirm = false },
        title = { Text(stringResource(R.string.teams_bot_unbind_title_fmt, bot.name)) },
        text = { Text(stringResource(R.string.teams_bot_unbind_detail)) },
        confirmButton = { TextButton(onClick = { confirm = false; onUnbind() }) { Text(stringResource(R.string.teams_bot_unbind)) } },
        dismissButton = { TextButton(onClick = { confirm = false }) { Text(stringResource(R.string.common_cancel)) } },
    )
}
