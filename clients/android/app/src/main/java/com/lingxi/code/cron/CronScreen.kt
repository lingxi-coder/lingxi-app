package com.lingxi.code.cron

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.viewmodel.compose.viewModel
import com.lingxi.code.bindings.CronTaskDto
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * The cron management screen (settings route `settings/cron`). Lists the
 * persisted scheduled tasks (with their human schedule + next run), and creates /
 * deletes them through the engine's cron FFI. Mutations re-arm the exact alarm so
 * the schedule takes effect immediately.
 *
 * Hosted inside the settings `page()` wrapper, which already provides the top bar
 * + vertical scroll + padding, so this is a plain [Column].
 */
@Composable
fun CronScreen(vm: CronManagementViewModel = viewModel()) {
    val state by vm.state.collectAsState()

    var prompt by remember { mutableStateOf("") }
    var cronExpr by remember { mutableStateOf("") }
    var recurring by remember { mutableStateOf(true) }
    var formError by remember { mutableStateOf<String?>(null) }

    Column(
        verticalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        if (!state.canScheduleExact) {
            ExactAlarmBanner()
        }
        if (!state.engineAvailable) {
            Text(
                "未配置 API Key，无法管理定时任务。请先在“智能 → 模型服务”中设置密钥。",
                color = MaterialTheme.colorScheme.error,
                fontSize = 13.sp,
            )
        }

        // ── Create form ──────────────────────────────────────────────────────
        Text("新建定时任务", fontWeight = FontWeight.SemiBold)
        OutlinedTextField(
            value = prompt,
            onValueChange = { prompt = it },
            label = { Text("提示词 / 任务") },
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = cronExpr,
            onValueChange = { cronExpr = it },
            label = { Text("Cron 表达式（分 时 日 月 周）") },
            placeholder = { Text("0 9 * * *") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(checked = recurring, onCheckedChange = { recurring = it })
            Spacer(Modifier.width(8.dp))
            Text(if (recurring) "重复执行" else "执行一次")
        }
        formError?.let { Text(it, color = MaterialTheme.colorScheme.error, fontSize = 13.sp) }
        Button(
            onClick = {
                formError = null
                if (prompt.isBlank() || cronExpr.isBlank()) {
                    formError = "提示词和 Cron 表达式都不能为空"
                    return@Button
                }
                vm.create(cronExpr, prompt, recurring) { error ->
                    if (error == null) {
                        prompt = ""
                        cronExpr = ""
                    } else {
                        formError = error
                    }
                }
            },
            enabled = state.engineAvailable,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("创建")
        }

        Spacer(Modifier.height(8.dp))

        // ── Existing jobs ────────────────────────────────────────────────────
        Text("已创建（${state.jobs.size}）", fontWeight = FontWeight.SemiBold)
        if (state.loading) {
            CircularProgressIndicator()
        }
        state.jobs.forEach { job ->
            CronJobRow(job = job, onDelete = { vm.delete(job.id) })
        }
        if (!state.loading && state.engineAvailable && state.jobs.isEmpty()) {
            Text("还没有定时任务。", color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 13.sp)
        }
    }
}

@Composable
private fun CronJobRow(job: CronTaskDto, onDelete: () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(),
    ) {
        Column(modifier = Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = job.prompt.lineSequence().firstOrNull()?.ifBlank { job.id } ?: job.id,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 14.sp,
                    modifier = Modifier.weight(1f),
                )
                IconButton(onClick = onDelete) {
                    Icon(Icons.Default.Delete, contentDescription = "删除")
                }
            }
            Text(
                text = job.human + if (job.recurring) "" else " · 仅一次",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 12.sp,
            )
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    text = job.cron,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 11.sp,
                    color = MaterialTheme.colorScheme.primary,
                )
                val next = job.nextFireMs?.toLong()
                if (next != null) {
                    Text(
                        text = "下次 " + formatEpochMs(next),
                        fontSize = 11.sp,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

@Composable
private fun ExactAlarmBanner() {
    val context = LocalContext.current
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                "需要“精确闹钟”权限，否则定时任务可能被系统延迟触发。",
                fontSize = 13.sp,
            )
            Button(
                onClick = {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                        runCatching {
                            context.startActivity(
                                Intent(
                                    Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM,
                                    Uri.parse("package:" + context.packageName),
                                ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                            )
                        }
                    }
                },
            ) {
                Text("授予权限")
            }
        }
    }
}

private val NEXT_FIRE_FORMAT = SimpleDateFormat("MM-dd HH:mm", Locale.getDefault())

private fun formatEpochMs(ms: Long): String = NEXT_FIRE_FORMAT.format(Date(ms))
