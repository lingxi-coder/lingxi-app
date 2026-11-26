package com.lingxi.code.teams

import android.icu.text.CompactDecimalFormat
import android.icu.text.RelativeDateTimeFormatter
import android.icu.util.ULocale
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.FactCheck
import androidx.compose.material.icons.automirrored.outlined.HelpOutline
import androidx.compose.material.icons.outlined.Block
import androidx.compose.material.icons.outlined.Cancel
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.HourglassEmpty
import androidx.compose.material.icons.outlined.Lock
import androidx.compose.material.icons.outlined.PauseCircle
import androidx.compose.material.icons.outlined.Pending
import androidx.compose.material.icons.outlined.Schedule
import androidx.compose.material.icons.outlined.TimerOff
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.theme.BotAvatar
import com.lingxi.code.theme.ConfirmDialog
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.theme.rememberReduceMotion

/**
 * The words and number formats of the usage line in the UI language (U5): tokens in the compact form of the locale (`1.2K`), the cost in the task's
 * currency with two to four decimals (`$0.0012`).
 */
@Composable
internal fun rememberTeamUsageStrings(): TeamUsageStrings {
    val context = LocalContext.current
    val locale = LocalConfiguration.current.locales[0]
    val missing = stringResource(R.string.teams_usage_missing)
    val partial = stringResource(R.string.teams_usage_partial)
    val costUnknown = stringResource(R.string.teams_usage_cost_unknown)
    val currencyUnknown = stringResource(R.string.teams_usage_currency_unknown)
    return remember(context, locale, missing, partial, costUnknown, currencyUnknown) {
        val compact = CompactDecimalFormat.getInstance(locale, CompactDecimalFormat.CompactStyle.SHORT)
        TeamUsageStrings(
            missing, partial, costUnknown, currencyUnknown,
            tokensTotal = { total -> context.getString(R.string.teams_usage_tokens_total_fmt, compact.format(total)) },
            tokensSplit = { input, output -> context.getString(R.string.teams_usage_tokens_fmt, input, output) },
            money = { amount, code -> TeamsBotText.money(amount, code, locale) },
        )
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
internal fun teamWaitExpiryText(expiresAt: Long): String = teamExpiryText(expiresAt, System.currentTimeMillis())

/** "in 4 minutes" in the UI language: how long until [expiresAt] as of [now], both in epoch milliseconds. */
@Composable
internal fun teamExpiryText(expiresAt: Long, now: Long): String {
    val locale = LocalConfiguration.current.locales[0]
    val (value, unit) = TeamTaskActions.expiry(expiresAt, now)
    return remember(locale, value, unit) {
        RelativeDateTimeFormatter.getInstance(ULocale.forLocale(locale)).format(value.toDouble(), RelativeDateTimeFormatter.Direction.NEXT, when (unit) {
            TeamTaskActions.ExpiryUnit.MINUTES -> RelativeDateTimeFormatter.RelativeUnit.MINUTES
            TeamTaskActions.ExpiryUnit.HOURS -> RelativeDateTimeFormatter.RelativeUnit.HOURS
            TeamTaskActions.ExpiryUnit.DAYS -> RelativeDateTimeFormatter.RelativeUnit.DAYS
        })
    }
}

/** The one line under the Bot's name on a run card, and the test tag it is found by. */
private class RunDetail(val text: String, val tag: String)

/**
 * Why the Bot stayed silent, for its maintainer: the reason the Bot's host gave and, when the Decision service judged, how likely it thought a reply was wanted.
 * Null for anyone else (the log is read for the maintainer only, and this is the second check) and for a task with no row.
 */
@Composable
private fun declinedReasonLine(task: TeamTask, viewer: TeamMemberRef?, maintainer: TeamMemberRef?, reason: TeamTriageItem?): String? {
    if (task.state != "declined" || viewer == null || maintainer != viewer) return null
    val (words, percent) = TeamTriageRules.reasonParts(reason)
    val parts = listOfNotNull(words?.let { stringResource(it) }, percent?.let { stringResource(R.string.teams_triage_probability_fmt, it) })
    return parts.joinToString(" \u00b7 ").ifEmpty { null }
}

/**
 * What one Bot request looks like in a conversation (U4): the state as a glyph and a word, the Bot's name, one line of detail and the actions. The line is the
 * usage once the run is over (a run cancelled before any model call has nothing to report), who a waiting task waits for, what a task that needs checking needs,
 * and for every other state what to do or what is going on. The text block is one accessibility element, "state, Bot, detail", with the token split as its
 * value; the buttons stay reachable beside it.
 *
 * Actions: Refresh status while the run is going and Open document once it has completed with one, as before; and the member's own: [onReply] answers the
 * Bot's question (set only on the question itself, while the viewer can still answer it), [onCancel] stops a task that has not ended, [onReview] opens the
 * review of an action of unknown outcome. [viewer] is the signed-in member and [maintainer] the Bot's maintainer when its card is known: they decide who may
 * cancel and review. [recipientName] names the member a waiting task waits for (not needed when that is the viewer). [reason] is the Bot's auto-reply log row of
 * this task, read for the Bot's maintainer only: under "Did not reply" it says why the Bot stayed silent. Nobody else is ever shown it.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun TeamTaskCard(
    task: TeamTask, botName: String, onArtifact: (String) -> Unit, onRefresh: (String) -> Unit = {}, viewer: TeamMemberRef? = null, maintainer: TeamMemberRef? = null,
    recipientName: String = "", onCancel: (String) -> Unit = {}, onReply: (() -> Unit)? = null, onReview: ((String) -> Unit)? = null, modifier: Modifier = Modifier,
    reason: TeamTriageItem? = null,
) {
    var cancelAsked by remember(task.taskId, task.state, task.cancelRequestedAt) { mutableStateOf(false) }
    val run = TeamsBotText.runState(task.state)
    val words = rememberTeamUsageStrings()
    val completed = task.state == "completed"
    val canCancel = TeamTaskActions.canCancel(task, viewer, maintainer)
    val resolves = TeamOperationRules.canResolve(viewer, maintainer)
    val review = onReview?.takeIf { TeamOperationRules.canReview(task, viewer, maintainer) }
    val state = stringResource(TeamsBotText.stateLabel(task))
    val usageShown = completed || (task.state == "cancelled" && task.usage != null)
    val wait = task.wait?.takeIf { task.state == "waiting" }
    val detail: RunDetail? = when {
        usageShown -> RunDetail(TeamsBotText.usageLine(task.usage, words), "teams.task.usage.${task.taskId}")
        wait != null -> {
            val expires = teamWaitExpiryText(wait.expiresAt)
            RunDetail(
                if (wait.recipient == viewer) stringResource(R.string.teams_task_waiting_you_fmt, expires) else stringResource(R.string.teams_task_waiting_member_fmt, recipientName, expires),
                "teams.task.wait.${task.taskId}",
            )
        }
        // An outside action of unknown outcome: the maintainer confirms it where the review screen is.
        task.state == "needs_reconciliation" ->
            RunDetail(stringResource(if (resolves) R.string.teams_task_needs_checking_maintainer else R.string.teams_task_needs_checking_hint), "teams.task.checking.${task.taskId}")
        // A Bot that chose not to answer: why, when this person maintains the Bot and its log has a row for the task; otherwise nothing more than "Did not reply".
        task.state == "declined" -> declinedReasonLine(task, viewer, maintainer, reason)?.let { RunDetail(it, "teams.task.reason.${task.taskId}") }
        // Sent while the Bot's Desktop was off (rule 6): it answers when the Desktop is back, within a day.
        task.waitsForHost -> RunDetail(stringResource(R.string.teams_bot_offline_wait_fmt, botName), "teams.task.hostwait.${task.taskId}")
        else -> run.detail?.let { RunDetail(stringResource(it), "teams.task.usage.${task.taskId}") }
    }
    val split = if (usageShown) TeamsBotText.tokenSplit(task.usage, words) else null
    val artifactId = task.artifactId?.takeIf { completed }
    Column(modifier.fillMaxWidth().padding(top = 4.dp).testTag("teams.task.${task.taskId}"), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Column(Modifier.semantics(mergeDescendants = true) { split?.let { stateDescription = it } }, verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                RunGlyphIcon(run)
                Text(state, style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold)
            }
            Text(botName, style = MaterialTheme.typography.titleSmall)
            detail?.let { Text(it.text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.testTag(it.tag)) }
        }
        if (onReply != null || review != null || canCancel || run.active || artifactId != null) FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (onReply != null) Button(onClick = onReply, modifier = Modifier.testTag("teams.task.reply.${task.taskId}")) { Text(stringResource(R.string.teams_message_reply)) }
            if (review != null) OutlinedButton(onClick = { review(task.taskId) }, modifier = Modifier.testTag("teams.task.review.${task.taskId}")) {
                Text(stringResource(if (resolves) R.string.teams_ops_review else R.string.teams_ops_view))
            }
            if (canCancel) OutlinedButton(onClick = { cancelAsked = true; onCancel(task.taskId) }, enabled = !cancelAsked, modifier = Modifier.testTag("teams.task.cancel.${task.taskId}")) {
                Text(stringResource(R.string.teams_task_cancel), color = MaterialTheme.colorScheme.error)
            }
            // The other states say what to do; Refresh status is for the ones that are still going.
            if (run.active) TextButton(onClick = { onRefresh(task.taskId) }) { Text(stringResource(R.string.teams_run_refresh)) }
            if (artifactId != null) TextButton(onClick = { onArtifact(artifactId) }) { Text(stringResource(R.string.teams_artifact_open)) }
        }
    }
}

/**
 * The state's glyph, drawn at the size of a line of label text so it follows the font scale. It is decorative: the label next to it is the state, so the
 * state is never colour alone. Processing is a spinner, and the static hourglass under Reduce motion (U10).
 */
@Composable
private fun RunGlyphIcon(run: TeamRunState) {
    val reduceMotion = rememberReduceMotion()
    val side = with(LocalDensity.current) { 16.sp.toDp() }
    val tint = when (run.tone) {
        RunTone.Ok -> LingXiTheme.palette.ok
        RunTone.Danger -> MaterialTheme.colorScheme.error
        RunTone.Muted -> MaterialTheme.colorScheme.onSurfaceVariant
    }
    if (run.glyph == RunGlyph.Spinner && !reduceMotion) {
        CircularProgressIndicator(Modifier.size(side).clearAndSetSemantics { }, color = tint, strokeWidth = 2.dp)
    } else {
        Icon(runGlyphVector(run.glyph, reduceMotion), contentDescription = null, modifier = Modifier.size(side), tint = tint)
    }
}

private fun runGlyphVector(glyph: RunGlyph, reduceMotion: Boolean): ImageVector = when (glyph) {
    RunGlyph.Clock -> Icons.Outlined.Schedule
    RunGlyph.Spinner -> if (reduceMotion) Icons.Outlined.HourglassEmpty else Icons.Outlined.Schedule
    RunGlyph.Check -> Icons.Outlined.CheckCircle
    RunGlyph.Alert -> Icons.Outlined.ErrorOutline
    RunGlyph.Expired -> Icons.Outlined.TimerOff
    RunGlyph.Paused -> Icons.Outlined.PauseCircle
    RunGlyph.Locked -> Icons.Outlined.Lock
    RunGlyph.Blocked -> Icons.Outlined.Block
    RunGlyph.Waiting -> Icons.Outlined.Pending
    RunGlyph.Cancelled -> Icons.Outlined.Cancel
    RunGlyph.Checking -> Icons.AutoMirrored.Outlined.FactCheck
    RunGlyph.Unknown -> Icons.AutoMirrored.Outlined.HelpOutline
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
 * decides or decided. Nothing is saved to the list before an approval. The request reads as one element for a screen reader; the buttons stay their own,
 * and wrap under each other when the font scale leaves no room for them side by side.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun TeamApprovalCard(approval: TeamListApproval, waitId: String, state: TeamApprovalState, deciderName: String, onDecide: (Boolean, TeamDecisionIds) -> Unit) {
    // One identity per decision, kept across retries so a lost answer cannot post the reply twice.
    val ids = remember(waitId) { mutableMapOf<Boolean, TeamDecisionIds>() }
    var asked by remember(waitId, state) { mutableStateOf(false) }
    // A decision that did not get through can be sent again after a moment.
    androidx.compose.runtime.LaunchedEffect(asked) { if (asked) { kotlinx.coroutines.delay(8000); asked = false } }
    // It sits flush with the message text, like the run card under it.
    Column(Modifier.fillMaxWidth().padding(top = 4.dp).testTag("teams.approval.$waitId"), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Column(Modifier.semantics(mergeDescendants = true) {}, verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(stringResource(R.string.teams_approval_title_fmt, approval.title), style = MaterialTheme.typography.titleSmall)
            approval.changes.forEach { change -> Text("• ${teamApprovalChangeText(change)}", style = MaterialTheme.typography.bodyMedium) }
            if (state == TeamApprovalState.Decide) Text(stringResource(R.string.teams_approval_hint), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        if (state == TeamApprovalState.Decide) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
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

/**
 * [onStopSharing] is the maintainer's Stop sharing (confirmed first); [onList], when set, opens the Bot's shared list; [onTriageLog], when set, opens the Bot's
 * auto-reply log (set only for its maintainer, and only for a Bot that answers on its own); [onExample], when set, starts a chat with one of the Bot's example
 * requests (see [TeamBotMemberInfo]). [heldModel], when set (only for the Bot's maintainer), reads the replies of the Bot that wait for them to let them go and offers "Replies
 * waiting" while there are some; [heldSignals] counts the service's Bot change signals, and [onHeldChat] takes the person to the Chat a reply was made in.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun TeamBotCardView(
    bot: TeamBotCard, ownerName: String, uid: String, generation: Long, enabled: Boolean, onMessage: () -> Unit, onStopSharing: () -> Unit,
    modifier: Modifier = Modifier, onList: (() -> Unit)? = null, onExample: ((String) -> Unit)? = null, onTriageLog: (() -> Unit)? = null,
    heldModel: TeamMessagingModel? = null, heldSignals: Long = 0, onHeldChat: ((String) -> Unit)? = null,
) {
    var confirm by remember(bot.sharedBotId, bot.revision, bot.state) { mutableStateOf(false) }
    Column(modifier.fillMaxWidth().padding(vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            BotAvatar(bot.avatarId, 56.dp)
            Column {
                Text(bot.name, style = MaterialTheme.typography.titleMedium)
                if (bot.description.isNotBlank()) Text(bot.description, style = MaterialTheme.typography.bodyMedium)
                Text("${botAvailabilityText(bot)} · ${stringResource(R.string.teams_bot_owner_fmt, ownerName)}", style = MaterialTheme.typography.bodySmall)
                Text(stringResource(if (bot.usePolicy.getString("mode") == "all") R.string.teams_bot_use_all else R.string.teams_bot_use_listed), style = MaterialTheme.typography.bodySmall)
                TeamBotMemberInfo(bot, enabled, onExample)
                // Up to five actions: they wrap under each other rather than run out of the card at a large font size.
                FlowRow {
                    TextButton(enabled = enabled && bot.selectable, onClick = onMessage) { Text(stringResource(R.string.teams_bot_message)) }
                    if (onList != null) TextButton(enabled = enabled, onClick = onList, modifier = Modifier.testTag("teams.bot.list.${bot.sharedBotId}")) { Text(stringResource(R.string.teams_list_open)) }
                    if (onTriageLog != null) TextButton(enabled = enabled, onClick = onTriageLog, modifier = Modifier.testTag("teams.bot.triage.${bot.sharedBotId}")) { Text(stringResource(R.string.teams_triage_title)) }
                    if (heldModel != null && onHeldChat != null) TeamHeldEntry(bot, heldModel, heldSignals, enabled, onHeldChat)
                    if (bot.canPause(uid, generation)) TextButton(enabled = enabled, onClick = { confirm = true }) { Text(stringResource(R.string.friends_share_bot_stop)) }
                }
            }
        }
    }
    if (confirm) StopSharingDialog(bot.name, onConfirm = { confirm = false; onStopSharing() }, onDismiss = { confirm = false })
}

/** The one Stop sharing dialog of Teams and Friends (U8): the Bot's name in the title, what stops and what stays, a destructive "Stop sharing", and Cancel. */
@Composable
internal fun StopSharingDialog(botName: String, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    ConfirmDialog(
        title = stringResource(R.string.teams_bot_stop_title_fmt, botName), body = stringResource(R.string.teams_bot_stop_detail),
        confirm = stringResource(R.string.friends_share_bot_stop), destructive = true, onConfirm = onConfirm, onDismiss = onDismiss,
    )
}

/**
 * Items on one line, "Wait and resume: Supported · Write operations: Not supported". The colon and the middle dot are punctuation, the same in every
 * language and on every client, so they stay in code; only the words are resources (U12).
 */
@Composable
internal fun teamBotLine(items: List<TeamBotItem>): String = items.map { item ->
    stringResource(item.label) + ": " + when (val value = item.value) {
        is TeamBotValue.Words -> stringResource(value.id)
        is TeamBotValue.Figure -> value.text
    }
}.joinToString(" · ")

/** Member-safe projection only: never render host reasons, credentials or configured quota limits. With a [conversation], what the Bot may use is what that Chat leaves of what its maintainer shared. */
@Composable
internal fun TeamBotMemberInfo(bot: TeamBotCard, enabled: Boolean, onExample: ((String) -> Unit)? = null, modifier: Modifier = Modifier, conversation: TeamMessageConversation? = null) {
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(teamBotLine(TeamBotEntry.capabilities(bot)), style = MaterialTheme.typography.bodySmall)
        Text(teamBotLine(listOf(TeamBotEntry.remaining(bot), TeamBotEntry.resets(bot))), style = MaterialTheme.typography.bodySmall)
        // With a Chat, what that Chat leaves of what the Bot's maintainer shared; without one, all of it.
        TeamBotScopeSummary(bot, conversation)
        if (onExample != null && bot.examples.isNotEmpty()) {
            // What a tap on an example does, in the words Desktop and Web use: it fills the draft and never sends.
            Text(stringResource(R.string.teams_bot_examples_hint), style = MaterialTheme.typography.bodySmall)
            bot.examples.forEachIndexed { index, example ->
                TextButton(enabled = enabled && bot.selectable, onClick = { onExample(example) }, modifier = Modifier.testTag("teams.bot.example.${bot.sharedBotId}.$index")) {
                    Text(example, maxLines = 2, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
                }
            }
        }
    }
}
