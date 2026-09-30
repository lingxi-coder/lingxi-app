package com.lingxi.code.terminal

import com.lingxi.code.bindings.android.MobileLinuxEventFfi
import com.lingxi.code.bindings.android.MobileLinuxEventKindFfi
import com.lingxi.code.bindings.android.MobileLinuxPtyOpenRequestFfi
import com.lingxi.code.bindings.android.MobileLinuxPtySessionHandleFfi
import com.lingxi.code.bindings.android.MobileLinuxPtySizeFfi
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test

class MobileLinuxTerminalGatewayTest {
    @Before
    fun resetRegistry() = runTest {
        MobileLinuxTerminalGateway.resetSharedSessions()
    }

    @Test
    fun sameSessionIdReusesExistingPty() = runTest {
        val runtime = FakeRuntime()
        val first = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux)
        val second = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux)

        val firstStart = first.start("session-1")
        val secondStart = second.start("session-1")

        assertEquals(true, firstStart.created)
        assertEquals(false, secondStart.created)
        assertEquals(1, runtime.openCalls)
    }

    @Test
    fun explicitCloseDropsSharedSession() = runTest {
        val runtime = FakeRuntime()
        val first = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux)
        val second = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux)

        first.start("session-1")
        first.close()
        val restarted = second.start("session-1")

        assertEquals(true, restarted.created)
        assertEquals(2, runtime.openCalls)
        assertEquals(1, runtime.closeCalls)
    }

    private class FakeRuntime : MobileLinuxTerminalGateway.TerminalRuntime {
        var openCalls = 0
        var closeCalls = 0
        private var nextHandle = 1
        private val events = mutableListOf<MobileLinuxEventFfi>()

        override suspend fun readEvents(
            afterSequence: ULong?,
            limit: UInt?,
        ): List<MobileLinuxEventFfi> =
            events.filter { afterSequence == null || it.sequence > afterSequence }
                .take(limit?.toInt() ?: events.size)

        override suspend fun openPty(request: MobileLinuxPtyOpenRequestFfi): MobileLinuxPtySessionHandleFfi {
            openCalls += 1
            return MobileLinuxPtySessionHandleFfi(id = "pty-${nextHandle++}")
        }

        override suspend fun writePty(handle: MobileLinuxPtySessionHandleFfi, input: ByteArray) = Unit

        override suspend fun resizePty(handle: MobileLinuxPtySessionHandleFfi, size: MobileLinuxPtySizeFfi) = Unit

        override suspend fun closePty(handle: MobileLinuxPtySessionHandleFfi) {
            closeCalls += 1
        }
    }
}
