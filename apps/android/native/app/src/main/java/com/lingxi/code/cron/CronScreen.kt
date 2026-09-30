package com.lingxi.code.cron

import android.app.DatePickerDialog
import android.app.TimePickerDialog
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
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
    engineSource: com.lingxi.code.conversation.ConversationSource? = null,
    projectStore: com.lingxi.code.project.ProjectStore? = null,
    onOpenSession: (CronRunRecord) -> Unit = {},
    vm: CronManagementViewModel = viewModel(),
) {
    val state by vm.state.collectAsState()
    val modelState = engineSource?.modelState?.collectAsState()?.value
    val projects = projectStore?.state?.collectAsState()?.value
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    var query by rememberSaveable { mutableStateOf("") }
    var filter by rememberSaveable { mutableStateOf("all") }
    var detail by rememberSaveable(initialTaskKey) { mutableStateOf(initialTaskKey != null) }
    var prompt by rememberSaveable { mutableStateOf("") }
    var cronExpr by rememberSaveable { mutableStateOf("0 9 * * *") }
    var recurring by rememberSaveable { mutableStateOf(true) }
    var selectedScopeId by rememberSaveable { mutableStateOf(GLOBAL_CRON_SCOPE_ID) }
    var editingTaskId by rememberSaveable { mutableStateOf<String?>(null) }
    var metadata by rememberSaveable { mutableStateOf(CronAutomation.defaults("").json) }
    val automation = CronAutomation(metadata)
    var dirty by rememberSaveable { mutableStateOf(false) }
    var discardAction by remember { mutableStateOf<(() -> Unit)?>(null) }
    var selectedRunId by rememberSaveable(initialRunId) { mutableStateOf(initialRunId) }
    var formError by remember { mutableStateOf<String?>(null) }
    var submitting by remember { mutableStateOf(false) }
    var appliedInitialTask by rememberSaveable(initialTaskKey) { mutableStateOf(false) }
    fun change(value: CronAutomation) { metadata = value.json; dirty = true }
    fun navigate(action: () -> Unit) { if (dirty) discardAction = action else action() }
    fun select(task: AndroidCronTask) {
        editingTaskId = task.task.id
        selectedScopeId = task.scope.scopeId
        prompt = task.task.prompt
        cronExpr = task.task.cron
        recurring = task.task.recurring
        metadata = CronAutomation.from(task.task).json
        detail = true; dirty = false; formError = null
    }
    fun create() {
        editingTaskId = null; selectedScopeId = GLOBAL_CRON_SCOPE_ID
        prompt = ""; cronExpr = "0 9 * * *"; recurring = true
        metadata = CronAutomation.defaults(modelState?.active.orEmpty()).json
        detail = true; dirty = false; formError = null
    }
    DisposableEffect(lifecycleOwner, vm) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) { vm.refresh(); vm.reconcile("ui-resume") }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    LaunchedEffect(initialTaskKey, state.tasks) {
        if (!appliedInitialTask && initialTaskKey != null) {
            state.tasks.firstOrNull { "${it.scope.scopeId}:${it.task.id}" == initialTaskKey }?.let {
                select(it); appliedInitialTask = true
            }
        }
    }
    discardAction?.let { action ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { discardAction = null },
            title = { Text("Discard unsaved changes?") },
            text = { Text("Your saved task will keep its current settings.") },
            confirmButton = { Button(onClick = { discardAction = null; dirty = false; action() }) { Text("Discard") } },
            dismissButton = { OutlinedButton(onClick = { discardAction = null }) { Text("Keep editing") } },
        )
    }
    // The settings host renders this page inside a scrolling Box, which
    // STACKS its children — without an explicit Column the run history,
    // divider, banners and notices draw on top of the task list.
    Column(verticalArrangement = Arrangement.spacedBy(14.dp), modifier = Modifier.fillMaxWidth()) {
        androidx.activity.compose.BackHandler(enabled = detail) { navigate { detail = false } }
        androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxWidth()) {
            val wide = maxWidth >= 760.dp
            Row(horizontalArrangement = Arrangement.spacedBy(24.dp), modifier = Modifier.fillMaxWidth()) {
                if (!detail || wide) Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.horizontalScroll(rememberScrollState())) {
                        listOf("all", "active", "paused", "completed").forEach { item ->
                            FilterChip(selected = filter == item, onClick = { filter = item }, label = { Text(item.replaceFirstChar { it.uppercase() }) })
                        }
                    }
                    OutlinedTextField(query, { query = it }, label = { Text("Search scheduled tasks") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    Button(onClick = { navigate { create() } }) { Text("Create") }
                    if (state.loading) CircularProgressIndicator()
                    state.tasks.filter { task ->
                        (filter == "all" || CronAutomation.from(task.task).status == filter) &&
                            (query.isBlank() || "${CronAutomation.from(task.task).name} ${task.task.prompt} ${task.scope.projectName}".contains(query, ignoreCase = true))
                    }.forEach { task ->
                        CronTaskCard(task,
                            onEdit = { navigate { select(task) } },
                            onRunNow = { vm.runNow(task.scope.scopeId, task.task.id) { formError = it } },
                            onDelete = { vm.delete(task.scope.scopeId, task.task.id) { formError = it } },
                        )
                    }
                    if (state.tasks.isEmpty() && !state.loading) Text(stringResource(R.string.cron_no_tasks_yet))
                    SchedulingSummary(state)
                }
                if (detail) Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlinedButton(onClick = { navigate { detail = false } }) { Text("Back") }
                        Text(if (editingTaskId == null) "New scheduled task" else "Task details", modifier = Modifier.padding(top = 12.dp), fontWeight = FontWeight.SemiBold)
                    }
                    CronChoice("Status", automation.status, listOf("active", "paused", "completed").map { it to it.replaceFirstChar(Char::uppercase) }) {
                        change(automation.change("status", it).change("statusReason", null))
                    }
                    automation.statusReason?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                    OutlinedTextField(automation.name, { change(automation.change("name", it)) }, label = { Text("Name") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    OutlinedTextField(prompt, { prompt = it; dirty = true }, label = { Text(stringResource(R.string.cron_prompt_field_label)) }, minLines = 3, modifier = Modifier.fillMaxWidth())
                    Text("Details", color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 13.sp)
                    androidx.compose.material3.OutlinedCard(
                        modifier = Modifier.fillMaxWidth(),
                        shape = androidx.compose.foundation.shape.RoundedCornerShape(16.dp),
                        colors = CardDefaults.outlinedCardColors(containerColor = MaterialTheme.colorScheme.surface),
                    ) {
                    CronChoice("Runs in", automation.runMode, listOf("new_session" to "New chat each run", "selected_session" to "Selected chat", "task_session" to "Task chat")) {
                        change(automation.change("runMode", it).change("targetSessionId", null).change("ownedSessionId", null))
                    }
                    CronChoice("Project", selectedScopeId, state.scopes.map { it.scopeId to if (it.projectId == null) "None" else it.projectName }, enabled = editingTaskId == null && automation.runMode != "selected_session") {
                        selectedScopeId = it; dirty = true
                    }
                    if (automation.runMode == "selected_session") {
                        val sessions = buildList {
                            state.generatedSessions.filter { it.projectId == null && it.sessionId != null }.distinctBy { it.sessionId }
                                .forEach { add(Triple(GLOBAL_CRON_SCOPE_ID, it.sessionId!!, it.prompt.take(120))) }
                            projects?.projects?.forEach { project -> project.sessions.filterNot { it.isArchived }.forEach { add(Triple(project.record.id, it.sessionId, "${project.record.name} / ${it.title}")) } }
                        }.filter { editingTaskId == null || it.first == selectedScopeId }
                        CronChoice("Chat", automation.targetSessionId, sessions.map { it.second to it.third }) { id ->
                            sessions.firstOrNull { it.second == id }?.let { selectedScopeId = it.first }
                            change(automation.change("targetSessionId", id))
                        }
                    }
                    if (editingTaskId != null) OutlinedButton(onClick = {
                        editingTaskId = null; metadata = automation.copied().json; dirty = true
                    }) { Text("Copy to project…") }
                    CronChoice("Model", automation.model, modelState?.available.orEmpty().map { it to (modelState?.details?.get(it)?.displayName ?: it) }) { model ->
                        change(automation.change("model", model).withReasoning(com.lingxi.code.bindings.client.ReasoningSelectionDto.Automatic))
                    }
                    val reasoningOptions = modelState?.details?.get(automation.model)?.reasoningOptions.orEmpty().filter { it.persistable }
                    val choices = listOf("{\"type\":\"automatic\"}" to "Automatic") + reasoningOptions.map { reasoningJson(it.selection).toString() to it.label }
                    CronChoice("Reasoning", automation.reasoningJson, choices.distinctBy { it.first }, fallbackLabel = automation.reasoningLabel) { change(automation.change("reasoning", org.json.JSONObject(it))) }
                    CronChoice("Notifications", automation.notificationPolicy, listOf("all" to "All runs", "failed" to "Failures only", "none" to "Off"), divider = false) { change(automation.change("notificationPolicy", it)) }
                    }
                    Text("Frequency", fontWeight = FontWeight.SemiBold)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.horizontalScroll(rememberScrollState())) {
                        schedulePresets().forEach { preset -> AssistChip(onClick = { cronExpr = preset.cron; recurring = true; dirty = true }, label = { Text(preset.label) }) }
                        AssistChip(onClick = { openOneShotDateTimePicker(context) { cronExpr = it; recurring = false; dirty = true } }, label = { Text(stringResource(R.string.cron_one_time_datetime_button)) })
                    }
                    OutlinedTextField(cronExpr, { cronExpr = it; dirty = true }, label = { Text(stringResource(R.string.cron_advanced_expression_label)) }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Switch(recurring, { recurring = it; dirty = true })
                        Text(if (recurring) "Repeat on schedule" else "Run once")
                    }
                    Text("Time zone: ${ZoneId.systemDefault().id}", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    val validation = stringResource(R.string.cron_form_validation_error)
                    Button(enabled = !submitting, onClick = {
                        if (prompt.isBlank() || cronExpr.isBlank() || automation.model.isBlank() || (automation.runMode == "selected_session" && automation.targetSessionId.isBlank())) {
                            formError = validation; return@Button
                        }
                        submitting = true
                        val result: (String?) -> Unit = { error ->
                            submitting = false; formError = error
                            if (error == null) { dirty = false; detail = false }
                        }
                        editingTaskId?.let { vm.update(selectedScopeId, it, cronExpr, prompt, recurring, automation, result) }
                            ?: vm.create(selectedScopeId, cronExpr, prompt, recurring, automation, result)
                    }) { Text(if (submitting) "Saving…" else "Save changes") }
                }
            }
        }
        formError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        state.errorMessage?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        if (!state.exactAlarmAllowed) ExactAlarmBanner()
        HorizontalDivider()
        Text(stringResource(R.string.cron_run_history_section), fontWeight = FontWeight.SemiBold)
        state.history.filter { !detail || editingTaskId == null || (it.scopeId == selectedScopeId && it.taskId == editingTaskId) }.take(50).forEach { run ->
            RunHistoryCard(run, selectedRunId == run.runId, { selectedRunId = if (selectedRunId == run.runId) null else run.runId })
            if (selectedRunId == run.runId && run.sessionId != null) {
                OutlinedButton(onClick = { onOpenSession(run) }) { Text("Open chat") }
            }
        }
        Text(stringResource(R.string.cron_force_stop_notice), color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 11.sp)
    }
}

@Composable
private fun CronChoice(
    label: String,
    value: String,
    choices: List<Pair<String, String>>,
    enabled: Boolean = true,
    divider: Boolean = true,
    fallbackLabel: String? = null,
    onChange: (String) -> Unit,
) {
    var expanded by remember { mutableStateOf(false) }
    val interactive = enabled && choices.isNotEmpty()
    androidx.compose.foundation.layout.Box(Modifier.fillMaxWidth()) {
        Column {
            Row(
                Modifier.fillMaxWidth().heightIn(min = 52.dp)
                    .clickable(enabled = interactive, role = androidx.compose.ui.semantics.Role.Button) { expanded = true }
                    .padding(horizontal = 16.dp, vertical = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(label, modifier = Modifier.padding(end = 16.dp), fontSize = 14.sp)
                Text(choices.firstOrNull { it.first == value }?.second ?: fallbackLabel ?: value.ifBlank { "Select…" },
                    maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f), fontSize = 14.sp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = androidx.compose.ui.text.style.TextAlign.End)
                if (interactive) Text("⌄", color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(start = 10.dp))
            }
            if (divider) HorizontalDivider(modifier = Modifier.padding(horizontal = 16.dp), color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.45f))
        }
        androidx.compose.material3.DropdownMenu(expanded, { expanded = false }) {
            choices.forEach { (id, title) -> androidx.compose.material3.DropdownMenuItem(text = { Text(title) }, onClick = { expanded = false; onChange(id) }) }
        }
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
        modifier = Modifier.fillMaxWidth().clickable(onClick = onEdit),
        colors = CardDefaults.cardColors(),
    ) {
        Column(
            modifier = Modifier.padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = CronAutomation.from(task.task).name.ifBlank { task.task.prompt.lineSequence().firstOrNull().orEmpty() }
                        .ifBlank { task.task.id },
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 14.sp,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                IconButton(onClick = onRunNow, enabled = task.activeRun == null && task.task.isActive()) {
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
                task.scope.projectName,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 12.sp,
            )
            Text(
                task.task.cron,
                fontFamily = FontFamily.Monospace,
                fontSize = 11.sp,
                color = MaterialTheme.colorScheme.primary,
            )
            val automation = CronAutomation.from(task.task)
            Text(automation.status.replaceFirstChar { it.uppercase() }, color = MaterialTheme.colorScheme.primary)
            automation.statusReason?.let { Text(it, color = MaterialTheme.colorScheme.error) }
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
                run.model?.let { Text("Model: $it", fontSize = 11.sp) }
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
    CronRunStatus.Interrupted -> "Interrupted"
}
