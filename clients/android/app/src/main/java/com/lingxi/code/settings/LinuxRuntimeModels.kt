package com.lingxi.code.settings

import com.lingxi.code.bindings.AndroidMobileLinuxConfigFfi
import com.lingxi.code.bindings.MobileLinuxCapabilityFfi
import com.lingxi.code.bindings.MobileLinuxRootfsStateFfi
import com.lingxi.code.bindings.MobileLinuxRuntimeModeFfi
import com.lingxi.code.bindings.MobileLinuxStatusFfi

enum class LinuxRuntimeMode(val title: String) {
    Legacy("Legacy"),
    MobileLinux("Mobile Linux");

    fun toFfi(): MobileLinuxRuntimeModeFfi =
        when (this) {
            Legacy -> MobileLinuxRuntimeModeFfi.LEGACY
            MobileLinux -> MobileLinuxRuntimeModeFfi.MOBILE_LINUX
        }

    companion object {
        fun fromFfi(value: MobileLinuxRuntimeModeFfi): LinuxRuntimeMode =
            when (value) {
                MobileLinuxRuntimeModeFfi.LEGACY -> Legacy
                MobileLinuxRuntimeModeFfi.MOBILE_LINUX -> MobileLinux
            }
    }
}

enum class LinuxRuntimeAction(val label: String) {
    Refresh("刷新"),
    Verify("校验"),
    Repair("修复"),
    Reset("重置"),
    Boot("启动"),
    Shutdown("关闭"),
    OpenTerminal("新建终端"),
    RefreshTasks("刷新任务"),
    RefreshMounts("刷新挂载"),
}

enum class LinuxRuntimeTerminalStatus { Disabled, Idle, Starting, Active }

enum class LinuxRuntimeMountAccess(val label: String) {
    ReadOnly("只读"),
    ReadWrite("读写"),
}

data class LinuxRuntimeMountUiState(
    val id: String,
    val guestPath: String,
    val summary: String,
    val access: LinuxRuntimeMountAccess,
    val pendingGrant: Boolean = false,
)

data class LinuxRuntimeMountDraft(
    val purposeLabel: String = "外部目录",
    val access: LinuxRuntimeMountAccess = LinuxRuntimeMountAccess.ReadOnly,
    val pickerSummary: String = "将通过系统目录选择器（SAF）逐目录授权，不暴露任意宿主路径",
)

data class LinuxRuntimeTaskUiState(
    val id: String,
    val label: String,
    val state: String,
    val stoppable: Boolean,
)

data class LinuxRuntimeTerminalUiState(
    val status: LinuxRuntimeTerminalStatus = LinuxRuntimeTerminalStatus.Disabled,
    val sessionId: String? = null,
    val commandDraft: String = "python3 -V",
    val transcript: List<String> = listOf("当前构建未接入可用 PTY 运行时。"),
)

data class LinuxRuntimeUiState(
    val selectedMode: LinuxRuntimeMode = LinuxRuntimeMode.Legacy,
    val backend: String = "android-minijail",
    val rootfsState: MobileLinuxRootfsStateFfi = MobileLinuxRootfsStateFfi.UNSUPPORTED,
    val version: String? = null,
    val managedRoot: String? = null,
    val installedSizeBytes: Long? = null,
    val available: Boolean = false,
    val terminalSupported: Boolean = false,
    val verifyAllowed: Boolean = false,
    val repairAllowed: Boolean = false,
    val resetAllowed: Boolean = false,
    val writableGuestPaths: List<String> = emptyList(),
    val summary: String = "当前仍使用 legacy Android 执行层",
    val detail: String = "Minijail + 现有 shell 路径保持不变；Mobile Linux 迁移仅接入到 phase-1 管理面板。",
    val lastAction: LinuxRuntimeAction? = null,
    val lastActionMessage: String? = null,
    val busyAction: LinuxRuntimeAction? = null,
    val mounts: List<LinuxRuntimeMountUiState> = emptyList(),
    val mountDraft: LinuxRuntimeMountDraft = LinuxRuntimeMountDraft(),
    val tasks: List<LinuxRuntimeTaskUiState> = emptyList(),
    val terminal: LinuxRuntimeTerminalUiState = LinuxRuntimeTerminalUiState(),
) {
    val badge: String
        get() = when {
            selectedMode == LinuxRuntimeMode.Legacy -> "默认"
            rootfsState == MobileLinuxRootfsStateFfi.BLOCKED_BY_LICENSE -> "授权阻塞"
            rootfsState == MobileLinuxRootfsStateFfi.UNSUPPORTED -> "未接入"
            available -> "可用"
            else -> "不可用"
        }

    val canOpenTerminal: Boolean get() = terminalSupported && available
    val canManageMounts: Boolean get() = available
    val canInspectTasks: Boolean get() = available
}

fun linuxRuntimeStateLabel(state: MobileLinuxRootfsStateFfi): String =
    when (state) {
        MobileLinuxRootfsStateFfi.MISSING -> "未安装"
        MobileLinuxRootfsStateFfi.INSTALLING -> "安装中"
        MobileLinuxRootfsStateFfi.READY -> "就绪"
        MobileLinuxRootfsStateFfi.CORRUPT -> "损坏"
        MobileLinuxRootfsStateFfi.REPAIRING -> "修复中"
        MobileLinuxRootfsStateFfi.RESETTING -> "重置中"
        MobileLinuxRootfsStateFfi.UNSUPPORTED -> "未接入"
        MobileLinuxRootfsStateFfi.BLOCKED_BY_LICENSE -> "授权阻塞"
    }

fun mobileLinuxConfig(
    managedRoot: String,
    workspaceHostPath: String,
    stableWorkspaceId: String,
    abi: String,
    mode: LinuxRuntimeMode,
    authorizationFile: String? = null,
): AndroidMobileLinuxConfigFfi =
    AndroidMobileLinuxConfigFfi(
        mode = mode.toFfi(),
        managedRoot = managedRoot,
        workspaceHostPath = workspaceHostPath,
        stableWorkspaceId = stableWorkspaceId,
        abi = abi,
        rootfsVersion = "1.0.0",
        archiveSha256 = null,
        authorizationFile = authorizationFile,
    )

fun linuxRuntimeUiStateFrom(
    mode: LinuxRuntimeMode,
    capability: MobileLinuxCapabilityFfi,
    status: MobileLinuxStatusFfi,
    lastAction: LinuxRuntimeAction? = null,
    tasks: List<com.lingxi.code.bindings.MobileLinuxTaskSnapshotFfi> = emptyList(),
): LinuxRuntimeUiState {
    val detail = status.lastError ?: capability.reason ?: "未返回额外诊断信息"
    val summary = when {
        mode == LinuxRuntimeMode.Legacy ->
            "当前仍使用 legacy Minijail + 现有 shell 执行层"
        status.state == MobileLinuxRootfsStateFfi.BLOCKED_BY_LICENSE ->
            "缺少额外书面授权，Mobile Linux 后端被显式阻塞"
        status.state == MobileLinuxRootfsStateFfi.UNSUPPORTED ->
            "授权存在或模式已选中，但当前构建未链接 Android PRoot 运行时"
        capability.available ->
            "Mobile Linux 运行时可用"
        else ->
            "Mobile Linux 运行时暂不可用"
    }
    return LinuxRuntimeUiState(
        selectedMode = mode,
        backend = status.backend,
        rootfsState = status.state,
        version = status.version,
        managedRoot = status.managedRoot,
        installedSizeBytes = status.installedSizeBytes?.toLong(),
        available = capability.available,
        terminalSupported = capability.pty,
        verifyAllowed = capability.rootfsIntegrity,
        repairAllowed = capability.rootfsIntegrity,
        resetAllowed = capability.rootfsIntegrity,
        writableGuestPaths = status.writableGuestPaths,
        summary = summary,
        detail = detail,
        lastAction = lastAction,
        lastActionMessage = detail,
        mounts = status.writableGuestPaths.mapIndexed { index, guestPath ->
            LinuxRuntimeMountUiState(
                id = "managed-$index",
                guestPath = guestPath,
                summary = if (guestPath == "/workspace") {
                    "工作区映射；外部目录将通过 SAF 单独授权"
                } else {
                    "rootfs 受管写路径"
                },
                access = LinuxRuntimeMountAccess.ReadWrite,
            )
        },
        tasks = tasks.map {
            LinuxRuntimeTaskUiState(
                id = it.taskId,
                label = it.command.ifBlank { "guest task" },
                state = when (it.status) {
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.QUEUED -> "排队中"
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.RUNNING -> "运行中"
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.BACKGROUNDED -> "后台运行"
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.COMPLETED -> "已完成"
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.FAILED -> "失败"
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.CANCELLED -> "已取消"
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.TIMED_OUT -> "已超时"
                } + (it.detail?.let { detail -> " · $detail" } ?: ""),
                stoppable = it.status == com.lingxi.code.bindings.MobileLinuxTaskStateFfi.RUNNING
                    || it.status == com.lingxi.code.bindings.MobileLinuxTaskStateFfi.BACKGROUNDED,
            )
        },
        terminal = LinuxRuntimeTerminalUiState(
            status = if (capability.pty && capability.available) {
                LinuxRuntimeTerminalStatus.Idle
            } else {
                LinuxRuntimeTerminalStatus.Disabled
            },
            transcript = listOf(
                if (capability.pty && capability.available) {
                    "终端入口已就绪，等待宿主接入真实 PTY 会话。"
                } else {
                    "当前构建未接入可用 PTY 运行时。"
                },
            ),
        ),
    )
}
