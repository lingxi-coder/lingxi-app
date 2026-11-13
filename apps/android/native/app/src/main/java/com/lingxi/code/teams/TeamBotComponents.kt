package com.lingxi.code.teams

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
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
        TextButton(onClick = { onRefresh(task.taskId) }) { Text(stringResource(if (task.cancelling) R.string.teams_task_cancelling else if (task.state == "waiting" && task.wait?.kind == "tool_approval") R.string.teams_task_waiting_approval else teamTaskStateRes(task.state)), style = MaterialTheme.typography.bodySmall) }
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

/** One proposed change in words. An item that was gone when the Bot asked has no text to show. */
@Composable
internal fun teamApprovalChangeText(change: TeamListApprovalChange): String {
    val item = change.item?.let { "“$it”" } ?: stringResource(R.string.teams_approval_missing_item)
    val text = "“${change.text.orEmpty()}”"
    return when {
        change.op == "add" -> stringResource(R.string.teams_approval_add_fmt, text)
        change.op == "remove" -> stringResource(R.string.teams_approval_remove_fmt, item)
        change.text != null && change.done == true -> stringResource(R.string.teams_approval_change_done_fmt, item, text)
        change.text != null && change.done == false -> stringResource(R.string.teams_approval_change_undone_fmt, item, text)
        change.text != null -> stringResource(R.string.teams_approval_change_fmt, item, text)
        change.done == false -> stringResource(R.string.teams_approval_undone_fmt, item)
        else -> stringResource(R.string.teams_approval_done_fmt, item)
    }
}

/**
 * A Bot's request to change its list, under the message that carries it: each change in words, and Approve and Decline for the
 * member the Bot works for while the task waits. Afterwards it says how the request ended. [deciderName] names the member who
 * decides or decided. Nothing is saved to the list before an approval.
 */
@Composable
internal fun TeamApprovalCard(approval: TeamListApproval, waitId: String, state: TeamApprovalState, deciderName: String, onDecide: (Boolean, TeamDecisionIds) -> Unit) {
    // One identity per decision, kept across retries so a lost answer cannot post the reply twice.
    val ids = remember(waitId) { mutableMapOf<Boolean, TeamDecisionIds>() }
    var asked by remember(waitId, state) { mutableStateOf(false) }
    // A decision that did not get through can be sent again after a moment.
    androidx.compose.runtime.LaunchedEffect(asked) { if (asked) { kotlinx.coroutines.delay(8000); asked = false } }
    Column(Modifier.fillMaxWidth().padding(12.dp).testTag("teams.approval.$waitId"), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(stringResource(R.string.teams_approval_title_fmt, approval.title), style = MaterialTheme.typography.titleSmall)
        approval.changes.forEach { change -> Text("• ${teamApprovalChangeText(change)}", style = MaterialTheme.typography.bodyMedium) }
        if (state == TeamApprovalState.Decide) {
            Text(stringResource(R.string.teams_approval_hint), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { asked = true; onDecide(true, ids.getOrPut(true) { TeamDecisionIds() }) }, enabled = !asked, modifier = Modifier.testTag("teams.approval.approve.$waitId")) { Text(stringResource(R.string.teams_approval_approve)) }
                OutlinedButton(onClick = { asked = true; onDecide(false, ids.getOrPut(false) { TeamDecisionIds() }) }, enabled = !asked, modifier = Modifier.testTag("teams.approval.decline.$waitId")) {
                    Text(stringResource(R.string.teams_approval_decline), color = MaterialTheme.colorScheme.error)
                }
            }
        } else Text(
            when (state) {
                is TeamApprovalState.Waiting -> stringResource(R.string.teams_approval_waiting_member_fmt, deciderName)
                is TeamApprovalState.Approved -> stringResource(R.string.teams_approval_approved_by_fmt, deciderName)
                is TeamApprovalState.Declined -> stringResource(R.string.teams_approval_declined_by_fmt, deciderName)
                else -> stringResource(R.string.teams_approval_closed)
            },
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.testTag("teams.approval.status.$waitId"),
        )
    }
}

@Composable
internal fun TeamBotCardView(bot: TeamBotCard, ownerName: String, uid: String, generation: Long, enabled: Boolean, onMessage: () -> Unit, onPause: () -> Unit, modifier: Modifier = Modifier, onList: (() -> Unit)? = null) {
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
                    if (onList != null) TextButton(enabled = enabled, onClick = onList, modifier = Modifier.testTag("teams.bot.list.${bot.sharedBotId}")) { Text(stringResource(R.string.teams_list_open)) }
                    if (bot.canPause(uid, generation)) TextButton(enabled = enabled, onClick = { confirm = true }) { Text(stringResource(R.string.teams_bot_pause)) }
                }
            }
        }
    }
    if (confirm) AlertDialog(onDismissRequest = { confirm = false }, title = { Text(bot.name) }, text = { Text(stringResource(R.string.teams_bot_pause)) },
        confirmButton = { TextButton(onClick = { confirm = false; onPause() }) { Text(stringResource(R.string.teams_bot_pause)) } },
        dismissButton = { TextButton(onClick = { confirm = false }) { Text(stringResource(R.string.common_cancel)) } })
}
