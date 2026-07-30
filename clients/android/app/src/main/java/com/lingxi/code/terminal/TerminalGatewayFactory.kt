package com.lingxi.code.terminal

import android.content.Context
import com.lingxi.code.settings.LinuxRuntimeMode

internal enum class TerminalBackend {
    LegacyPty,
    MobileLinux,
}

internal fun selectTerminalBackend(mode: LinuxRuntimeMode): TerminalBackend =
    when (mode) {
        LinuxRuntimeMode.Legacy -> TerminalBackend.LegacyPty
        LinuxRuntimeMode.MobileLinux -> TerminalBackend.MobileLinux
    }

internal fun createTerminalGateway(
    context: Context,
    mode: LinuxRuntimeMode,
): TerminalSessionGateway =
    when (selectTerminalBackend(mode)) {
        TerminalBackend.LegacyPty -> LegacyTerminalGateway(context)
        TerminalBackend.MobileLinux -> MobileLinuxTerminalGateway(context, mode)
    }
