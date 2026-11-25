package com.lingxi.code.teams

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/** The small tag next to a name ("Can edit", "Limited in this chat"). */
@Composable
internal fun TeamBotScopeTag(text: String, modifier: Modifier = Modifier) {
    Surface(modifier, shape = MaterialTheme.shapes.extraSmall, color = MaterialTheme.colorScheme.surfaceContainerHighest) {
        Text(text, Modifier.padding(horizontal = 6.dp, vertical = 1.dp), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/**
 * What a Bot may use of its maintainer's computer (`packages/teams/BotScopeSummary.tsx`): the names of its Local Apps and its datasets, and a dataset it may
 * edit says so. Nothing else of them is known here. [limited] says a Chat uses less of it than the maintainer shared. A Bot that uses none of it says that instead.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun TeamBotScopeSummary(scope: TeamBotScope, limited: Boolean, modifier: Modifier = Modifier, tag: String = "teams.bot.scope") {
    val separator = if (TeamsListStrings.isChinese(LocalConfiguration.current.locales[0].toLanguageTag())) TeamsListStrings.ZH_LIST_SEPARATOR else ", "
    Column(modifier.fillMaxWidth().testTag(tag), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        // The tag stays on the line of the sentence when it fits and goes under it when it does not.
        FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(stringResource(if (scope.isEmpty) R.string.teams_bot_scope_none else R.string.teams_bot_scope_uses), style = MaterialTheme.typography.bodySmall)
            if (limited) TeamBotScopeTag(stringResource(R.string.teams_bot_scope_limited))
        }
        if (scope.localApps.isNotEmpty()) {
            Text(stringResource(R.string.teams_bot_scope_apps), style = MaterialTheme.typography.bodySmall, fontWeight = FontWeight.SemiBold)
            Text(scope.localApps.joinToString(separator) { it.name }, Modifier.padding(start = 12.dp), style = MaterialTheme.typography.bodySmall)
        }
        if (scope.datasets.isNotEmpty()) {
            Text(stringResource(R.string.teams_bot_scope_datasets), style = MaterialTheme.typography.bodySmall, fontWeight = FontWeight.SemiBold)
            scope.datasets.forEach { row ->
                // The name and what the Bot may do with it, as one element.
                Row(Modifier.padding(start = 12.dp).semantics(mergeDescendants = true) { }, horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(row.dataset, style = MaterialTheme.typography.bodySmall)
                    if (row.write) TeamBotScopeTag(stringResource(R.string.teams_bot_scope_edit))
                }
            }
        }
    }
}

/** What [bot] may use in [conversation]: what its maintainer published, cut down to what the Chat allows. Without a Chat, all of what was published. */
@Composable
internal fun TeamBotScopeSummary(bot: TeamBotCard, conversation: TeamMessageConversation?, modifier: Modifier = Modifier) {
    val effective = TeamBotScopeRules.effective(bot, conversation)
    TeamBotScopeSummary(effective, TeamBotScopeRules.narrowed(bot.scope, effective), modifier, "teams.bot.scope.${bot.sharedBotId}")
}

/**
 * "What Bots can use in this chat" (`packages/teams/BotScopeDialog.tsx`): one section per Bot of the Chat that shares something of its maintainer's computer,
 * with what it shares as checkboxes, ticked where the Chat allows it. Saving names what stays allowed; allowing everything again lifts the limit. A change someone
 * made meanwhile, or the maintainer, is not written over: what the Bot may use now is shown again, with the reason.
 *
 * [onSave] sends one Bot's narrowing from the revision of its entry and returns the revision that the write made; it fails with a [TeamMessageFailure].
 * [onRefreshBots] reads the Bot cards again.
 */
@Composable
internal fun TeamBotScopeSheet(
    conversation: TeamMessageConversation, bots: List<TeamBotCard>, uid: String, online: Boolean,
    onSave: suspend (sharedBotId: String, expectedRevision: Long, narrowing: TeamBotScopeNarrowing?) -> Long, onRefreshBots: () -> Unit, onDismiss: () -> Unit,
) {
    TeamsSheet(stringResource(R.string.teams_bot_scope_title), onDismiss) {
        Text(stringResource(R.string.teams_bot_scope_note), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        TeamBotScopeRules.limitable(conversation, bots, uid).forEach { bot ->
            key(bot.sharedBotId) { TeamBotScopeBotSection(bot, conversation.botScopes?.get(bot.sharedBotId), online, onSave, onRefreshBots) }
        }
    }
}

/** One Bot of the Chat. What is shown follows the Chat and the Bot as they are now ([bot], [entry]); the ticks are what is chosen. */
@Composable
private fun TeamBotScopeBotSection(
    bot: TeamBotCard, entry: TeamBotScopeEntry?, online: Boolean,
    onSave: suspend (String, Long, TeamBotScopeNarrowing?) -> Long, onRefreshBots: () -> Unit,
) {
    val basis = TeamBotScopeBasis.of(bot.scope, entry)
    // The ticks start from what the Chat allows. When that changes (a save, someone else, the maintainer), they start again from it.
    var choice by remember(basis) { mutableStateOf(basis.allowed) }
    /** Null before anything was sent for this Bot; empty once it was written; otherwise the code it failed with. */
    var result by remember(bot.sharedBotId) { mutableStateOf<String?>(null) }
    var sending by remember(bot.sharedBotId) { mutableStateOf(false) }
    /** The revision this Bot's entry got from the last write: its checkboxes wait until the listener brings it, as another change before then would conflict. */
    var written by remember(bot.sharedBotId) { mutableLongStateOf(0L) }
    val scope = rememberCoroutineScope()
    val enabled = online && !sending && written <= basis.revision
    fun edit(next: TeamBotScopeChoice) { choice = next; result = null }
    fun send(narrowing: TeamBotScopeNarrowing?) {
        if (!enabled) return
        sending = true; result = null
        scope.launch {
            try { written = onSave(bot.sharedBotId, basis.revision, narrowing); result = "" }
            catch (failure: CancellationException) { throw failure }
            catch (failure: TeamMessageFailure) {
                result = failure.code
                // What the Bot shares, or what the Chat holds, changed under this person: the Bots are read again, and the Chat's side arrives by its listener.
                if (failure.code == "SCOPE_NOT_PUBLISHED" || failure.code == "BOT_SCOPE_REVISION_CONFLICT") onRefreshBots()
            }
            catch (_: Exception) { result = "SERVICE_UNAVAILABLE" }
            finally { sending = false }
        }
    }
    Column(Modifier.fillMaxWidth().testTag("teams.botscope.${bot.sharedBotId}"), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(stringResource(R.string.teams_bot_scope_limit_title_fmt, bot.name), Modifier.semantics { heading() }, style = MaterialTheme.typography.titleSmall)
        if (basis.published.localApps.isNotEmpty()) {
            Text(stringResource(R.string.teams_bot_scope_apps), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
            basis.published.localApps.forEach { app ->
                TeamBotScopeCheckRow(app.name, false, app.id in choice.localApps, enabled, "teams.botscope.${bot.sharedBotId}.app.${app.id}") {
                    edit(choice.copy(localApps = TeamBotScopeRules.toggled(choice.localApps, app.id, it)))
                }
            }
        }
        if (basis.published.datasets.isNotEmpty()) {
            Text(stringResource(R.string.teams_bot_scope_datasets), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
            basis.published.datasets.forEach { row ->
                TeamBotScopeCheckRow(row.dataset, row.write, row.dataset in choice.datasets, enabled, "teams.botscope.${bot.sharedBotId}.dataset.${row.dataset}") {
                    edit(choice.copy(datasets = TeamBotScopeRules.toggled(choice.datasets, row.dataset, it)))
                }
            }
        }
        result?.let { code ->
            Text(
                stringResource(if (code.isEmpty()) R.string.teams_bot_scope_saved else teamsErrorRes(code)), style = MaterialTheme.typography.bodySmall,
                color = if (code.isEmpty()) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.error,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }.testTag(if (code.isEmpty()) "teams.botscope.${bot.sharedBotId}.saved" else "teams.botscope.${bot.sharedBotId}.error"),
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Button(enabled = enabled && !TeamBotScopeRules.same(choice, basis.allowed), onClick = { send(TeamBotScopeRules.narrowing(choice, basis.published)) },
                modifier = Modifier.testTag("teams.botscope.${bot.sharedBotId}.save")) { Text(stringResource(R.string.common_save)) }
            TextButton(enabled = enabled && TeamBotScopeRules.limits(entry), onClick = { send(null) },
                modifier = Modifier.testTag("teams.botscope.${bot.sharedBotId}.reset")) { Text(stringResource(R.string.teams_bot_scope_reset)) }
        }
    }
}

/** One checkbox: the whole row is the control and reads "name, Can edit, checkbox, ticked". */
@Composable
private fun TeamBotScopeCheckRow(label: String, canEdit: Boolean, checked: Boolean, enabled: Boolean, tag: String, onChange: (Boolean) -> Unit) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag(tag).toggleable(value = checked, enabled = enabled, role = Role.Checkbox, onValueChange = onChange),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Checkbox(checked = checked, enabled = enabled, onCheckedChange = null)
        Text(label, Modifier.weight(1f, fill = false), maxLines = 2, overflow = TextOverflow.Ellipsis)
        if (canEdit) TeamBotScopeTag(stringResource(R.string.teams_bot_scope_edit))
    }
}
