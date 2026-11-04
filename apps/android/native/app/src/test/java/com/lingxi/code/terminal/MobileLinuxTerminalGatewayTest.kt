package com.lingxi.code.terminal

import com.lingxi.code.bindings.android.MobileLinuxEventFfi
import com.lingxi.code.bindings.android.MobileLinuxEventKindFfi
import com.lingxi.code.bindings.android.MobileLinuxPtyOpenRequestFfi
import com.lingxi.code.bindings.android.MobileLinuxPtySessionHandleFfi
import com.lingxi.code.bindings.android.MobileLinuxPtySizeFfi
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
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
        val first = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux, "/home/user/.brand/chat", "conversation:a")
        val second = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux, "/home/user/.brand/chat", "conversation:a")

        val firstStart = first.start("session-1")
        val secondStart = second.start("session-1")

        assertEquals(true, firstStart.created)
        assertEquals(false, secondStart.created)
        assertEquals(1, runtime.openCalls)
        assertEquals(listOf("/home/user/.brand/chat"), runtime.cwdRequests)
    }

    @Test
    fun explicitCloseDropsSharedSession() = runTest {
        val runtime = FakeRuntime()
        val first = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux, "/home/user/.brand/chat", "conversation:a")
        val second = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux, "/home/user/.brand/chat", "conversation:a")

        first.start("session-1")
        first.close()
        val restarted = second.start("session-1")

        assertEquals(true, restarted.created)
        assertEquals(2, runtime.openCalls)
        assertEquals(1, runtime.closeCalls)
    }

    @Test
    fun switchingOwnerOrWorkspaceDoesNotReuseOrCloseTheEarlierPty() = runTest {
        val runtime = FakeRuntime()
        val first = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux, "/home/user/.brand/chat", "conversation:a")
        val otherWorkspace = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux, "/home/user/.brand/code", "conversation:b")
        val sameWorkspaceOtherOwner = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux, "/home/user/.brand/code", "conversation:c")

        assertEquals(true, first.start("interactive").created)
        assertEquals(true, otherWorkspace.start("interactive").created)
        assertEquals(true, sameWorkspaceOtherOwner.start("interactive").created)
        assertEquals(listOf("/home/user/.brand/chat", "/home/user/.brand/code", "/home/user/.brand/code"), runtime.cwdRequests)
        assertEquals(0, runtime.closeCalls)
    }

    @Test
    fun missingOwnerOrWorkingDirectoryIsRejected() {
        val runtime = FakeRuntime()
        assertThrows(IllegalArgumentException::class.java) {
            MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux, "", "conversation:a")
        }
        assertThrows(IllegalArgumentException::class.java) {
            MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux, "/home/user/.brand/chat", "")
        }
    }

    @Test
    fun failureBeforeNativeOpenCanRetryOnTheSameGateway() = runTest {
        val runtime = FakeRuntime().apply { readFailuresRemaining = 1 }
        val gateway = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux,
            "/home/user/.brand/chat", "conversation:a")
        try {
            gateway.start("session-1")
            throw AssertionError("Expected controlled open failure")
        } catch (expected: IllegalStateException) {
            assertEquals("controlled read failure", expected.message)
        }
        assertEquals(true, gateway.start("session-1").created)
        assertEquals(1, runtime.openCalls)
        gateway.close()
    }

    @Test
    fun unknownNativeOpenOutcomeKeepsTheOwnerReserved() = runTest {
        val runtime = FakeRuntime().apply { openFailuresRemaining = 1 }
        val first = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux,
            "/home/user/.brand/chat", "conversation:a")
        assertEquals(true, runCatching { first.start("session-1") }.isFailure)
        assertEquals(true, runCatching { first.start("session-1") }.isFailure)
        first.close()
        val reopened = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux,
            "/home/user/.brand/chat", "conversation:a")
        assertEquals(true, runCatching { reopened.start("session-1") }.isFailure)
        assertEquals(1, runtime.openCalls)
    }

    @Test
    fun unconfirmedCloseKeepsTheOwnerReserved() = runTest {
        val runtime = FakeRuntime().apply { closeFailuresRemaining = 1 }
        val first = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux,
            "/home/user/.brand/chat", "conversation:a")
        first.start("session-1")
        first.close()
        val reopened = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux,
            "/home/user/.brand/chat", "conversation:a")
        assertEquals(true, runCatching { reopened.start("session-1") }.isFailure)
        assertEquals(1, runtime.openCalls)
    }

    @Test
    fun cancellationBeforeNativeOpenReleasesOnlyItsOwnReservation() = runTest {
        val gate = CompletableDeferred<Unit>()
        val runtime = FakeRuntime().apply { firstReadGate = gate }
        val first = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux,
            "/home/user/.brand/chat", "conversation:a")
        val pending = async { first.start("session-1") }
        runCurrent()
        pending.cancelAndJoin()
        val reopened = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux,
            "/home/user/.brand/chat", "conversation:a")
        assertEquals(true, reopened.start("session-1").created)
        assertEquals(1, runtime.openCalls)
        reopened.close()
    }

    @Test
    fun staleSessionCleanupCannotRemoveItsReplacement() {
        val original = Any()
        val replacement = Any()
        val owners = mutableMapOf("same-key" to original)
        owners["same-key"] = replacement
        owners.removeIfSame("same-key", original)
        assertEquals(replacement, owners["same-key"])
        owners.removeIfSame("same-key", replacement)
        assertEquals(null, owners["same-key"])
    }

    private class FakeRuntime : MobileLinuxTerminalGateway.TerminalRuntime {
        var openCalls = 0
        var closeCalls = 0
        var readFailuresRemaining = 0
        var openFailuresRemaining = 0
        var closeFailuresRemaining = 0
        var firstReadGate: CompletableDeferred<Unit>? = null
        val cwdRequests = mutableListOf<String?>()
        private var nextHandle = 1
        private val events = mutableListOf<MobileLinuxEventFfi>()

        override suspend fun readEvents(
            afterSequence: ULong?,
            limit: UInt?,
        ): List<MobileLinuxEventFfi> {
            val gate = firstReadGate
            firstReadGate = null
            gate?.await()
            if (readFailuresRemaining > 0) {
                readFailuresRemaining -= 1
                error("controlled read failure")
            }
            return events.filter { afterSequence == null || it.sequence > afterSequence }
                .take(limit?.toInt() ?: events.size)
        }

        override suspend fun openPty(request: MobileLinuxPtyOpenRequestFfi): MobileLinuxPtySessionHandleFfi {
            openCalls += 1
            cwdRequests += request.cwd
            if (openFailuresRemaining > 0) {
                openFailuresRemaining -= 1
                error("controlled open failure")
            }
            return MobileLinuxPtySessionHandleFfi(id = "pty-${nextHandle++}")
        }

        override suspend fun writePty(handle: MobileLinuxPtySessionHandleFfi, input: ByteArray) = Unit

        override suspend fun resizePty(handle: MobileLinuxPtySessionHandleFfi, size: MobileLinuxPtySizeFfi) = Unit

        override suspend fun closePty(handle: MobileLinuxPtySessionHandleFfi) {
            closeCalls += 1
            if (closeFailuresRemaining > 0) {
                closeFailuresRemaining -= 1
                error("controlled close failure")
            }
        }
    }
}
