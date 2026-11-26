package com.lingxi.code.teams

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.PauseCircle
import androidx.compose.material.icons.outlined.PlayCircle
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.lingxi.code.R
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.launch
import java.text.DateFormat
import java.util.Date

/**
 * What a Bot decided about the messages nobody asked it about, for the person who maintains it (Desktop's Bots tab, `packages/teams/TriageLogDialog.tsx`, iOS
 * `TeamTriageLogView`): newest first, a page at a time, in every Chat of the Team or, with [conversation], in one. Only the decision and the reason are kept, never
 * what a model said. When the service says the person does not maintain the Bot after all, [onDenied] takes its entry away and the screen closes.
 */
@Composable
internal fun TeamTriageLogDialog(bot: TeamBotCard, conversation: TeamTriageChat?, model: TeamMessagingModel, onDenied: (String) -> Unit, onClose: () -> Unit) {
    val scope = rememberCoroutineScope()
    // The list says whose it is: a change of Bot or Chat starts a new one, and nothing read for the old one is shown for the new one.
    val key = bot.sharedBotId + "|" + (conversation?.id ?: "")
    var list by remember(key) { mutableStateOf(TeamTriageList()) }
    val reader = remember(key, model) {
        TeamTriageReader(read = { cursor, limit -> model.listTriage(bot.sharedBotId, conversation?.id, cursor, limit) }, changed = { list = it })
    }
    LaunchedEffect(reader) { reader.load() }
    // Leaving drops what is still being read.
    DisposableEffect(reader) { onDispose { reader.close() } }
    LaunchedEffect(list.status) {
        if (list.status == TeamTriageList.Status.Denied) { onDenied(bot.sharedBotId); onClose() }
    }

    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize().testTag("teams.triage.${bot.sharedBotId}"), color = MaterialTheme.colorScheme.surface) {
            Column(Modifier.fillMaxSize().systemBarsPadding()) {
                Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = onClose, modifier = Modifier.testTag("teams.triage.close")) { Icon(Icons.Outlined.Close, stringResource(R.string.common_close)) }
                    Text(stringResource(R.string.teams_triage_title), Modifier.weight(1f).semantics { heading() }, style = MaterialTheme.typography.titleLarge, maxLines = 1)
                    IconButton(enabled = !list.busy, onClick = { scope.launch { reader.load() } }, modifier = Modifier.testTag("teams.triage.refresh")) {
                        Icon(Icons.Outlined.Refresh, stringResource(R.string.teams_list_refresh))
                    }
                }
                if (list.busy) LinearProgressIndicator(Modifier.fillMaxWidth()) else Box(Modifier.padding(top = 4.dp))
                LazyColumn(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    item("intro") { Text(stringResource(R.string.teams_triage_intro), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                    item("subject") {
                        Column(Modifier.padding(vertical = 4.dp).semantics(mergeDescendants = true) {}) {
                            Text(bot.name, style = MaterialTheme.typography.titleSmall)
                            if (conversation != null) Text(conversation.title, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                    if (list.status == TeamTriageList.Status.Loading) item("loading") {
                        Text(stringResource(R.string.teams_triage_loading), Modifier.testTag("teams.triage.loading"), color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    if (list.status == TeamTriageList.Status.Ready && list.items.isEmpty()) item("empty") {
                        Text(stringResource(R.string.teams_triage_empty), Modifier.testTag("teams.triage.empty"), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    items(list.items, key = { it.taskId }) { item -> TeamTriageRow(item) }
                    if (list.status == TeamTriageList.Status.Failed && list.error.isNotEmpty()) item("error") {
                        Column {
                            Text(stringResource(teamsErrorRes(list.error)), Modifier.testTag("teams.triage.error").semantics { liveRegion = LiveRegionMode.Polite }, color = MaterialTheme.colorScheme.error)
                            // With rows on the screen the failed read is the next page, and Load more below tries it again; with none, the first page is tried again here.
                            if (list.items.isEmpty()) TextButton(onClick = { scope.launch { reader.load() } }, modifier = Modifier.testTag("teams.triage.retry")) { Text(stringResource(R.string.common_retry)) }
                        }
                    }
                    if (list.nextCursor != null && list.status != TeamTriageList.Status.Denied) item("more") {
                        TextButton(onClick = { scope.launch { reader.more() } }, enabled = !list.busy, modifier = Modifier.fillMaxWidth().testTag("teams.triage.more")) {
                            Text(stringResource(if (list.status == TeamTriageList.Status.More) R.string.teams_triage_loading else R.string.teams_triage_more))
                        }
                    }
                }
            }
        }
    }
}

/**
 * One judged message: the decision as a glyph and a word, how likely the Decision service thought a reply was wanted (only when it judged), the reason, the Chat and
 * the time, and where the message ended up. The row reads as one element for a screen reader.
 */
@Composable
private fun TeamTriageRow(item: TeamTriageItem) {
    val outcome = TeamTriageRules.outcomes[item.state] ?: TeamTriageOutcome.Waiting
    val chat = item.conversationTitle ?: stringResource(R.string.teams_triage_chat_untitled)
    val time = remember(item.triage.at) { DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(item.triage.at)) }
    val palette = LingXiTheme.palette
    val quiet = MaterialTheme.colorScheme.onSurfaceVariant
    Row(
        Modifier.fillMaxWidth().padding(vertical = 8.dp).semantics(mergeDescendants = true) {}.testTag("teams.triage.row.${item.taskId}"),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(triageGlyph(item.triage.action), contentDescription = null, modifier = Modifier.size(22.dp), tint = if (item.triage.action == "run") palette.ok else quiet)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                TeamTriageRules.actionLabel(item.triage.action)?.let { Text(stringResource(it), style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold) }
                item.triage.probability?.let {
                    Text(
                        stringResource(R.string.teams_triage_probability_fmt, TeamTriageRules.percent(it)), style = MaterialTheme.typography.labelSmall, color = quiet,
                        modifier = Modifier.clip(RoundedCornerShape(50)).background(MaterialTheme.colorScheme.surfaceContainerHighest).padding(horizontal = 6.dp, vertical = 1.dp),
                    )
                }
            }
            TeamTriageRules.reasonLabel(item.triage.reason)?.let { Text(stringResource(it), style = MaterialTheme.typography.bodySmall, color = quiet) }
            Text(listOfNotNull(chat, time, TeamTriageRules.approvalLabel(item.approval)?.let { stringResource(it) }, TeamTriageRules.roundLabel(item.round)?.let { stringResource(it) }).joinToString(" · "), style = MaterialTheme.typography.labelSmall, color = quiet)
        }
        Text(
            stringResource(TeamTriageRules.outcomeLabel(outcome)), Modifier.padding(start = 8.dp), style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.End,
            color = when (outcome) { TeamTriageOutcome.Failed -> MaterialTheme.colorScheme.error; TeamTriageOutcome.Replied -> palette.ok; else -> quiet },
        )
    }
}

/** The glyph of a decision: answer, leave it to the Bot, or stay silent. Decorative: the word next to it is the decision. */
private fun triageGlyph(action: String): ImageVector = when (action) {
    "run" -> Icons.Outlined.PlayCircle
    "run_may_decline" -> Icons.Outlined.ErrorOutline
    else -> Icons.Outlined.PauseCircle
}
