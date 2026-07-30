package com.lingxi.code.terminal

import com.lingxi.code.settings.LinuxRuntimeMode
import org.junit.Assert.assertEquals
import org.junit.Test

class TerminalGatewayFactoryTest {
    @Test
    fun legacyModeUsesTheLocalPtyBackend() {
        assertEquals(
            TerminalBackend.LegacyPty,
            selectTerminalBackend(LinuxRuntimeMode.Legacy),
        )
    }

    @Test
    fun mobileLinuxModeUsesTheManagedRuntimeBackend() {
        assertEquals(
            TerminalBackend.MobileLinux,
            selectTerminalBackend(LinuxRuntimeMode.MobileLinux),
        )
    }
}
