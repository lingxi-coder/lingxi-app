package com.lingxi.code.teams

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import java.time.ZoneId

/**
 * What a task did outside the Chat (Desktop `OperationsDialog`). A task that needs checking ends only when each action nobody could verify
 * is known to have happened or not; the Bot's maintainer says which, after checking where the action took place. The requester sees the
 * same list without the choice.
 */
@Composable
internal fun TeamOperationsDialog(taskId: String, model: TeamMessagingModel, canResolve: Boolean, members: List<TeamMember>, onClose: () -> Unit) {
    val scope = rememberCoroutineScope()
    var rows by remember(taskId) { mutableStateOf<List<TeamTaskOperation>?>(null) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf("") }
    val strings = rememberTeamsListStrings()
    val locale = LocalConfiguration.current.locales[0]
    val zone = remember { ZoneId.systemDefault() }

    suspend fun load() { rows = model.listOperations(taskId) }
    /** One request at a time. */
    fun attempt(work: suspend () -> Unit) {
        if (busy) return
        busy = true; error = ""
        scope.launch {
            try { work() } catch (failure: CancellationException) { throw failure } catch (failure: TeamMessageFailure) { error = failure.code } catch (failure: Exception) { error = "SERVICE_UNAVAILABLE" }
            finally { busy = false }
        }
    }
    /** After a decision the list is read again whether or not it was accepted, so it never shows a choice that is gone. */
    fun resolve(operation: TeamTaskOperation, happened: Boolean) = attempt {
        try { model.resolveOperation(taskId, operation.operationId, happened) }
        catch (failure: TeamMessageFailure) { runCatching { load() }.onFailure { if (it is CancellationException) throw it }; throw failure }
        load()
    }
    LaunchedEffect(taskId) { attempt { load() } }

    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize().testTag("teams.ops.$taskId"), color = MaterialTheme.colorScheme.surface) {
            Column(Modifier.fillMaxSize().systemBarsPadding()) {
                Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = onClose) { Icon(Icons.Outlined.Close, stringResource(R.string.common_close)) }
                    Text(stringResource(R.string.teams_ops_title), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge, maxLines = 1)
                    IconButton(enabled = !busy, onClick = { attempt { load() } }, modifier = Modifier.testTag("teams.ops.refresh")) { Icon(Icons.Outlined.Refresh, stringResource(R.string.teams_list_refresh)) }
                }
                if (busy) LinearProgressIndicator(Modifier.fillMaxWidth()) else Box(Modifier.padding(top = 4.dp))
                val shown = rows
                LazyColumn(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    if (shown == null) {
                        if (error.isEmpty()) item("loading") { Text(stringResource(R.string.teams_doc_loading), color = MaterialTheme.colorScheme.onSurfaceVariant) }
                    } else {
                        if (shown.any { it.unverified }) item("intro") {
                            Text(stringResource(if (canResolve) R.string.teams_ops_intro_maintainer else R.string.teams_ops_intro_requester), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        if (shown.isEmpty()) item("empty") { Text(stringResource(R.string.teams_ops_empty), color = MaterialTheme.colorScheme.onSurfaceVariant) }
                        items(shown, key = { it.operationId }) { operation ->
                            val by = operation.resolvedBy?.let { ref -> members.firstOrNull { it.uid == ref.uid }?.displayName.orEmpty().ifEmpty { strings.member } }
                            Column(Modifier.fillMaxWidth().testTag("teams.ops.row.${operation.operationId}"), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text(operation.toolName, style = MaterialTheme.typography.titleSmall)
                                Text(stringResource(TeamOperationRules.stateRes(operation)), style = MaterialTheme.typography.bodyMedium,
                                    color = if (operation.unverified) MaterialTheme.colorScheme.tertiary else MaterialTheme.colorScheme.onSurfaceVariant)
                                Text(formatDate(operation.dispatchedAt, zone, locale, "yMMMdjm") + if (by != null) " · " + stringResource(R.string.teams_ops_confirmed_by_fmt, by) else "",
                                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                if (canResolve && operation.unverified) Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    val happened = stringResource(R.string.teams_ops_happened_label_fmt, operation.toolName)
                                    val not = stringResource(R.string.teams_ops_not_happened_label_fmt, operation.toolName)
                                    OutlinedButton(enabled = !busy, onClick = { resolve(operation, true) }, modifier = Modifier.semantics { contentDescription = happened }.testTag("teams.ops.happened.${operation.operationId}")) { Text(stringResource(R.string.teams_ops_happened)) }
                                    OutlinedButton(enabled = !busy, onClick = { resolve(operation, false) }, modifier = Modifier.semantics { contentDescription = not }.testTag("teams.ops.not_happened.${operation.operationId}")) { Text(stringResource(R.string.teams_ops_not_happened)) }
                                }
                            }
                        }
                    }
                    if (error.isNotEmpty()) item("error") { Text(stringResource(TeamOperationRules.errorRes(error)), Modifier.testTag("teams.ops.error"), color = MaterialTheme.colorScheme.error) }
                }
            }
        }
    }
}
