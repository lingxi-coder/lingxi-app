package com.lingxi.code.computeruse

import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable

@Composable
fun ComputerUseApprovalDialog(
    approval: ComputerUseApproval?,
    onResolve: (String, Boolean) -> Unit,
) {
    val pending = approval ?: return
    AlertDialog(
        onDismissRequest = { onResolve(pending.id, false) },
        title = { Text("确认高风险操作") },
        text = {
            Text(
                "目标应用：${pending.targetPackage}\n" +
                    "动作：${pending.summary}\n\n" +
                    "此确认只对当前这一次操作有效，60 秒后自动拒绝。",
            )
        },
        confirmButton = {
            TextButton(onClick = { onResolve(pending.id, true) }) {
                Text("仅允许这一次")
            }
        },
        dismissButton = {
            TextButton(onClick = { onResolve(pending.id, false) }) {
                Text("拒绝")
            }
        },
    )
}
