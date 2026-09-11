package com.lingxi.code.settings

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.components.LXIconName
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

@Composable
fun LinuxRuntimePage(
    state: SettingsUiState,
    store: SettingsStore,
    modifier: Modifier = Modifier,
    onOpenTerminal: ((LinuxRuntimeTerminalLaunchRequest) -> Unit)? = null,
) {
    val context = LocalContext.current
    val resources = LocalResources.current
    val scope = rememberCoroutineScope()
    val runtime = state.linuxRuntime

    LaunchedEffect(runtime.selectedMode) {
        runLinuxRuntimeAction(
            context = context,
            store = store,
            action = LinuxRuntimeAction.Refresh,
            mode = runtime.selectedMode,
        )
    }

    Column(
        modifier = modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(0.dp),
    ) {
        Blurb(stringResource(R.string.settings_linux_blurb_android))

        SettingsSection(label = stringResource(R.string.settings_linux_section_backend)) {
            RadioList(
                options = listOf(
                    RadioOption(
                        LinuxRuntimeMode.Legacy.name,
                        "Legacy",
                        stringResource(R.string.settings_linux_legacy_sub_android),
                    ),
                    RadioOption(
                        LinuxRuntimeMode.MobileLinux.name,
                        "Mobile Linux",
                        stringResource(R.string.settings_linux_mobile_linux_sub_android),
                    ),
                ),
                selected = runtime.selectedMode.name,
                onSelect = { next ->
                    val mode = LinuxRuntimeMode.valueOf(next)
                    store.setLinuxRuntimeMode(mode)
                },
            )
        }

        SettingsSection(label = stringResource(R.string.settings_linux_section_status)) {
            SettingsRow(
                icon = LXIconName.Workflow,
                label = stringResource(R.string.settings_linux_current_backend),
                value = runtime.backend,
                chevron = false,
            )
            SettingsRow(
                label = "Rootfs",
                sub = runtime.detail ?: stringResource(runtime.detailRes),
                value = stringResource(linuxRuntimeStateLabel(runtime.rootfsState)),
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_linux_version),
                value = runtime.version ?: stringResource(R.string.settings_linux_not_installed),
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_linux_size),
                value = runtime.installedSizeBytes?.let(::formatBytes) ?: "—",
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_linux_managed_dir),
                sub = runtime.managedRoot ?: stringResource(R.string.settings_linux_not_created),
                value = stringResource(runtime.badgeRes),
                chevron = false,
                isLast = true,
            )
        }

        SettingsSection(
            label = stringResource(R.string.settings_linux_section_maintenance),
            footer = stringResource(R.string.settings_linux_maintenance_footer_android),
        ) {
            ActionRow(
                title = stringResource(R.string.settings_linux_action_install),
                action = LinuxRuntimeAction.Install,
                busy = runtime.busyAction == LinuxRuntimeAction.Install,
                enabled = runtime.installAllowed && runtime.busyAction == null,
                onClick = {
                    scope.launch {
                        runLinuxRuntimeAction(context, store, LinuxRuntimeAction.Install, runtime.selectedMode)
                    }
                },
            )
            ActionRow(
                title = stringResource(R.string.settings_linux_action_boot),
                action = LinuxRuntimeAction.Boot,
                busy = runtime.busyAction == LinuxRuntimeAction.Boot,
                enabled = runtime.available && runtime.busyAction == null,
                onClick = {
                    scope.launch {
                        runLinuxRuntimeAction(context, store, LinuxRuntimeAction.Boot, runtime.selectedMode)
                    }
                },
            )
            ActionRow(
                title = stringResource(R.string.settings_linux_action_shutdown),
                action = LinuxRuntimeAction.Shutdown,
                busy = runtime.busyAction == LinuxRuntimeAction.Shutdown,
                enabled = runtime.available && runtime.busyAction == null,
                onClick = {
                    scope.launch {
                        runLinuxRuntimeAction(context, store, LinuxRuntimeAction.Shutdown, runtime.selectedMode)
                    }
                },
            )
            ActionRow(
                title = stringResource(R.string.settings_linux_action_refresh),
                action = LinuxRuntimeAction.Refresh,
                busy = runtime.busyAction == LinuxRuntimeAction.Refresh,
                enabled = runtime.busyAction == null,
                onClick = {
                    scope.launch {
                        runLinuxRuntimeAction(context, store, LinuxRuntimeAction.Refresh, runtime.selectedMode)
                    }
                },
            )
            ActionRow(
                title = stringResource(R.string.settings_linux_action_verify),
                action = LinuxRuntimeAction.Verify,
                busy = runtime.busyAction == LinuxRuntimeAction.Verify,
                enabled = runtime.verifyAllowed && runtime.busyAction == null,
                onClick = {
                    scope.launch {
                        runLinuxRuntimeAction(context, store, LinuxRuntimeAction.Verify, runtime.selectedMode)
                    }
                },
            )
            ActionRow(
                title = stringResource(R.string.settings_linux_action_repair),
                action = LinuxRuntimeAction.Repair,
                busy = runtime.busyAction == LinuxRuntimeAction.Repair,
                enabled = runtime.repairAllowed && runtime.busyAction == null,
                onClick = {
                    scope.launch {
                        runLinuxRuntimeAction(context, store, LinuxRuntimeAction.Repair, runtime.selectedMode)
                    }
                },
            )
            ActionRow(
                title = stringResource(R.string.settings_linux_action_reset),
                action = LinuxRuntimeAction.Reset,
                busy = runtime.busyAction == LinuxRuntimeAction.Reset,
                enabled = runtime.resetAllowed && runtime.busyAction == null,
                onClick = {
                    scope.launch {
                        runLinuxRuntimeAction(context, store, LinuxRuntimeAction.Reset, runtime.selectedMode)
                    }
                },
                isLast = true,
            )
        }

        SettingsSection(label = stringResource(R.string.settings_linux_section_workspace)) {
            SettingsRow(
                label = stringResource(R.string.settings_linux_terminal_entry),
                sub = when (runtime.terminal.status) {
                    LinuxRuntimeTerminalStatus.Disabled -> stringResource(R.string.settings_linux_pty_unavailable)
                    LinuxRuntimeTerminalStatus.Idle -> stringResource(R.string.settings_linux_pty_available)
                    LinuxRuntimeTerminalStatus.Starting -> stringResource(R.string.settings_linux_terminal_starting)
                    LinuxRuntimeTerminalStatus.Active -> stringResource(R.string.settings_linux_terminal_active)
                },
                value = if (runtime.canOpenTerminal) {
                    stringResource(R.string.settings_status_available)
                } else {
                    stringResource(R.string.settings_status_disabled)
                },
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_linux_command_draft),
                sub = runtime.terminal.transcript.firstOrNull()
                    ?: stringResource(
                        if (runtime.terminal.status == LinuxRuntimeTerminalStatus.Disabled) {
                            R.string.settings_linux_terminal_not_linked
                        } else {
                            R.string.settings_linux_terminal_ready
                        },
                    ),
                value = runtime.terminal.commandDraft,
                chevron = false,
            )
            ActionRow(
                title = stringResource(R.string.settings_linux_open_terminal),
                action = LinuxRuntimeAction.OpenTerminal,
                busy = runtime.busyAction == LinuxRuntimeAction.OpenTerminal,
                enabled = runtime.canOpenTerminal && onOpenTerminal != null && runtime.busyAction == null,
                onClick = {
                    onOpenTerminal?.invoke(
                        LinuxRuntimeTerminalLaunchRequest(
                            sessionId = runtime.terminal.sessionId ?: java.util.UUID.randomUUID().toString(),
                            initCommand = runtime.terminal.commandDraft,
                        ),
                    )
                },
            )
            SettingsRow(
                label = stringResource(R.string.settings_linux_external_mounts),
                sub = if (runtime.mounts.isEmpty()) {
                    stringResource(runtime.mountDraft.pickerSummaryRes)
                } else {
                    runtime.mounts.joinToString(limit = 2, truncated = "…") {
                        "${it.guestPath} ${resources.getString(it.access.labelRes)}"
                    }
                },
                value = stringResource(R.string.settings_linux_mounts_count_fmt, runtime.mounts.size),
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_linux_mount_auth_mode),
                sub = stringResource(runtime.mountDraft.pickerSummaryRes),
                value = stringResource(runtime.mountDraft.access.labelRes),
                chevron = false,
            )
            SettingsRow(
                label = stringResource(R.string.settings_linux_task_execution),
                sub = runtime.tasks.firstOrNull()?.let { task ->
                    stringResource(task.stateRes) + (task.stateDetail?.let { " · $it" } ?: "")
                } ?: stringResource(R.string.settings_linux_no_stoppable_tasks),
                value = if (runtime.tasks.isEmpty()) {
                    stringResource(R.string.settings_linux_idle)
                } else {
                    stringResource(R.string.settings_linux_tasks_count_fmt, runtime.tasks.size)
                },
                chevron = false,
                isLast = true,
            )
        }

        SettingsSection(label = stringResource(R.string.settings_linux_section_background)) {
            runtime.tasks.firstOrNull { it.stoppable }?.let { task ->
                ActionRow(
                    title = stringResource(R.string.settings_linux_stop_task_fmt, task.label),
                    action = LinuxRuntimeAction.StopTask,
                    busy = runtime.busyAction == LinuxRuntimeAction.StopTask,
                    enabled = runtime.busyAction == null,
                    onClick = {
                        scope.launch {
                            stopLinuxRuntimeTask(context, store, runtime.selectedMode, task.id)
                        }
                    },
                )
            }
            ActionRow(
                title = stringResource(R.string.settings_linux_refresh_tasks_short),
                action = LinuxRuntimeAction.RefreshTasks,
                busy = runtime.busyAction == LinuxRuntimeAction.RefreshTasks,
                enabled = runtime.canInspectTasks && runtime.busyAction == null,
                onClick = {
                    scope.launch {
                        runLinuxRuntimeAction(context, store, LinuxRuntimeAction.RefreshTasks, runtime.selectedMode)
                    }
                },
            )
            ActionRow(
                title = stringResource(R.string.settings_linux_refresh_mounts_view),
                action = LinuxRuntimeAction.RefreshMounts,
                busy = runtime.busyAction == LinuxRuntimeAction.RefreshMounts,
                enabled = runtime.canManageMounts && runtime.busyAction == null,
                onClick = {
                    scope.launch {
                        runLinuxRuntimeAction(context, store, LinuxRuntimeAction.RefreshMounts, runtime.selectedMode)
                    }
                },
                isLast = true,
            )
        }

        SettingsSection(label = stringResource(R.string.settings_linux_section_safety)) {
            SettingsRow(
                icon = LXIconName.Pin,
                label = stringResource(R.string.settings_linux_execution_boundary),
                sub = stringResource(R.string.settings_linux_execution_boundary_sub_android),
                chevron = false,
            )
            SettingsRow(
                icon = LXIconName.X,
                label = stringResource(R.string.settings_linux_stop_current_task),
                sub = stringResource(R.string.settings_linux_stop_task_sub_android),
                value = stringResource(R.string.settings_status_not_enabled),
                chevron = false,
                isLast = true,
            )
        }
    }
}

@Composable
private fun ActionRow(
    title: String,
    action: LinuxRuntimeAction,
    busy: Boolean,
    enabled: Boolean,
    onClick: () -> Unit,
    isLast: Boolean = false,
) {
    val t = LingXiTheme.palette
    SettingsRow(
        label = title,
        sub = when {
            busy -> stringResource(R.string.settings_linux_running)
            enabled -> stringResource(action.labelRes)
            else -> stringResource(R.string.settings_linux_action_unavailable)
        },
        chevron = false,
        isLast = isLast,
        trailing = {
            when {
                busy -> CircularProgressIndicator(
                    modifier = Modifier.padding(start = 8.dp),
                    strokeWidth = 2.dp,
                )
                enabled -> OutlinedButton(
                    onClick = onClick,
                    contentPadding = PaddingValues(horizontal = 12.dp, vertical = 6.dp),
                ) {
                    Text(stringResource(action.labelRes), fontSize = 12.sp)
                }
                else -> Text(
                    text = stringResource(R.string.settings_status_not_enabled),
                    color = t.text4,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.Medium,
                )
            }
        },
    )
}

private suspend fun runLinuxRuntimeAction(
    context: Context,
    store: SettingsStore,
    action: LinuxRuntimeAction,
    mode: LinuxRuntimeMode,
) {
    if (!store.tryBeginLinuxRuntimeAction(action, mode)) return
    try {
        val snapshot = when (action) {
            LinuxRuntimeAction.Refresh -> LinuxRuntimeBridge.load(context, mode)
            LinuxRuntimeAction.Install -> LinuxRuntimeBridge.install(context, mode)
            LinuxRuntimeAction.Verify -> LinuxRuntimeBridge.verify(context, mode)
            LinuxRuntimeAction.Repair -> LinuxRuntimeBridge.repair(context, mode)
            LinuxRuntimeAction.Reset -> LinuxRuntimeBridge.reset(context, mode)
            LinuxRuntimeAction.Boot -> LinuxRuntimeBridge.boot(context, mode)
            LinuxRuntimeAction.Shutdown -> LinuxRuntimeBridge.shutdown(context, mode)
            LinuxRuntimeAction.RefreshTasks -> LinuxRuntimeBridge.refreshTasks(context, mode)
            LinuxRuntimeAction.RefreshMounts -> LinuxRuntimeBridge.load(context, mode)
            LinuxRuntimeAction.OpenTerminal -> LinuxRuntimeBridge.load(context, mode)
            LinuxRuntimeAction.StopTask -> LinuxRuntimeBridge.refreshTasks(context, mode)
        }
        store.completeLinuxRuntimeAction(action, mode, snapshot)
    } catch (cancelled: CancellationException) {
        store.cancelLinuxRuntimeAction(action, mode)
        throw cancelled
    } catch (error: Exception) {
        store.failLinuxRuntimeAction(
            action = action,
            mode = mode,
            message = error.message ?: error::class.java.simpleName,
        )
    }
}

private suspend fun stopLinuxRuntimeTask(
    context: Context,
    store: SettingsStore,
    mode: LinuxRuntimeMode,
    taskId: String,
) {
    val action = LinuxRuntimeAction.StopTask
    if (!store.tryBeginLinuxRuntimeAction(action, mode)) return
    try {
        LinuxRuntimeBridge.killProcess(
            context = context,
            mode = mode,
            handle = com.lingxi.code.bindings.MobileLinuxProcessHandleFfi(taskId),
        )
        store.completeLinuxRuntimeAction(
            action,
            mode,
            LinuxRuntimeBridge.refreshTasks(context, mode).copy(lastAction = action),
        )
    } catch (cancelled: CancellationException) {
        store.cancelLinuxRuntimeAction(action, mode)
        throw cancelled
    } catch (error: Exception) {
        store.failLinuxRuntimeAction(
            action = action,
            mode = mode,
            message = error.message ?: error::class.java.simpleName,
        )
    }
}

private fun formatBytes(bytes: Long): String {
    val kb = 1024L
    val mb = kb * 1024
    val gb = mb * 1024
    return when {
        bytes >= gb -> String.format("%.1f GB", bytes.toDouble() / gb.toDouble())
        bytes >= mb -> String.format("%.1f MB", bytes.toDouble() / mb.toDouble())
        bytes >= kb -> String.format("%.1f KB", bytes.toDouble() / kb.toDouble())
        else -> "$bytes B"
    }
}
