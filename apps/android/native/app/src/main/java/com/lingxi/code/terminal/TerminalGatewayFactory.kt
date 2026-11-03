package com.lingxi.code.terminal

import android.content.Context
import com.lingxi.code.settings.LinuxRuntimeMode

internal fun createTerminalGateway(
    context: Context,
    mode: LinuxRuntimeMode,
    cwd: String,
    ownerKey: String,
): TerminalSessionGateway = MobileLinuxTerminalGateway(context, mode, cwd, ownerKey)
