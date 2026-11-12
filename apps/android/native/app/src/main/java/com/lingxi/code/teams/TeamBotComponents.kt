package com.lingxi.code.teams

import android.icu.text.CompactDecimalFormat
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.HelpOutline
import androidx.compose.material.icons.outlined.Block
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.HourglassEmpty
import androidx.compose.material.icons.outlined.Lock
import androidx.compose.material.icons.outlined.PauseCircle
import androidx.compose.material.icons.outlined.Schedule
import androidx.compose.material.icons.outlined.TimerOff
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Surface
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
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
import com.lingxi.code.conversation.AIText
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

/**
 * What one Bot request looks like in a conversation (U4): the state as a glyph and a word, the Bot's name, one line of detail (the usage once the run has
 * completed, what to do or what is going on before that) and at most one button. The text block is one accessibility element, "Bot, state, detail", with the
 * token split as its value; the button stays reachable beside it.
 */
@Composable
internal fun TeamTaskCard(task: TeamTask, botName: String, onArtifact: (String) -> Unit, onRefresh: (String) -> Unit = {}) {
    val run = TeamsBotText.runState(task.state)
    val words = rememberTeamUsageStrings()
    val completed = task.state == "completed"
    val state = stringResource(run.label)
    val detail = if (completed) TeamsBotText.usageLine(task.usage, words) else run.detail?.let { stringResource(it) }
    val split = if (completed) TeamsBotText.tokenSplit(task.usage, words) else null
    Column(Modifier.fillMaxWidth().padding(top = 4.dp).testTag("teams.task.${task.taskId}"), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Column(Modifier.semantics(mergeDescendants = true) { split?.let { stateDescription = it } }, verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                RunGlyphIcon(run)
                Text(state, style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold)
            }
            Text(botName, style = MaterialTheme.typography.titleSmall)
            detail?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.testTag("teams.task.usage.${task.taskId}")) }
        }
        // At most one button: Refresh status while the run is going, Open document once it has completed with one. The other states say what to do.
        val artifactId = task.artifactId
        when {
            run.active -> TextButton(onClick = { onRefresh(task.taskId) }) { Text(stringResource(R.string.teams_run_refresh)) }
            completed && artifactId != null -> TextButton(onClick = { onArtifact(artifactId) }) { Text(stringResource(R.string.teams_artifact_open)) }
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
    RunGlyph.Unknown -> Icons.AutoMirrored.Outlined.HelpOutline
}

@Composable
internal fun TeamArtifactViewer(artifact: TeamArtifact, onClose: () -> Unit) {
    androidx.compose.ui.window.Dialog(onDismissRequest = onClose, properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) {
            Column(Modifier.fillMaxSize().padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(artifact.title, style = MaterialTheme.typography.headlineSmall)
                androidx.compose.foundation.text.selection.SelectionContainer(Modifier.weight(1f).verticalScroll(rememberScrollState())) { AIText(markdown = artifact.body, modifier = Modifier.fillMaxWidth().testTag("teams.artifact.body"), onOpenLink = rememberTeamBotLinkOpener()) }
                Text(stringResource(R.string.apps_version_fmt, artifact.revision), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                TextButton(onClick = onClose) { Text(stringResource(R.string.common_close)) }
            }
        }
    }
}

@Composable
internal fun TeamBotCardView(bot: TeamBotCard, ownerName: String, uid: String, generation: Long, enabled: Boolean, onMessage: () -> Unit, onStopSharing: () -> Unit, modifier: Modifier = Modifier) {
    var confirm by remember(bot.sharedBotId, bot.revision, bot.state) { mutableStateOf(false) }
    Column(modifier.fillMaxWidth().padding(vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            BotAvatar(bot.avatarId, 56.dp)
            Column {
                Text(bot.name, style = MaterialTheme.typography.titleMedium)
                if (bot.description.isNotBlank()) Text(bot.description, style = MaterialTheme.typography.bodyMedium)
                Text("${botAvailabilityText(bot)} · ${stringResource(R.string.teams_bot_owner_fmt, ownerName)}", style = MaterialTheme.typography.bodySmall)
                Text(stringResource(if (bot.usePolicy.getString("mode") == "all") R.string.teams_bot_use_all else R.string.teams_bot_use_listed), style = MaterialTheme.typography.bodySmall)
                Row {
                    TextButton(enabled = enabled && bot.selectable, onClick = onMessage) { Text(stringResource(R.string.teams_bot_message)) }
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
