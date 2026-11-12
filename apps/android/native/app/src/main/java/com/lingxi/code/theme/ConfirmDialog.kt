package com.lingxi.code.theme

import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R

/**
 * A confirmation (docs/TEAMS-UI-POLISH.md U8): a [title] that names the thing, a [body] only when it adds something, a verb for the
 * [confirm] button, and a Cancel that is always there (`common_cancel`, never the word "Confirm"). A [destructive] confirm is drawn in the
 * error colour. [confirmTag] is the confirm button's test tag.
 */
@Composable
fun ConfirmDialog(
    title: String, confirm: String, onConfirm: () -> Unit, onDismiss: () -> Unit,
    body: String? = null, destructive: Boolean = false, enabled: Boolean = true, confirmTag: String? = null,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = body?.let { text -> { Text(text) } },
        confirmButton = {
            TextButton(onClick = onConfirm, enabled = enabled, modifier = if (confirmTag != null) Modifier.testTag(confirmTag) else Modifier) {
                Text(confirm, color = if (destructive) MaterialTheme.colorScheme.error else Color.Unspecified)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_cancel)) } },
    )
}
