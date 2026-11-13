package com.lingxi.code.teams

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.lingxi.code.R
import com.lingxi.code.conversation.AIText
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import java.time.ZoneId

/**
 * A shared document (Desktop `DocumentDialog`): read it, edit it, and look through its history. Reading always checks the current Team
 * membership. The body is Markdown rendered by the same `AIText` as chat messages; links open only when they are http(s).
 *
 * An edit is saved only on top of the revision it was made from. When someone saved first, nothing is saved: the text being edited stays
 * in the editor above the newer version, and saving over it is a decision the person makes.
 */
@Composable
internal fun TeamArtifactViewer(artifact: TeamArtifact, model: TeamMessagingModel, members: List<TeamMember>, bots: List<TeamBotCard>, onClose: () -> Unit) {
    val scope = rememberCoroutineScope()
    var shown by remember(artifact.artifactId) { mutableStateOf(artifact) }
    var draft by remember(artifact.artifactId) { mutableStateOf<TeamDocumentDraft?>(null) }
    /** The newer revision that refused the draft; while set, the draft is saved on top of it. */
    var newer by remember(artifact.artifactId) { mutableStateOf<TeamArtifact?>(null) }
    var history by remember(artifact.artifactId) { mutableStateOf<TeamArtifactRevisions?>(null) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf("") }
    var leaving by remember { mutableStateOf(false) }
    var operation by remember { mutableStateOf<TeamDocumentOperation?>(null) }
    val base = newer ?: shown
    val changed = draft?.changed(base) == true

    /** One request at a time; a failure is shown and never clears what was typed. */
    fun attempt(work: suspend () -> Unit) {
        if (busy) return
        busy = true; error = ""
        scope.launch {
            try { work() } catch (failure: CancellationException) { throw failure } catch (failure: TeamMessageFailure) { error = failure.code } catch (failure: Exception) { error = "SERVICE_UNAVAILABLE" }
            finally { busy = false }
        }
    }
    fun save() {
        val text = draft ?: return
        if (!changed || !text.valid) return
        attempt {
            val current = TeamDocumentOperation.of(operation, base.revision, text)
            operation = current
            try { model.reviseArtifact(base.artifactId, base.revision, current.id, text) }
            catch (failure: TeamMessageFailure) { if (failure.code != "ARTIFACT_REVISION_CONFLICT") throw failure; newer = model.readArtifact(base.artifactId); return@attempt }
            shown = model.readArtifact(base.artifactId); draft = null; newer = null
        }
    }
    /** Leaves the editor without saving. After a refused save the reader shows the newer revision, not the one the edit started from. */
    fun stopEditing() {
        draft = null; error = ""; leaving = false
        newer?.let { shown = it; newer = null }
    }
    // Unsaved text is never lost to Back: leaving asks first.
    fun back() {
        when {
            draft != null -> if (changed) leaving = true else stopEditing()
            history != null -> { history = null; error = "" }
            else -> onClose()
        }
    }

    Dialog(onDismissRequest = ::back, properties = DialogProperties(usePlatformDefaultWidth = false, dismissOnClickOutside = false)) {
        Surface(Modifier.fillMaxSize().testTag("teams.artifact.${shown.artifactId}"), color = MaterialTheme.colorScheme.surface) {
            Column(Modifier.fillMaxSize().systemBarsPadding().imePadding()) {
                val text = draft
                val rows = history
                Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    if (text == null && rows == null) IconButton(onClick = onClose) { Icon(Icons.Outlined.Close, stringResource(R.string.common_close)) }
                    else IconButton(onClick = ::back) { Icon(Icons.AutoMirrored.Outlined.ArrowBack, stringResource(if (text != null && newer != null) R.string.teams_doc_discard_mine else R.string.remote_back)) }
                    Text(stringResource(when { text != null -> R.string.teams_doc_edit_title; rows != null -> R.string.teams_doc_history_title; else -> R.string.teams_artifact_title }),
                        Modifier.weight(1f), style = MaterialTheme.typography.titleLarge, maxLines = 1)
                    if (text != null) Button(enabled = !busy && changed && text.valid, onClick = ::save, modifier = Modifier.padding(end = 12.dp).testTag("teams.artifact.save")) {
                        Text(when { busy -> stringResource(R.string.teams_doc_saving); newer != null -> stringResource(R.string.teams_doc_save_over_fmt, base.revision); else -> stringResource(R.string.common_save) })
                    }
                }
                if (busy) LinearProgressIndicator(Modifier.fillMaxWidth()) else Box(Modifier.padding(top = 4.dp))
                when {
                    text != null -> DocumentEditor(text, newer, error, Modifier.weight(1f)) { draft = it }
                    rows != null -> DocumentHistory(rows, members, bots, !busy, error, Modifier.weight(1f)) { revision ->
                        attempt { shown = model.readArtifact(shown.artifactId, revision); history = null }
                    }
                    else -> {
                        DocumentReader(shown, error, Modifier.weight(1f))
                        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                            if (shown.old) TextButton(enabled = !busy, onClick = { attempt { shown = model.readArtifact(shown.artifactId) } }, modifier = Modifier.testTag("teams.artifact.latest")) { Text(stringResource(R.string.teams_doc_show_latest)) }
                            else TextButton(enabled = !busy, onClick = { error = ""; draft = TeamDocumentDraft(shown) }, modifier = Modifier.testTag("teams.artifact.edit")) { Text(stringResource(R.string.teams_doc_edit)) }
                            Box(Modifier.weight(1f))
                            TextButton(enabled = !busy, onClick = { attempt { history = model.listArtifactRevisions(shown.artifactId) } }, modifier = Modifier.testTag("teams.artifact.history")) { Text(stringResource(R.string.teams_doc_history)) }
                        }
                    }
                }
            }
        }
    }
    if (leaving) AlertDialog(onDismissRequest = { leaving = false }, text = { Text(stringResource(R.string.teams_doc_unsaved)) },
        confirmButton = { TextButton(onClick = ::stopEditing, modifier = Modifier.testTag("teams.artifact.discard")) { Text(stringResource(R.string.teams_doc_discard), color = MaterialTheme.colorScheme.error) } },
        dismissButton = { TextButton(onClick = { leaving = false }) { Text(stringResource(R.string.teams_doc_keep_editing)) } })
}

@Composable
private fun DocumentFailure(error: String) {
    if (error.isNotEmpty()) Text(stringResource(teamsErrorRes(error)), Modifier.testTag("teams.artifact.error"), color = MaterialTheme.colorScheme.error)
}

@Composable
private fun DocumentReader(shown: TeamArtifact, error: String, modifier: Modifier) {
    Column(modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(shown.title, style = MaterialTheme.typography.headlineSmall)
        Text(if (shown.old) stringResource(R.string.teams_doc_revision_old_fmt, shown.revision, shown.currentRevision) else stringResource(R.string.teams_doc_revision_fmt, shown.revision),
            Modifier.testTag("teams.artifact.revision"), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        SelectionContainer { AIText(markdown = shown.body, modifier = Modifier.fillMaxWidth().testTag("teams.artifact.body"), onOpenLink = rememberTeamBotLinkOpener()) }
        DocumentFailure(error)
    }
}

@Composable
private fun DocumentEditor(draft: TeamDocumentDraft, newer: TeamArtifact?, error: String, modifier: Modifier, onChange: (TeamDocumentDraft) -> Unit) {
    Column(modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        OutlinedTextField(draft.title, { onChange(draft.copy(title = it)) }, Modifier.fillMaxWidth().testTag("teams.artifact.edit.title"), singleLine = true,
            label = { Text(stringResource(R.string.teams_doc_field_title)) }, isError = draft.trimmedTitle.isEmpty() || draft.trimmedTitle.length > 120)
        OutlinedTextField(draft.body, { onChange(draft.copy(body = it)) }, Modifier.fillMaxWidth().heightIn(min = 260.dp).testTag("teams.artifact.edit.body"),
            label = { Text(stringResource(R.string.teams_doc_field_body)) }, isError = draft.tooLong, textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace))
        if (draft.tooLong) Text(stringResource(R.string.teams_doc_too_long), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        DocumentFailure(error)
        if (newer != null) {
            Text(stringResource(R.string.teams_doc_conflict_fmt, newer.revision), Modifier.testTag("teams.artifact.conflict"), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.tertiary)
            Text(stringResource(R.string.teams_doc_newer), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(newer.title, style = MaterialTheme.typography.titleMedium)
            SelectionContainer { AIText(markdown = newer.body, modifier = Modifier.fillMaxWidth(), onOpenLink = rememberTeamBotLinkOpener()) }
        }
    }
}

@Composable
private fun DocumentHistory(history: TeamArtifactRevisions, members: List<TeamMember>, bots: List<TeamBotCard>, enabled: Boolean, error: String, modifier: Modifier, onOpen: (Long) -> Unit) {
    val strings = rememberTeamsListStrings()
    val locale = LocalConfiguration.current.locales[0]
    val zone = remember { ZoneId.systemDefault() }
    LazyColumn(modifier.fillMaxWidth()) {
        items(history.revisions, key = { it.revision }) { row ->
            Column(Modifier.fillMaxWidth().clickable(enabled = enabled, role = Role.Button) { onOpen(row.revision) }.padding(horizontal = 20.dp, vertical = 10.dp).testTag("teams.artifact.revision.${row.revision}"),
                verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(R.string.teams_doc_revision_fmt, row.revision), style = MaterialTheme.typography.titleSmall)
                    if (row.revision == history.currentRevision) Text(stringResource(R.string.teams_doc_current), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Text(row.title, style = MaterialTheme.typography.bodyMedium)
                Text("${TeamDocumentRules.authorName(row.author, members, bots, strings)} · ${formatDate(row.createdAt, zone, locale, "yMMMdjm")}",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        if (error.isNotEmpty()) item("error") { Box(Modifier.padding(horizontal = 20.dp, vertical = 8.dp)) { DocumentFailure(error) } }
    }
}
