package com.lingxi.code.cron

import android.app.DatePickerDialog
import android.app.TimePickerDialog
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.viewmodel.compose.viewModel
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Calendar

private data class SchedulePreset(val label: String, val cron: String)

private val SCHEDULE_PRESETS = listOf(
    SchedulePreset("每 15 分钟", "*/15 * * * *"),
    SchedulePreset("每小时", "0 * * * *"),
    SchedulePreset("每天 09:00", "0 9 * * *"),
    SchedulePreset("工作日 09:00", "0 9 * * 1-5"),
    SchedulePreset("每周一 09:00", "0 9 * * 1"),
)

/**
 * Android's real scheduled-task management surface.
 *
 * Task storage is scoped to the selected Project workspace. AlarmManager only
 * wakes the app; every actual run and retry is represented by WorkManager and
 * the durable history rendered below.
 */
@Composable
fun CronScreen(
    initialTaskKey: String? = null,
    initialRunId: String? = null,
    vm: CronManagementViewModel = viewModel(),
) {
    val state by vm.state.collectAsState()
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current

    var prompt by rememberSaveable { mutableStateOf("") }
    var cronExpr by rememberSaveable { mutableStateOf("0 9 * * *") }
    var recurring by rememberSaveable { mutableStateOf(true) }
    var selectedScopeId by rememberSaveable { mutableStateOf(GLOBAL_CRON_SCOPE_ID) }
    var editingTaskId by rememberSaveable { mutableStateOf<String?>(null) }
    var selectedRunId by rememberSaveable(initialRunId) { mutableStateOf(initialRunId) }
    var formError by remember { mutableStateOf<String?>(null) }
    var submitting by remember { mutableStateOf(false) }
    var appliedInitialTask by rememberSaveable(initialTaskKey) { mutableStateOf(false) }

    DisposableEffect(lifecycleOwner, vm) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                vm.refresh()
                vm.reconcile("ui-resume")
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }

    LaunchedEffect(state.activeScopeId, state.scopes) {
        if (editingTaskId == null && state.scopes.any { it.scopeId == state.activeScopeId }) {
            selectedScopeId = state.activeScopeId
        }
    }
    LaunchedEffect(initialTaskKey, state.tasks) {
        if (!appliedInitialTask && !initialTaskKey.isNullOrBlank()) {
            state.tasks.firstOrNull {
                "${it.scope.scopeId}:${it.task.id}" == initialTaskKey
            }?.let { selected ->
                selectedScopeId = selected.scope.scopeId
                editingTaskId = selected.task.id
                prompt = selected.task.prompt
                cronExpr = selected.task.cron
                recurring = selected.task.recurring
                appliedInitialTask = true
            }
        }
    }

    fun resetForm() {
        editingTaskId = null
        prompt = ""
        cronExpr = "0 9 * * *"
        recurring = true
        formError = null
        selectedScopeId = state.activeScopeId
    }

    Column(
        verticalArrangement = Arrangement.spacedBy(14.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        SchedulingSummary(state)
        initialRunId?.let { runId ->
            state.history.firstOrNull { it.runId == runId }?.let { run ->
                Text("通知对应的运行结果", fontWeight = FontWeight.SemiBold)
                RunHistoryCard(run = run, expanded = true, onClick = {})
            }
        }
        if (!state.exactAlarmAllowed) {
            ExactAlarmBanner()
        }
        state.errorMessage?.let {
            Text(it, color = MaterialTheme.colorScheme.error, fontSize = 13.sp)
        }

        Text(
            if (editingTaskId == null) "新建定时任务" else "编辑定时任务",
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            "任务在所选 Project 的 workspace 中运行；全局用于兼容旧任务。",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontSize = 12.sp,
        )
        Row(
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
        ) {
            state.scopes.forEach { scope ->
                FilterChip(
                    selected = scope.scopeId == selectedScopeId,
                    onClick = { if (editingTaskId == null) selectedScopeId = scope.scopeId },
                    enabled = editingTaskId == null,
                    label = { Text(scope.projectName) },
                )
            }
        }
        OutlinedTextField(
            value = prompt,
            onValueChange = { prompt = it },
            label = { Text("提示词 / 任务") },
            minLines = 2,
            modifier = Modifier.fillMaxWidth(),
        )
        Row(
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
        ) {
            SCHEDULE_PRESETS.forEach { preset ->
                AssistChip(
                    onClick = {
                        cronExpr = preset.cron
                        recurring = true
                    },
                    label = { Text(preset.label) },
                )
            }
            AssistChip(
                onClick = {
                    openOneShotDateTimePicker(context) { expression ->
                        cronExpr = expression
                        recurring = false
                    }
                },
                label = { Text("单次日期时间") },
            )
        }
        OutlinedTextField(
            value = cronExpr,
            onValueChange = { cronExpr = it },
            label = { Text("高级五段 Cron（分 时 日 月 周）") },
            placeholder = { Text("0 9 * * *") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(checked = recurring, onCheckedChange = { recurring = it })
            Text(
                if (recurring) "重复执行（最小间隔 15 分钟）" else "仅执行一次",
                modifier = Modifier.padding(start = 8.dp),
            )
        }
        formError?.let {
            Text(it, color = MaterialTheme.colorScheme.error, fontSize = 13.sp)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(
                onClick = {
                    formError = null
                    if (prompt.isBlank() || cronExpr.isBlank()) {
                        formError = "提示词和 Cron 表达式都不能为空"
                        return@Button
                    }
                    submitting = true
                    val completed: (String?) -> Unit = { error ->
                        submitting = false
                        formError = error
                        if (error == null) {
                            resetForm()
                            if (!state.exactAlarmAllowed) {
                                openExactAlarmSettings(context)
                            }
                        }
                    }
                    val taskId = editingTaskId
                    if (taskId == null) {
                        vm.create(selectedScopeId, cronExpr, prompt, recurring, completed)
                    } else {
                        vm.update(selectedScopeId, taskId, cronExpr, prompt, recurring, completed)
                    }
                },
                enabled = !submitting && state.scopes.any { it.scopeId == selectedScopeId },
            ) {
                Text(if (editingTaskId == null) "创建并调度" else "保存")
            }
            if (editingTaskId != null) {
                OutlinedButton(onClick = ::resetForm) { Text("取消编辑") }
            }
        }

        HorizontalDivider()
        Text("任务（${state.tasks.size}）", fontWeight = FontWeight.SemiBold)
        if (state.loading) {
            CircularProgressIndicator()
        }
        state.tasks.groupBy { it.scope }.forEach { (scope, tasks) ->
            Text(
                scope.projectName,
                color = MaterialTheme.colorScheme.primary,
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
            )
            tasks.forEach { task ->
                CronTaskCard(
                    task = task,
                    onEdit = {
                        selectedScopeId = task.scope.scopeId
                        editingTaskId = task.task.id
                        prompt = task.task.prompt
                        cronExpr = task.task.cron
                        recurring = task.task.recurring
                        formError = null
                    },
                    onRunNow = { vm.runNow(task.scope.scopeId, task.task.id) { formError = it } },
                    onDelete = {
                        vm.delete(task.scope.scopeId, task.task.id) { formError = it }
                        if (editingTaskId == task.task.id) resetForm()
                    },
                )
            }
        }
        if (!state.loading && state.tasks.isEmpty()) {
            Text(
                "还没有定时任务。",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
            )
        }

        HorizontalDivider()
        Text("运行历史", fontWeight = FontWeight.SemiBold)
        state.history.take(50).forEach { run ->
            RunHistoryCard(
                run = run,
                expanded = selectedRunId == run.runId,
                onClick = {
                    selectedRunId = if (selectedRunId == run.runId) null else run.runId
                },
            )
        }
        if (!state.loading && state.history.isEmpty()) {
            Text(
                "暂无运行记录。结果不会写入普通聊天 Session。",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
            )
        }
        Text(
            "系统限制：用户 Force Stop 后，Alarm 和 WorkManager 都会暂停，直到再次打开应用。",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontSize = 11.sp,
        )
    }
}

@Composable
private fun SchedulingSummary(state: AndroidCronRepositoryState) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier.padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(5.dp),
        ) {
            Text(
                if (state.exactAlarmAllowed) "精确闹钟已授权" else "15 分钟级非精确模式",
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                state.nextScheduledAtMs?.let { "下一次唤醒：${formatEpochMs(it)}" }
                    ?: "当前没有可调度的任务",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 12.sp,
            )
            Text(
                "执行队列 ${state.activeWorkCount} · " +
                    if (state.networkAvailable) "网络已连接" else "等待网络",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 12.sp,
            )
        }
    }
}

@Composable
private fun CronTaskCard(
    task: AndroidCronTask,
    onEdit: () -> Unit,
    onRunNow: () -> Unit,
    onDelete: () -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(),
    ) {
        Column(
            modifier = Modifier.padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = task.task.prompt.lineSequence().firstOrNull()
                        ?.ifBlank { task.task.id } ?: task.task.id,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 14.sp,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                IconButton(onClick = onRunNow, enabled = task.activeRun == null) {
                    Icon(Icons.Default.PlayArrow, contentDescription = "立即运行")
                }
                IconButton(onClick = onEdit, enabled = task.activeRun == null) {
                    Icon(Icons.Default.Edit, contentDescription = "编辑")
                }
                IconButton(onClick = onDelete, enabled = task.activeRun == null) {
                    Icon(Icons.Default.Delete, contentDescription = "删除")
                }
            }
            Text(
                task.task.human + if (task.task.recurring) "" else " · 仅一次",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 12.sp,
            )
            Text(
                task.task.cron,
                fontFamily = FontFamily.Monospace,
                fontSize = 11.sp,
                color = MaterialTheme.colorScheme.primary,
            )
            val stateText = when {
                task.schedulingMode == CronSchedulingMode.Unsupported ->
                    task.unsupportedReason ?: "Android 不支持此任务"
                task.activeRun != null -> runStatusLabel(task.activeRun.status)
                task.lastRun != null -> "最近：${runStatusLabel(task.lastRun.status)}"
                task.schedulingMode == CronSchedulingMode.FifteenMinuteFallback ->
                    "15 分钟巡检模式"
                else -> "精确闹钟模式"
            }
            Text(
                buildString {
                    append(stateText)
                    task.task.nextFireMs?.toLong()?.let {
                        append(" · 下次 ")
                        append(formatEpochMs(it))
                    }
                },
                color = if (task.schedulingMode == CronSchedulingMode.Unsupported) {
                    MaterialTheme.colorScheme.error
                } else {
                    MaterialTheme.colorScheme.onSurfaceVariant
                },
                fontSize = 11.sp,
            )
        }
    }
}

@Composable
private fun RunHistoryCard(
    run: CronRunRecord,
    expanded: Boolean,
    onClick: () -> Unit,
) {
    OutlinedButton(
        onClick = onClick,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(modifier = Modifier.fillMaxWidth()) {
            Text(
                "${run.projectName} · ${run.prompt.lineSequence().firstOrNull().orEmpty()}",
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                fontWeight = FontWeight.Medium,
            )
            Text(
                "${runStatusLabel(run.status)} · ${formatEpochMs(run.scheduledAtMs)} · 尝试 ${maxOf(1, run.attempt)}",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 11.sp,
            )
            if (expanded) {
                Spacer(Modifier.height(6.dp))
                Text(
                    run.resultText ?: run.errorMessage ?: "没有输出",
                    color = if (run.errorMessage != null) {
                        MaterialTheme.colorScheme.error
                    } else {
                        MaterialTheme.colorScheme.onSurface
                    },
                    fontSize = 12.sp,
                )
                Text(
                    "run id: ${run.runId}",
                    fontFamily = FontFamily.Monospace,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 10.sp,
                )
            }
        }
    }
}

@Composable
private fun ExactAlarmBanner() {
    val context = LocalContext.current
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier.padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                "未授予“闹钟和提醒”权限。任务仍会创建，但由 WorkManager 每 15 分钟巡检，触发时间可能延迟。",
                fontSize = 13.sp,
            )
            Button(
                onClick = {
                    openExactAlarmSettings(context)
                },
            ) {
                Text("打开“闹钟和提醒”")
            }
        }
    }
}

private fun openExactAlarmSettings(context: android.content.Context) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return
    runCatching {
        context.startActivity(
            Intent(
                Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM,
                Uri.parse("package:${context.packageName}"),
            ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }
}

private fun openOneShotDateTimePicker(
    context: android.content.Context,
    onSelected: (String) -> Unit,
) {
    val selected = Calendar.getInstance().apply { add(Calendar.HOUR_OF_DAY, 1) }
    DatePickerDialog(
        context,
        { _, year, month, day ->
            selected.set(Calendar.YEAR, year)
            selected.set(Calendar.MONTH, month)
            selected.set(Calendar.DAY_OF_MONTH, day)
            TimePickerDialog(
                context,
                { _, hour, minute ->
                    selected.set(Calendar.HOUR_OF_DAY, hour)
                    selected.set(Calendar.MINUTE, minute)
                    selected.set(Calendar.SECOND, 0)
                    if (selected.timeInMillis > System.currentTimeMillis()) {
                        onSelected("$minute $hour $day ${month + 1} *")
                    }
                },
                selected.get(Calendar.HOUR_OF_DAY),
                selected.get(Calendar.MINUTE),
                true,
            ).show()
        },
        selected.get(Calendar.YEAR),
        selected.get(Calendar.MONTH),
        selected.get(Calendar.DAY_OF_MONTH),
    ).apply {
        datePicker.minDate = System.currentTimeMillis()
        datePicker.maxDate = System.currentTimeMillis() + 366L * 24L * 60L * 60L * 1000L
        show()
    }
}

private val CRON_TIME_FORMATTER = DateTimeFormatter.ofPattern("MM-dd HH:mm")

private fun formatEpochMs(ms: Long): String =
    Instant.ofEpochMilli(ms)
        .atZone(ZoneId.systemDefault())
        .format(CRON_TIME_FORMATTER)

private fun runStatusLabel(status: CronRunStatus): String = when (status) {
    CronRunStatus.Queued -> "已排队"
    CronRunStatus.Running -> "运行中"
    CronRunStatus.Succeeded -> "成功"
    CronRunStatus.Failed -> "失败"
    CronRunStatus.TimedOut -> "超时"
    CronRunStatus.Cancelled -> "已取消"
    CronRunStatus.Skipped -> "已跳过"
}
