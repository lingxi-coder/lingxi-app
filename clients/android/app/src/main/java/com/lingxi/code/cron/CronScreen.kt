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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.viewmodel.compose.viewModel
import com.lingxi.code.R
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Calendar

private data class SchedulePreset(val label: String, val cron: String)

@Composable
private fun schedulePresets(): List<SchedulePreset> = listOf(
    SchedulePreset(stringResource(R.string.cron_every_15_minutes), "*/15 * * * *"),
    SchedulePreset(stringResource(R.string.cron_preset_every_hour), "0 * * * *"),
    SchedulePreset(stringResource(R.string.cron_daily_9am), "0 9 * * *"),
    SchedulePreset(stringResource(R.string.cron_weekdays_9am), "0 9 * * 1-5"),
    SchedulePreset(stringResource(R.string.cron_preset_every_monday_9am), "0 9 * * 1"),
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
        val validationErrorMessage = stringResource(R.string.cron_form_validation_error)

        SchedulingSummary(state)
        initialRunId?.let { runId ->
            state.history.firstOrNull { it.runId == runId }?.let { run ->
                Text(
                    stringResource(R.string.cron_notification_run_result_label),
                    fontWeight = FontWeight.SemiBold,
                )
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
            if (editingTaskId == null) {
                stringResource(R.string.cron_new_task_button)
            } else {
                stringResource(R.string.cron_edit_task_button)
            },
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            stringResource(R.string.cron_scope_hint),
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
            label = { Text(stringResource(R.string.cron_prompt_field_label)) },
            minLines = 2,
            modifier = Modifier.fillMaxWidth(),
        )
        Row(
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
        ) {
            schedulePresets().forEach { preset ->
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
                label = { Text(stringResource(R.string.cron_one_time_datetime_button)) },
            )
        }
        OutlinedTextField(
            value = cronExpr,
            onValueChange = { cronExpr = it },
            label = { Text(stringResource(R.string.cron_advanced_expression_label)) },
            placeholder = { Text("0 9 * * *") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(checked = recurring, onCheckedChange = { recurring = it })
            Text(
                if (recurring) {
                    stringResource(R.string.cron_recurring_interval_note)
                } else {
                    stringResource(R.string.cron_run_once_toggle)
                },
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
                        formError = validationErrorMessage
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
                Text(
                    if (editingTaskId == null) {
                        stringResource(R.string.cron_create_and_schedule_button)
                    } else {
                        stringResource(R.string.voice_save_button)
                    },
                )
            }
            if (editingTaskId != null) {
                OutlinedButton(onClick = ::resetForm) {
                    Text(stringResource(R.string.cron_cancel_edit_button))
                }
            }
        }

        HorizontalDivider()
        Text(
            stringResource(R.string.cron_task_count_header_fmt, state.tasks.size),
            fontWeight = FontWeight.SemiBold,
        )
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
                stringResource(R.string.cron_no_tasks_yet),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
            )
        }

        HorizontalDivider()
        Text(stringResource(R.string.cron_run_history_section), fontWeight = FontWeight.SemiBold)
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
                stringResource(R.string.cron_no_run_history_android),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
            )
        }
        Text(
            stringResource(R.string.cron_force_stop_notice),
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
                if (state.exactAlarmAllowed) {
                    stringResource(R.string.cron_exact_alarm_granted)
                } else {
                    stringResource(R.string.cron_fallback_mode_label)
                },
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                state.nextScheduledAtMs?.let {
                    stringResource(R.string.cron_next_wake_fmt, formatEpochMs(it))
                } ?: stringResource(R.string.cron_no_schedulable_tasks),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 12.sp,
            )
            Text(
                stringResource(
                    R.string.cron_active_queue_summary_fmt,
                    state.activeWorkCount,
                    if (state.networkAvailable) {
                        stringResource(R.string.cron_network_connected)
                    } else {
                        stringResource(R.string.cron_network_waiting)
                    },
                ),
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
                    Icon(
                        Icons.Default.PlayArrow,
                        contentDescription = stringResource(R.string.cron_run_now_button),
                    )
                }
                IconButton(onClick = onEdit, enabled = task.activeRun == null) {
                    Icon(
                        Icons.Default.Edit,
                        contentDescription = stringResource(R.string.settings_title_edit),
                    )
                }
                IconButton(onClick = onDelete, enabled = task.activeRun == null) {
                    Icon(
                        Icons.Default.Delete,
                        contentDescription = stringResource(R.string.common_delete),
                    )
                }
            }
            Text(
                task.task.human + if (task.task.recurring) {
                    ""
                } else {
                    " · " + stringResource(R.string.cron_once_only)
                },
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
                    task.unsupportedReason ?: stringResource(R.string.cron_task_unsupported_default)
                task.activeRun != null -> runStatusLabel(task.activeRun.status)
                task.lastRun != null ->
                    stringResource(R.string.cron_last_run_status_fmt, runStatusLabel(task.lastRun.status))
                task.schedulingMode == CronSchedulingMode.FifteenMinuteFallback ->
                    stringResource(R.string.cron_fifteen_minute_patrol_mode)
                else -> stringResource(R.string.cron_exact_alarm_mode)
            }
            Text(
                buildString {
                    append(stateText)
                    task.task.nextFireMs?.toLong()?.let {
                        append(stringResource(R.string.cron_next_fire_suffix_fmt, formatEpochMs(it)))
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
                stringResource(
                    R.string.cron_run_summary_fmt,
                    runStatusLabel(run.status),
                    formatEpochMs(run.scheduledAtMs),
                    maxOf(1, run.attempt),
                ),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 11.sp,
            )
            if (expanded) {
                Spacer(Modifier.height(6.dp))
                Text(
                    run.resultText ?: run.errorMessage ?: stringResource(R.string.cron_no_output),
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
                stringResource(R.string.cron_exact_alarm_banner_body),
                fontSize = 13.sp,
            )
            Button(
                onClick = {
                    openExactAlarmSettings(context)
                },
            ) {
                Text(stringResource(R.string.cron_open_alarm_settings_button))
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

@Composable
private fun runStatusLabel(status: CronRunStatus): String = when (status) {
    CronRunStatus.Queued -> stringResource(R.string.cron_status_queued)
    CronRunStatus.Running -> stringResource(R.string.chat_status_running)
    CronRunStatus.Succeeded -> stringResource(R.string.cron_status_succeeded)
    CronRunStatus.Failed -> stringResource(R.string.chat_status_failed)
    CronRunStatus.TimedOut -> stringResource(R.string.chat_status_timed_out)
    CronRunStatus.Cancelled -> stringResource(R.string.chat_status_cancelled)
    CronRunStatus.Skipped -> stringResource(R.string.cron_status_skipped)
}
