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
import android.icu.text.RelativeDateTimeFormatter
import android.icu.util.ULocale
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.conversation.AIText
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
    "waiting" -> R.string.teams_task_waiting
    "cancelled" -> R.string.teams_task_cancelled
    "needs_reconciliation" -> R.string.teams_task_needs_checking
    else -> R.string.chat_status_unknown
}

/** The words of the usage line in the UI language. */
@Composable
internal fun rememberTeamUsageStrings(): TeamUsageStrings {
    val context = LocalContext.current
    val missing = stringResource(R.string.teams_usage_missing)
    val partial = stringResource(R.string.teams_usage_partial)
    val complete = stringResource(R.string.teams_usage_complete)
    val costUnknown = stringResource(R.string.teams_usage_cost_unknown)
    val currencyUnknown = stringResource(R.string.teams_usage_currency_unknown)
    return remember(context, missing, partial, complete, costUnknown, currencyUnknown) {
        TeamUsageStrings(missing, partial, complete, costUnknown, currencyUnknown) { input, output -> context.getString(R.string.teams_usage_tokens_fmt, input, output) }
    }
}

/** Opens a link from a Bot's text: web pages only, everything else is ignored. */
@Composable
internal fun rememberTeamBotLinkOpener(): (String) -> Unit {
    val handler = LocalUriHandler.current
    return remember(handler) { { link: String -> if (TeamsBotText.allowsLink(link)) runCatching { handler.openUri(link.trim()) } } }
}

/** "in 4 minutes" in the UI language, for a wait's expiry. */
@Composable
internal fun teamWaitExpiryText(expiresAt: Long): String {
    val locale = LocalConfiguration.current.locales[0]
    val (value, unit) = TeamTaskActions.expiry(expiresAt, System.currentTimeMillis())
    return remember(locale, value, unit) {
        RelativeDateTimeFormatter.getInstance(ULocale.forLocale(locale)).format(value.toDouble(), RelativeDateTimeFormatter.Direction.NEXT, when (unit) {
            TeamTaskActions.ExpiryUnit.MINUTES -> RelativeDateTimeFormatter.RelativeUnit.MINUTES
            TeamTaskActions.ExpiryUnit.HOURS -> RelativeDateTimeFormatter.RelativeUnit.HOURS
            TeamTaskActions.ExpiryUnit.DAYS -> RelativeDateTimeFormatter.RelativeUnit.DAYS
        })
    }
}

/**
 * [viewer] is the signed-in member and [maintainer] the Bot's maintainer when its card is known: they decide who may cancel.
 * [recipientName] names the member a waiting task waits for (not needed when that is the viewer). [onReply] starts the answer
 * to the Bot's question: set only on the question itself, while the viewer can still answer it.
 */
@Composable
internal fun TeamTaskCard(
    task: TeamTask, botName: String, onArtifact: (String) -> Unit, onRefresh: (String) -> Unit, viewer: TeamMemberRef?, maintainer: TeamMemberRef?,
    recipientName: String, onCancel: (String) -> Unit, onReply: (() -> Unit)? = null,
) {
    var cancelAsked by remember(task.taskId, task.state, task.cancelRequestedAt) { mutableStateOf(false) }
    val canCancel = TeamTaskActions.canCancel(task, viewer, maintainer)
    Column(Modifier.fillMaxWidth().padding(12.dp).testTag("teams.task.${task.taskId}"), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(botName, style = MaterialTheme.typography.titleSmall)
        TextButton(onClick = { onRefresh(task.taskId) }) { Text(stringResource(if (task.cancelling) R.string.teams_task_cancelling else teamTaskStateRes(task.state)), style = MaterialTheme.typography.bodySmall) }
        task.wait?.takeIf { task.state == "waiting" }?.let { wait ->
            val expires = teamWaitExpiryText(wait.expiresAt)
            Text(
                if (wait.recipient == viewer) stringResource(R.string.teams_task_waiting_you_fmt, expires) else stringResource(R.string.teams_task_waiting_member_fmt, recipientName, expires),
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.testTag("teams.task.wait.${task.taskId}"),
            )
        }
        // An outside action of unknown outcome: the maintainer confirms it where the review screen is.
        if (task.state == "needs_reconciliation") Text(stringResource(R.string.teams_task_needs_checking_hint), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.testTag("teams.task.checking.${task.taskId}"))
        // A task cancelled before any model call has nothing to report.
        if (!(task.state == "cancelled" && task.usage == null)) Text(TeamsBotText.usageLine(task.usage, rememberTeamUsageStrings()), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.testTag("teams.task.usage.${task.taskId}"))
        if (onReply != null || canCancel) Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (onReply != null) Button(onClick = onReply, modifier = Modifier.testTag("teams.task.reply.${task.taskId}")) { Text(stringResource(R.string.teams_message_reply)) }
            if (canCancel) OutlinedButton(onClick = { cancelAsked = true; onCancel(task.taskId) }, enabled = !cancelAsked, modifier = Modifier.testTag("teams.task.cancel.${task.taskId}")) {
                Text(stringResource(R.string.teams_task_cancel), color = MaterialTheme.colorScheme.error)
            }
        }
        task.artifactId?.let { id -> TextButton(onClick = { onArtifact(id) }) { Text(stringResource(R.string.teams_artifact_open)) } }
    }
}

@Composable
internal fun TeamArtifactViewer(artifact: TeamArtifact, onClose: () -> Unit) {
    androidx.compose.ui.window.Dialog(onDismissRequest = onClose, properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) {
            Column(Modifier.fillMaxSize().padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(artifact.title, style = MaterialTheme.typography.headlineSmall)
                androidx.compose.foundation.text.selection.SelectionContainer(Modifier.weight(1f).verticalScroll(rememberScrollState())) { AIText(markdown = artifact.body, modifier = Modifier.fillMaxWidth().testTag("teams.artifact.body"), onOpenLink = rememberTeamBotLinkOpener()) }
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
