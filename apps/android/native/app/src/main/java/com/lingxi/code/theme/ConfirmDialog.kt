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
 * error colour. [confirmTag] is the confirm button's test tag. [cancel] is for the one case where "Cancel" alone is ambiguous: a question about
 * leaving something half done, where the exact word for staying is better ("Keep editing"); the button is the Cancel all the same. [dialogTag] is
 * the test tag of the dialog itself.
 */
@Composable
fun ConfirmDialog(
    title: String, confirm: String, onConfirm: () -> Unit, onDismiss: () -> Unit,
    body: String? = null, destructive: Boolean = false, enabled: Boolean = true, confirmTag: String? = null, cancel: String? = null, dialogTag: String? = null,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        modifier = if (dialogTag != null) Modifier.testTag(dialogTag) else Modifier,
        title = { Text(title) },
        text = body?.let { text -> { Text(text) } },
        confirmButton = {
            TextButton(onClick = onConfirm, enabled = enabled, modifier = if (confirmTag != null) Modifier.testTag(confirmTag) else Modifier) {
                Text(confirm, color = if (destructive) MaterialTheme.colorScheme.error else Color.Unspecified)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(cancel ?: stringResource(R.string.common_cancel)) } },
    )
}
