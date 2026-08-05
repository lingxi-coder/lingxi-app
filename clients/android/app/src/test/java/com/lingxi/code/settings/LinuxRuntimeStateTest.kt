package com.lingxi.code.settings

import com.lingxi.code.R
import com.lingxi.code.bindings.MobileLinuxCapabilityFfi
import com.lingxi.code.bindings.MobileLinuxTaskSnapshotFfi
import com.lingxi.code.bindings.MobileLinuxTaskStateFfi
import com.lingxi.code.bindings.MobileLinuxRootfsStateFfi
import com.lingxi.code.bindings.MobileLinuxRuntimeModeFfi
import com.lingxi.code.bindings.MobileLinuxStatusFfi
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LinuxRuntimeStateTest {
    @Test
    fun missing_mobile_linux_rootfs_exposes_install_without_terminal() {
        val capability = MobileLinuxCapabilityFfi(
            available = true,
            backend = "android-proot",
            mode = MobileLinuxRuntimeModeFfi.MOBILE_LINUX,
            reason = null,
            streamingOutput = true,
            backgroundProcesses = true,
            pty = true,
            bindMounts = true,
            rootfsIntegrity = true,
        )
        val status = MobileLinuxStatusFfi(
            state = MobileLinuxRootfsStateFfi.MISSING,
            backend = "android-proot",
            mode = MobileLinuxRuntimeModeFfi.MOBILE_LINUX,
            platform = "android",
            abi = "arm64-v8a",
            version = "1.0.0",
            managedRoot = "/tmp/mobile-linux",
            activeRoot = null,
            stagedRoot = null,
            archiveSha256 = "fixed",
            installedSizeBytes = null,
            writableGuestPaths = emptyList(),
            lastError = null,
        )

        val ui = linuxRuntimeUiStateFrom(LinuxRuntimeMode.MobileLinux, capability, status)

        assertTrue(ui.installAllowed)
        assertFalse(ui.canOpenTerminal)
        assertEquals(LinuxRuntimeTerminalStatus.Disabled, ui.terminal.status)
    }

    @Test
    fun blocked_mobile_linux_maps_to_blocked_ui_state() {
        val capability = MobileLinuxCapabilityFfi(
            available = false,
            backend = "android-proot",
            mode = MobileLinuxRuntimeModeFfi.MOBILE_LINUX,
            reason = "missing additional written authorization",
            streamingOutput = false,
            backgroundProcesses = false,
            pty = false,
            bindMounts = false,
            rootfsIntegrity = false,
        )
        val status = MobileLinuxStatusFfi(
            state = MobileLinuxRootfsStateFfi.BLOCKED_BY_LICENSE,
            backend = "android-proot",
            mode = MobileLinuxRuntimeModeFfi.MOBILE_LINUX,
            platform = "android",
            abi = "arm64-v8a",
            version = "alpine-v1-fixed-toolset",
            managedRoot = "/tmp/mobile-linux",
            activeRoot = null,
            stagedRoot = null,
            archiveSha256 = null,
            installedSizeBytes = null,
            writableGuestPaths = listOf("/root", "/tmp"),
            lastError = "missing additional written authorization",
        )

        val ui = linuxRuntimeUiStateFrom(LinuxRuntimeMode.MobileLinux, capability, status)

        assertEquals("android-proot", ui.backend)
        assertEquals(MobileLinuxRootfsStateFfi.BLOCKED_BY_LICENSE, ui.rootfsState)
        assertEquals(R.string.settings_linux_badge_blocked, ui.badgeRes)
        assertFalse(ui.canOpenTerminal)
        assertTrue(ui.detail.orEmpty().contains("authorization"))
        assertEquals(2, ui.mounts.size)
        assertEquals(LinuxRuntimeTerminalStatus.Disabled, ui.terminal.status)
    }

    @Test
    fun stale_runtime_completion_cannot_overwrite_a_new_mode() {
        val store = SettingsStore()
        assertTrue(
            store.tryBeginLinuxRuntimeAction(
                LinuxRuntimeAction.Refresh,
                LinuxRuntimeMode.Legacy,
            ),
        )

        store.setLinuxRuntimeMode(LinuxRuntimeMode.MobileLinux)
        store.completeLinuxRuntimeAction(
            LinuxRuntimeAction.Refresh,
            LinuxRuntimeMode.Legacy,
            LinuxRuntimeUiState(
                selectedMode = LinuxRuntimeMode.Legacy,
                backend = "stale-backend",
            ),
        )

        assertEquals(LinuxRuntimeMode.MobileLinux, store.state.value.linuxRuntime.selectedMode)
        assertEquals("android-minijail", store.state.value.linuxRuntime.backend)
        assertEquals(null, store.state.value.linuxRuntime.busyAction)
    }

    @Test
    fun runtime_actions_are_serialized_until_the_active_action_finishes() {
        val store = SettingsStore()
        assertTrue(
            store.tryBeginLinuxRuntimeAction(
                LinuxRuntimeAction.Verify,
                LinuxRuntimeMode.Legacy,
            ),
        )
        assertFalse(
            store.tryBeginLinuxRuntimeAction(
                LinuxRuntimeAction.Reset,
                LinuxRuntimeMode.Legacy,
            ),
        )

        store.cancelLinuxRuntimeAction(LinuxRuntimeAction.Verify, LinuxRuntimeMode.Legacy)

        assertTrue(
            store.tryBeginLinuxRuntimeAction(
                LinuxRuntimeAction.Reset,
                LinuxRuntimeMode.Legacy,
            ),
        )
    }

    @Test
    fun running_task_snapshots_are_lowered_into_ui_rows() {
        val capability = MobileLinuxCapabilityFfi(
            available = true,
            backend = "android-proot",
            mode = MobileLinuxRuntimeModeFfi.MOBILE_LINUX,
            reason = null,
            streamingOutput = true,
            backgroundProcesses = true,
            pty = true,
            bindMounts = true,
            rootfsIntegrity = true,
        )
        val status = MobileLinuxStatusFfi(
            state = MobileLinuxRootfsStateFfi.READY,
            backend = "android-proot",
            mode = MobileLinuxRuntimeModeFfi.MOBILE_LINUX,
            platform = "android",
            abi = "arm64-v8a",
            version = "1.0.0",
            managedRoot = "/tmp/mobile-linux",
            activeRoot = "/tmp/mobile-linux/active",
            stagedRoot = null,
            archiveSha256 = null,
            installedSizeBytes = 4096UL,
            writableGuestPaths = listOf("/root", "/workspace"),
            lastError = null,
        )

        val ui = linuxRuntimeUiStateFrom(
            mode = LinuxRuntimeMode.MobileLinux,
            capability = capability,
            status = status,
            tasks = listOf(
                MobileLinuxTaskSnapshotFfi(
                    taskId = "run-1",
                    status = MobileLinuxTaskStateFfi.RUNNING,
                    command = "python3 -m http.server",
                    startedAtMs = 1UL,
                    finishedAtMs = null,
                    exitCode = null,
                    detail = "serving /workspace/default",
                ),
            ),
        )

        assertTrue(ui.canOpenTerminal)
        assertFalse(ui.installAllowed)
        assertEquals(1, ui.tasks.size)
        assertTrue(ui.tasks.first().stoppable)
        assertEquals(R.string.settings_linux_task_running, ui.tasks.first().stateRes)
        assertEquals("serving /workspace/default", ui.tasks.first().stateDetail)
    }
}
