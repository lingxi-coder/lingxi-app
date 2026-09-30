package com.lingxi.code.computeruse

import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R

@Composable
fun ComputerUseApprovalDialog(
    approval: ComputerUseApproval?,
    onResolve: (String, Boolean) -> Unit,
) {
    val pending = approval ?: return
    AlertDialog(
        onDismissRequest = { onResolve(pending.id, false) },
        title = { Text(stringResource(R.string.computeruse_approval_title)) },
        text = {
            Text(
                stringResource(
                    R.string.computeruse_approval_body_fmt,
                    pending.targetPackage,
                    pending.summary,
                ),
            )
        },
        confirmButton = {
            TextButton(onClick = { onResolve(pending.id, true) }) {
                Text(stringResource(R.string.computeruse_approval_allow_once))
            }
        },
        dismissButton = {
            TextButton(onClick = { onResolve(pending.id, false) }) {
                Text(stringResource(R.string.permission_deny))
            }
        },
    )
}
