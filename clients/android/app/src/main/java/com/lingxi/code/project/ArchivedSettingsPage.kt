package com.lingxi.code.project

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R

/** Restoration changes catalog metadata only; the retained conversation remains intact. */
@Composable
fun ArchivedSettingsPage(
    state: ProjectStoreState,
    onRestore: (projectId: String?, sessionId: String) -> Unit,
    onArchive: (projectId: String?, sessionId: String) -> Unit,
) {
    val conversationsLabel = stringResource(R.string.drawer_tab_chat)
    val sessions = state.globalSessions.map { Triple<String?, String, ProjectSessionSummary>(null, conversationsLabel, it) } +
        state.projects.flatMap { project ->
            project.sessions.map { Triple(project.record.id, project.record.name, it) }
        }
    val archived = sessions.filter { it.third.isArchived }
    val active = sessions.filterNot { it.third.isArchived }
    LazyColumn(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item { Text(stringResource(R.string.settings_parity_archived), Modifier.padding(16.dp), style = MaterialTheme.typography.titleMedium) }
        if (archived.isEmpty()) {
            item { Text(stringResource(R.string.settings_parity_empty_archive), Modifier.padding(16.dp)) }
        }
        items(archived, key = { "${it.first}:${it.third.sessionId}" }) { (projectId, scopeName, session) ->
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Column(Modifier.weight(1f)) {
                    Text(session.title, style = MaterialTheme.typography.bodyLarge)
                    Text(scopeName, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                TextButton(onClick = { onRestore(projectId, session.sessionId) }) { Text(stringResource(R.string.settings_parity_restore)) }
            }
        }
        if (active.isNotEmpty()) {
            item { Text(stringResource(R.string.settings_parity_archive_conversation), Modifier.padding(16.dp), style = MaterialTheme.typography.titleMedium) }
            items(active, key = { "${it.first}:${it.third.sessionId}" }) { (projectId, scopeName, session) ->
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Column(Modifier.weight(1f)) {
                        Text(session.title, style = MaterialTheme.typography.bodyLarge)
                        Text(scopeName, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    TextButton(onClick = { onArchive(projectId, session.sessionId) }) { Text(stringResource(R.string.settings_parity_archive)) }
                }
            }
        }
    }
}
