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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
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
        Blurb("Mobile Linux 使用受管 Alpine rootfs 与 Android PRoot 后端。选择该模式后，运行时错误会直接显示，不会静默回退到 Legacy。")

        SettingsSection(label = "后端选择") {
            RadioList(
                options = listOf(
                    RadioOption(LinuxRuntimeMode.Legacy.name, "Legacy", "继续使用现有 Minijail + shell 执行层"),
                    RadioOption(LinuxRuntimeMode.MobileLinux.name, "Mobile Linux", "使用 Android PRoot + Alpine 运行时"),
                ),
                selected = runtime.selectedMode.name,
                onSelect = { next ->
                    val mode = LinuxRuntimeMode.valueOf(next)
                    store.setLinuxRuntimeMode(mode)
                },
            )
        }

        SettingsSection(label = "状态") {
            SettingsRow(
                icon = LXIconName.Workflow,
                label = "当前后端",
                value = runtime.backend,
                chevron = false,
            )
            SettingsRow(
                label = "Rootfs",
                sub = runtime.detail,
                value = linuxRuntimeStateLabel(runtime.rootfsState),
                chevron = false,
            )
            SettingsRow(
                label = "版本",
                value = runtime.version ?: "未安装",
                chevron = false,
            )
            SettingsRow(
                label = "体积",
                value = runtime.installedSizeBytes?.let(::formatBytes) ?: "—",
                chevron = false,
            )
            SettingsRow(
                label = "托管目录",
                sub = runtime.managedRoot ?: "未创建",
                value = runtime.badge,
                chevron = false,
                isLast = true,
            )
        }

        SettingsSection(
            label = "维护",
            footer = "安装、修复和重置只作用于受管 rootfs；外部工作区、会话与密钥不会被删除。",
        ) {
            ActionRow(
                title = "安装 rootfs",
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
                title = "启动 runtime",
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
                title = "关闭 runtime",
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
                title = "刷新状态",
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
                title = "校验 rootfs",
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
                title = "修复 rootfs",
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
                title = "重置 rootfs",
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

        SettingsSection(label = "工作区与终端") {
            SettingsRow(
                label = "终端入口",
                sub = when (runtime.terminal.status) {
                    LinuxRuntimeTerminalStatus.Disabled -> "当前构建未提供可用 PTY 运行时"
                    LinuxRuntimeTerminalStatus.Idle -> "可创建 PTY 终端会话"
                    LinuxRuntimeTerminalStatus.Starting -> "终端会话启动中"
                    LinuxRuntimeTerminalStatus.Active -> "存在运行中的 PTY 会话"
                },
                value = if (runtime.canOpenTerminal) "可用" else "已禁用",
                chevron = false,
            )
            SettingsRow(
                label = "命令草稿",
                sub = runtime.terminal.transcript.firstOrNull() ?: "暂无输出",
                value = runtime.terminal.commandDraft,
                chevron = false,
            )
            ActionRow(
                title = "新建终端",
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
                label = "外部目录挂载",
                sub = if (runtime.mounts.isEmpty()) {
                    runtime.mountDraft.pickerSummary
                } else {
                    runtime.mounts.joinToString(limit = 2, truncated = "…") {
                        "${it.guestPath} ${it.access.label}"
                    }
                },
                value = "${runtime.mounts.size} 项",
                chevron = false,
            )
            SettingsRow(
                label = "目录授权模式",
                sub = runtime.mountDraft.pickerSummary,
                value = runtime.mountDraft.access.label,
                chevron = false,
            )
            SettingsRow(
                label = "执行任务",
                sub = runtime.tasks.firstOrNull()?.state ?: "当前没有可停止的 Mobile Linux 任务",
                value = if (runtime.tasks.isEmpty()) "空闲" else "${runtime.tasks.size} 个",
                chevron = false,
                isLast = true,
            )
        }

        SettingsSection(label = "后台任务与挂载") {
            runtime.tasks.firstOrNull { it.stoppable }?.let { task ->
                ActionRow(
                    title = "停止 ${task.label}",
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
                title = "刷新任务",
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
                title = "刷新挂载视图",
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

        SettingsSection(label = "安全说明") {
            SettingsRow(
                icon = LXIconName.Pin,
                label = "执行边界",
                sub = "PRoot 不是安全边界；真实边界仍是 Android App 沙箱与外层 Minijail 策略",
                chevron = false,
            )
            SettingsRow(
                icon = LXIconName.X,
                label = "停止当前任务",
                sub = "未授权或 runtime 未链接时必须 fail-closed；不会静默回退到 legacy shell",
                value = "未启用",
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
            busy -> "执行中…"
            enabled -> action.label
            else -> "当前构建未开放该操作"
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
                    Text(action.label, fontSize = 12.sp)
                }
                else -> Text(
                    text = "未启用",
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
