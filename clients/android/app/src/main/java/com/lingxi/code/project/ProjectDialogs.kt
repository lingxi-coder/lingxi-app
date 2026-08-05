package com.lingxi.code.project

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.lingxi.code.R

@Composable
fun CreateProjectDialog(
    visible: Boolean,
    onDismiss: () -> Unit,
    onCreateInternal: (String) -> Unit,
    onChooseExternal: (String) -> Unit,
) {
    if (!visible) return
    var name by remember { mutableStateOf("") }
    val valid = name.trim().isNotEmpty()
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.project_dialog_create_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(stringResource(R.string.project_dialog_create_body))
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it.take(120) },
                    label = { Text(stringResource(R.string.drawer_project_name_placeholder)) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        },
        confirmButton = {
            Row {
                TextButton(
                    enabled = valid,
                    onClick = { onChooseExternal(name.trim()) },
                ) { Text(stringResource(R.string.project_dialog_import_external_button)) }
                TextButton(
                    enabled = valid,
                    onClick = { onCreateInternal(name.trim()) },
                ) { Text(stringResource(R.string.project_dialog_create_internal_button)) }
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_cancel)) }
        },
    )
}

@Composable
fun ProjectConflictDialog(
    conflicts: List<ProjectSyncConflict>,
    onKeepExternal: () -> Unit,
    onKeepInternal: () -> Unit,
    onDismiss: () -> Unit,
) {
    if (conflicts.isEmpty()) return
    val projectId = conflicts.first().projectId
    AlertDialog(
        onDismissRequest = onDismiss,
        title = {
            Text(stringResource(R.string.project_conflict_dialog_title_fmt, conflicts.size))
        },
        text = {
            Column {
                Text(stringResource(R.string.project_conflict_dialog_body))
                conflicts.take(8).forEach { conflict ->
                    Text(
                        text = conflict.relativePath,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                if (conflicts.size > 8) {
                    Text(
                        stringResource(
                            R.string.project_conflict_dialog_more_files_fmt,
                            conflicts.size - 8,
                        ),
                    )
                }
            }
        },
        confirmButton = {
            Row {
                TextButton(onClick = onKeepExternal) {
                    Text(stringResource(R.string.project_conflict_keep_external_button))
                }
                TextButton(onClick = onKeepInternal) {
                    Text(stringResource(R.string.project_conflict_keep_internal_button))
                }
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text(stringResource(R.string.project_conflict_defer_button))
            }
        },
    )
}

@Composable
fun ProjectErrorDialog(
    message: String?,
    onDismiss: () -> Unit,
) {
    if (message == null) return
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.project_error_dialog_title)) },
        text = { Text(message) },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_got_it)) }
        },
    )
}
