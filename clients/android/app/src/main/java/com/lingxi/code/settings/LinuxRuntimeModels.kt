package com.lingxi.code.settings

import androidx.annotation.StringRes
import com.lingxi.code.R
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

enum class LinuxRuntimeAction(@StringRes val labelRes: Int) {
    Refresh(R.string.settings_linux_refresh_button),
    Install(R.string.settings_linux_action_install_short),
    Verify(R.string.settings_linux_action_verify_short),
    Repair(R.string.settings_linux_action_repair_short),
    Reset(R.string.settings_linux_action_reset_short),
    Boot(R.string.common_start),
    Shutdown(R.string.common_close),
    OpenTerminal(R.string.settings_linux_open_terminal),
    RefreshTasks(R.string.settings_linux_refresh_tasks_short),
    RefreshMounts(R.string.settings_linux_refresh_mounts_short),
    StopTask(R.string.settings_linux_stop_task_short),
}

enum class LinuxRuntimeTerminalStatus { Disabled, Idle, Starting, Active }

enum class LinuxRuntimeMountAccess(@StringRes val labelRes: Int) {
    ReadWrite(R.string.settings_linux_read_write),
}

data class LinuxRuntimeMountUiState(
    val id: String,
    val guestPath: String,
    @StringRes val summaryRes: Int,
    val access: LinuxRuntimeMountAccess,
    val pendingGrant: Boolean = false,
)

data class LinuxRuntimeMountDraft(
    @StringRes val purposeLabelRes: Int = R.string.settings_linux_external_dir,
    // Android PRoot cannot enforce read-only bind mounts. Expose only the
    // access mode the runtime can actually honor.
    val access: LinuxRuntimeMountAccess = LinuxRuntimeMountAccess.ReadWrite,
    @StringRes val pickerSummaryRes: Int = R.string.settings_linux_saf_picker_summary,
)

data class LinuxRuntimeTaskUiState(
    val id: String,
    val label: String,
    @StringRes val stateRes: Int,
    val stateDetail: String? = null,
    val stoppable: Boolean,
)

data class LinuxRuntimeTerminalUiState(
    val status: LinuxRuntimeTerminalStatus = LinuxRuntimeTerminalStatus.Disabled,
    val sessionId: String? = null,
    val commandDraft: String = "python3 -V",
    val transcript: List<String> = emptyList(),
)

data class LinuxRuntimeTerminalLaunchRequest(
    val sessionId: String,
    val initCommand: String,
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
    val mountSupported: Boolean = false,
    val installAllowed: Boolean = false,
    val verifyAllowed: Boolean = false,
    val repairAllowed: Boolean = false,
    val resetAllowed: Boolean = false,
    val writableGuestPaths: List<String> = emptyList(),
    @StringRes val summaryRes: Int = R.string.settings_linux_summary_legacy_android,
    val detail: String? = null,
    @StringRes val detailRes: Int = R.string.settings_linux_detail_legacy_android,
    val lastAction: LinuxRuntimeAction? = null,
    val lastActionMessage: String? = null,
    val busyAction: LinuxRuntimeAction? = null,
    val mounts: List<LinuxRuntimeMountUiState> = emptyList(),
    val mountDraft: LinuxRuntimeMountDraft = LinuxRuntimeMountDraft(),
    val tasks: List<LinuxRuntimeTaskUiState> = emptyList(),
    val terminal: LinuxRuntimeTerminalUiState = LinuxRuntimeTerminalUiState(),
) {
    @get:StringRes
    val badgeRes: Int
        get() = when {
            selectedMode == LinuxRuntimeMode.Legacy -> R.string.settings_badge_default
            rootfsState == MobileLinuxRootfsStateFfi.BLOCKED_BY_LICENSE -> R.string.settings_linux_badge_blocked
            rootfsState == MobileLinuxRootfsStateFfi.UNSUPPORTED -> R.string.settings_linux_badge_not_linked
            available -> R.string.settings_status_available
            else -> R.string.settings_linux_task_unavailable
        }

    val canOpenTerminal: Boolean
        get() = terminalSupported && available && rootfsState == MobileLinuxRootfsStateFfi.READY
    val canManageMounts: Boolean get() = mountSupported && available
    val canInspectTasks: Boolean get() = available
}

@StringRes
fun linuxRuntimeStateLabel(state: MobileLinuxRootfsStateFfi): Int =
    when (state) {
        MobileLinuxRootfsStateFfi.MISSING -> R.string.settings_linux_not_installed
        MobileLinuxRootfsStateFfi.INSTALLING -> R.string.settings_linux_state_installing
        MobileLinuxRootfsStateFfi.READY -> R.string.settings_linux_state_ready
        MobileLinuxRootfsStateFfi.CORRUPT -> R.string.settings_linux_state_corrupt
        MobileLinuxRootfsStateFfi.REPAIRING -> R.string.settings_linux_state_repairing
        MobileLinuxRootfsStateFfi.RESETTING -> R.string.settings_linux_state_resetting
        MobileLinuxRootfsStateFfi.UNSUPPORTED -> R.string.settings_linux_badge_not_linked
        MobileLinuxRootfsStateFfi.BLOCKED_BY_LICENSE -> R.string.settings_linux_badge_blocked
    }

fun mobileLinuxConfig(
    managedRoot: String,
    appSandboxRoot: String,
    workspaceHostPath: String,
    stableWorkspaceId: String,
    abi: String,
    mode: LinuxRuntimeMode,
    authorizationFile: String? = null,
    rootfsIdentity: RootfsArtifactIdentity? = null,
): AndroidMobileLinuxConfigFfi {
    if (mode == LinuxRuntimeMode.MobileLinux) {
        checkNotNull(rootfsIdentity) { "Mobile Linux requires a bundled verified release rootfs for ABI $abi" }
    }
    check(rootfsIdentity == null || rootfsIdentity.abi == abi) { "Bundled rootfs identity ABI mismatch" }
    return AndroidMobileLinuxConfigFfi(
        mode = mode.toFfi(),
        managedRoot = managedRoot,
        appSandboxRoot = appSandboxRoot,
        workspaceHostPath = workspaceHostPath,
        stableWorkspaceId = stableWorkspaceId,
        abi = abi,
        rootfsVersion = rootfsIdentity?.version.orEmpty(),
        archiveSha256 = rootfsIdentity?.sha256,
        authorizationFile = authorizationFile,
    )
}

fun linuxRuntimeUiStateFrom(
    mode: LinuxRuntimeMode,
    capability: MobileLinuxCapabilityFfi,
    status: MobileLinuxStatusFfi,
    lastAction: LinuxRuntimeAction? = null,
    tasks: List<com.lingxi.code.bindings.MobileLinuxTaskSnapshotFfi> = emptyList(),
): LinuxRuntimeUiState {
    val detail = status.lastError ?: capability.reason
    val summaryRes = when {
        mode == LinuxRuntimeMode.Legacy ->
            R.string.settings_linux_summary_legacy_minijail
        status.state == MobileLinuxRootfsStateFfi.BLOCKED_BY_LICENSE ->
            R.string.settings_linux_summary_blocked_license_android
        status.state == MobileLinuxRootfsStateFfi.UNSUPPORTED ->
            R.string.settings_linux_summary_not_linked_android
        capability.available ->
            R.string.settings_linux_summary_available
        else ->
            R.string.settings_linux_summary_unavailable
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
        mountSupported = capability.bindMounts,
        installAllowed = mode == LinuxRuntimeMode.MobileLinux &&
            status.state == MobileLinuxRootfsStateFfi.MISSING,
        verifyAllowed = capability.rootfsIntegrity,
        repairAllowed = capability.rootfsIntegrity,
        resetAllowed = capability.rootfsIntegrity,
        writableGuestPaths = status.writableGuestPaths,
        summaryRes = summaryRes,
        detail = detail,
        detailRes = R.string.settings_linux_no_diagnostics,
        lastAction = lastAction,
        lastActionMessage = detail,
        mounts = status.writableGuestPaths.mapIndexed { index, guestPath ->
            LinuxRuntimeMountUiState(
                id = "managed-$index",
                guestPath = guestPath,
                summaryRes = if (guestPath == "/workspace") {
                    R.string.settings_linux_mount_workspace_summary
                } else {
                    R.string.settings_linux_mount_managed_summary
                },
                access = LinuxRuntimeMountAccess.ReadWrite,
            )
        },
        tasks = tasks.map {
            LinuxRuntimeTaskUiState(
                id = it.taskId,
                label = it.command.ifBlank { "guest task" },
                stateRes = when (it.status) {
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.QUEUED -> R.string.settings_linux_task_queued
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.RUNNING -> R.string.settings_linux_task_running
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.BACKGROUNDED -> R.string.settings_linux_task_backgrounded
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.COMPLETED -> R.string.chat_status_completed
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.FAILED -> R.string.settings_linux_task_failed
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.CANCELLED -> R.string.chat_status_cancelled
                    com.lingxi.code.bindings.MobileLinuxTaskStateFfi.TIMED_OUT -> R.string.settings_linux_task_timed_out
                },
                stateDetail = it.detail,
                stoppable = it.status == com.lingxi.code.bindings.MobileLinuxTaskStateFfi.RUNNING
                    || it.status == com.lingxi.code.bindings.MobileLinuxTaskStateFfi.BACKGROUNDED,
            )
        },
        terminal = LinuxRuntimeTerminalUiState(
            status = if (
                capability.pty &&
                capability.available &&
                status.state == MobileLinuxRootfsStateFfi.READY
            ) {
                LinuxRuntimeTerminalStatus.Idle
            } else {
                LinuxRuntimeTerminalStatus.Disabled
            },
        ),
    )
}
