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
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

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
        title = { Text("新建项目") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("项目使用稳定 UUID 保存；名称不会成为文件夹路径。")
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it.take(120) },
                    label = { Text("项目名称") },
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
                ) { Text("导入外部目录") }
                TextButton(
                    enabled = valid,
                    onClick = { onCreateInternal(name.trim()) },
                ) { Text("创建本机项目") }
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("取消") }
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
        title = { Text("发现 ${conflicts.size} 个同步冲突") },
        text = {
            Column {
                Text("默认已跳过冲突文件。请选择全部保留本机版本或外部目录版本。")
                conflicts.take(8).forEach { conflict ->
                    Text(
                        text = conflict.relativePath,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                if (conflicts.size > 8) Text("还有 ${conflicts.size - 8} 个文件…")
            }
        },
        confirmButton = {
            Row {
                TextButton(onClick = onKeepExternal) { Text("保留外部") }
                TextButton(onClick = onKeepInternal) { Text("保留本机") }
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("稍后处理") }
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
        title = { Text("项目操作失败") },
        text = { Text(message) },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text("知道了") }
        },
    )
}
