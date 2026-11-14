package com.lingxi.code.teams

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import java.text.DateFormat
import java.util.Date

/**
 * A Bot's shared list (Desktop `SharedListDialog`): the people who may use the Bot and the Bot's own runs change the same items.
 * A change is saved only on top of the list it was made from. When someone else, or the Bot, saved first, nothing is saved: the list is
 * read again, what the person typed stays where it is, and saving it again is their decision.
 */
@Composable
internal fun TeamSharedListDialog(bot: TeamBotCard, model: TeamMessagingModel, members: List<TeamMember>, onClose: () -> Unit) {
    val scope = rememberCoroutineScope()
    var list by remember { mutableStateOf<TeamSharedList?>(null) }
    var text by remember { mutableStateOf("") }
    var editing by remember { mutableStateOf<Pair<String, String>?>(null) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf("") }
    var moved by remember { mutableStateOf(false) }
    var operation by remember { mutableStateOf<TeamSharedListOperation?>(null) }
    var history by remember { mutableStateOf<TeamSharedListHistory?>(null) }
    var historyOpen by remember { mutableStateOf(false) }
    var historyError by remember { mutableStateOf("") }

    /** Reads the list. A read that answers after a newer one is dropped, so the list never goes back. */
    suspend fun read() {
        val next = model.readList(bot.sharedBotId)
        if (next.dataRevision < (list?.dataRevision ?: -1)) return
        list = next
        if (editing != null && next.items.none { it.itemId == editing?.first }) editing = null
    }
    /** One request at a time; a failure is shown under the list and never clears what was typed. */
    fun attempt(work: suspend () -> Unit) {
        if (busy) return
        busy = true; error = ""
        scope.launch {
            try { work() } catch (failure: CancellationException) { throw failure } catch (failure: TeamMessageFailure) { error = failure.code } catch (failure: Exception) { error = "SERVICE_UNAVAILABLE" }
            finally { busy = false }
        }
    }
    fun write(changes: List<TeamSharedListChange>, saved: () -> Unit = {}) {
        val current = list ?: return
        attempt {
            val next = TeamSharedListOperation.of(operation, current.dataRevision, changes)
            operation = next; moved = false
            try { model.writeList(bot.sharedBotId, next.id, current.dataRevision, changes) }
            catch (failure: TeamMessageFailure) { if (!TeamSharedListRules.moved(failure.code)) throw failure; read(); moved = true; return@attempt }
            saved(); read()
        }
    }
    fun add() { if (TeamSharedListRules.usable(text)) write(listOf(TeamSharedListChange.Add(text.trim()))) { text = "" } }
    /** The newest page (`before` null) or the page before `before`. A failure is shown in the history and keeps what is shown. */
    suspend fun page(before: Long?) {
        try { history = TeamSharedListHistoryRules.merge(history, model.listHistory(bot.sharedBotId, before), before == null); historyError = "" }
        catch (failure: CancellationException) { throw failure } catch (failure: TeamMessageFailure) { historyError = failure.code } catch (failure: Exception) { historyError = "SERVICE_UNAVAILABLE" }
    }
    LaunchedEffect(bot.sharedBotId) { attempt { read() } }
    // Someone else, or the Bot, changed the list: show it without waiting for Refresh. What is being typed stays.
    LaunchedEffect(bot.sharedBotId) {
        model.state.map { it.botSignals }.distinctUntilChanged().drop(1).collect {
            try { read() } catch (failure: CancellationException) { throw failure } catch (_: Exception) {}
            if (history != null) page(null)
        }
    }
    LaunchedEffect(historyOpen) { if (historyOpen && history == null) page(null) }

    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize().testTag("teams.list.${bot.sharedBotId}"), color = MaterialTheme.colorScheme.surface) {
            Column(Modifier.fillMaxSize().systemBarsPadding().imePadding()) {
                Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = onClose) { Icon(Icons.Outlined.Close, stringResource(R.string.common_close)) }
                    Text(list?.title ?: bot.sharedList?.title.orEmpty(), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge, maxLines = 1)
                    IconButton(enabled = !busy, onClick = { attempt { moved = false; read(); if (history != null) page(null) } }, modifier = Modifier.testTag("teams.list.refresh")) {
                        Icon(Icons.Outlined.Refresh, stringResource(R.string.teams_list_refresh))
                    }
                }
                if (busy) LinearProgressIndicator(Modifier.fillMaxWidth()) else Box(Modifier.padding(top = 4.dp))
                val shown = list
                LazyColumn(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    if (shown == null) {
                        if (error.isEmpty()) item("loading") { Text(stringResource(R.string.teams_list_loading), color = MaterialTheme.colorScheme.onSurfaceVariant) }
                    } else {
                        item("hint") { Text(stringResource(R.string.teams_list_hint_fmt, bot.name), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                        if (moved) item("moved") { Text(stringResource(R.string.teams_list_moved), Modifier.testTag("teams.list.moved"), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.tertiary) }
                        if (shown.items.isEmpty()) item("empty") { Text(stringResource(R.string.teams_list_empty), color = MaterialTheme.colorScheme.onSurfaceVariant) }
                        items(shown.items, key = { it.itemId }) { item ->
                            val edit = editing?.takeIf { it.first == item.itemId }
                            if (edit != null) ListItemEditor(item, edit.second, busy, onText = { editing = item.itemId to it }, onCancel = { editing = null },
                                onSave = { write(listOf(TeamSharedListChange.SetText(item.itemId, edit.second.trim()))) { editing = null } })
                            else ListItemRow(item, !busy, onDone = { write(listOf(TeamSharedListChange.SetDone(item.itemId, it))) }, onEdit = { editing = item.itemId to item.text },
                                onRemove = { write(listOf(TeamSharedListChange.Remove(item.itemId))) })
                        }
                    }
                    if (error.isNotEmpty()) item("error") { Text(stringResource(teamsErrorRes(error)), Modifier.testTag("teams.list.error"), color = MaterialTheme.colorScheme.error) }
                    if (shown != null) {
                        item("history") {
                            TextButton(onClick = { historyOpen = !historyOpen }, modifier = Modifier.testTag("teams.list.history")) { Text(stringResource(R.string.teams_list_history)) }
                        }
                        if (historyOpen) {
                            val loaded = history
                            if (loaded == null) { if (historyError.isEmpty()) item("history-loading") { Text(stringResource(R.string.teams_list_history_loading), color = MaterialTheme.colorScheme.onSurfaceVariant) } }
                            else {
                                if (loaded.revisions.isEmpty()) item("history-empty") { Text(stringResource(R.string.teams_list_history_empty), color = MaterialTheme.colorScheme.onSurfaceVariant) }
                                items(loaded.revisions, key = { "revision-${it.dataRevision}" }) { revision -> ListRevision(revision, model.uid, members, bot.name) }
                                if (loaded.hasMore) item("history-older") {
                                    TextButton(onClick = { scope.launch { page(loaded.revisions.lastOrNull()?.dataRevision) } }, modifier = Modifier.testTag("teams.list.history.older")) {
                                        Text(stringResource(R.string.teams_list_history_older))
                                    }
                                }
                            }
                            if (historyError.isNotEmpty()) item("history-error") { Text(stringResource(teamsErrorRes(historyError)), color = MaterialTheme.colorScheme.error) }
                            item("history-kept") { Text(stringResource(R.string.teams_list_history_kept), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                        }
                    }
                }
                if (shown != null) Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlinedTextField(text, { text = it }, Modifier.weight(1f).testTag("teams.list.new"), singleLine = true,
                            placeholder = { Text(stringResource(R.string.teams_list_add_placeholder)) }, isError = !TeamSharedListRules.fits(text),
                            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done), keyboardActions = KeyboardActions(onDone = { add() }))
                        Button(enabled = !busy && TeamSharedListRules.usable(text), onClick = { add() }, modifier = Modifier.testTag("teams.list.add")) { Text(stringResource(R.string.teams_list_add)) }
                    }
                    if (!TeamSharedListRules.fits(text)) Text(stringResource(R.string.teams_list_too_long), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                }
            }
        }
    }
}

@Composable
private fun ListRevision(revision: TeamSharedListRevision, viewer: String, members: List<TeamMember>, botName: String) {
    @Composable fun person(person: TeamSharedListHistoryRules.Person, inline: Boolean) = when (person) {
        TeamSharedListHistoryRules.Person.You -> stringResource(if (inline) R.string.teams_list_history_you_inline else R.string.teams_list_history_you)
        is TeamSharedListHistoryRules.Person.Member -> person.name
        TeamSharedListHistoryRules.Person.Former -> stringResource(if (inline) R.string.teams_list_history_former_inline else R.string.teams_list_history_former)
    }
    val author = when (val who = TeamSharedListHistoryRules.author(revision.author, viewer, members, botName)) {
        is TeamSharedListHistoryRules.Author.Of -> person(who.person, inline = false)
        is TeamSharedListHistoryRules.Author.Bot -> stringResource(R.string.teams_list_history_for_fmt, who.name, person(who.requester, inline = true))
    }
    val time = remember(revision.createdAt) { DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(revision.createdAt)) }
    Column(Modifier.fillMaxWidth().padding(vertical = 4.dp).semantics(mergeDescendants = true) {}, verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text("$author · $time", style = MaterialTheme.typography.labelLarge)
        for (line in revision.changes.flatMap(TeamSharedListHistoryRules::lines)) {
            Text(when (line) {
                is TeamSharedListHistoryRules.Line.Added -> stringResource(R.string.teams_list_added_fmt, line.text)
                is TeamSharedListHistoryRules.Line.Removed -> stringResource(R.string.teams_list_removed_fmt, line.text)
                is TeamSharedListHistoryRules.Line.Renamed -> stringResource(R.string.teams_list_renamed_fmt, line.was, line.text)
                is TeamSharedListHistoryRules.Line.Ticked -> stringResource(R.string.teams_list_ticked_fmt, line.text)
                is TeamSharedListHistoryRules.Line.Unticked -> stringResource(R.string.teams_list_unticked_fmt, line.text)
                is TeamSharedListHistoryRules.Line.Unchanged -> stringResource(R.string.teams_list_unchanged_fmt, line.text)
            }, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

@Composable
private fun ListItemRow(item: TeamSharedListItem, enabled: Boolean, onDone: (Boolean) -> Unit, onEdit: () -> Unit, onRemove: () -> Unit) {
    var menu by remember(item.itemId) { mutableStateOf(false) }
    val state = stringResource(if (item.done) R.string.teams_list_done else R.string.teams_list_not_done)
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        // The whole text toggles the item; the checkbox itself is decoration so the row is one control for a screen reader.
        Row(Modifier.weight(1f).toggleable(item.done, enabled = enabled, role = Role.Checkbox, onValueChange = onDone).semantics { stateDescription = state }.testTag("teams.list.item.${item.itemId}"),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Checkbox(item.done, onCheckedChange = null, enabled = enabled)
            Text(item.text, Modifier.padding(vertical = 12.dp), textDecoration = if (item.done) TextDecoration.LineThrough else null,
                color = if (item.done) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.onSurface)
        }
        Box {
            IconButton(enabled = enabled, onClick = { menu = true }) { Icon(Icons.Outlined.MoreVert, stringResource(R.string.teams_list_item_options_fmt, item.text)) }
            DropdownMenu(menu, onDismissRequest = { menu = false }) {
                DropdownMenuItem(text = { Text(stringResource(R.string.teams_list_edit)) }, onClick = { menu = false; onEdit() })
                DropdownMenuItem(text = { Text(stringResource(R.string.teams_list_remove), color = MaterialTheme.colorScheme.error) }, onClick = { menu = false; onRemove() })
            }
        }
    }
}

@Composable
private fun ListItemEditor(item: TeamSharedListItem, text: String, busy: Boolean, onText: (String) -> Unit, onCancel: () -> Unit, onSave: () -> Unit) {
    Column(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        OutlinedTextField(text, onText, Modifier.fillMaxWidth().testTag("teams.list.edit.${item.itemId}"), label = { Text(stringResource(R.string.teams_list_edit_title)) }, isError = !TeamSharedListRules.fits(text))
        if (!TeamSharedListRules.fits(text)) Text(stringResource(R.string.teams_list_too_long), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(enabled = !busy && TeamSharedListRules.usable(text) && text.trim() != item.text, onClick = onSave) { Text(stringResource(R.string.common_save)) }
            TextButton(onClick = onCancel) { Text(stringResource(R.string.common_cancel)) }
        }
    }
}
