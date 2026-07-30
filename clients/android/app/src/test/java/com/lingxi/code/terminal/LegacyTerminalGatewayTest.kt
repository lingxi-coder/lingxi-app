package com.lingxi.code.terminal

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LegacyTerminalGatewayTest {
    @Test
    fun startsInteractivePtyAndForwardsInputResizeAndClose() = runTest {
        val runtime = FakeRuntime()
        val gateway = LegacyTerminalGateway(runtime)

        val result = gateway.start("interactive")
        gateway.write("echo ok".toByteArray())
        gateway.resize(120, 40)
        gateway.close()

        assertTrue(result.created)
        assertEquals(TerminalConnectionState.CLOSED, gateway.state.value)
        assertEquals(LegacyTerminalGateway.LegacyPtySize(80, 24), runtime.openedSize)
        assertArrayEquals("echo ok".toByteArray(), runtime.written)
        assertEquals(LegacyTerminalGateway.LegacyPtySize(120, 40), runtime.resizedTo)
        assertEquals(41, runtime.closedFd)
        assertEquals(73, runtime.signalledPid)
    }

    @Test
    fun startupFailureIsExposedThroughGatewayState() = runTest {
        val failure = IllegalStateException("PTY unavailable")
        val gateway = LegacyTerminalGateway(FakeRuntime(openFailure = failure))

        val thrown = runCatching { gateway.start("interactive") }.exceptionOrNull()

        assertEquals(failure::class, thrown?.let { it::class })
        assertEquals("PTY unavailable", thrown?.message)
        assertEquals(TerminalConnectionState.FAILED, gateway.state.value)
        assertEquals("PTY unavailable", gateway.error.value)
    }

    private class FakeRuntime(
        private val openFailure: Throwable? = null,
    ) : LegacyTerminalGateway.LegacyPtyRuntime {
        private val exit = CountDownLatch(1)
        var openedSize: LegacyTerminalGateway.LegacyPtySize? = null
        var written = byteArrayOf()
        var resizedTo: LegacyTerminalGateway.LegacyPtySize? = null
        var closedFd: Int? = null
        var signalledPid: Int? = null

        override fun open(
            size: LegacyTerminalGateway.LegacyPtySize,
        ): LegacyTerminalGateway.LegacyPtySession {
            openFailure?.let { throw it }
            openedSize = size
            return LegacyTerminalGateway.LegacyPtySession(fd = 41, pid = 73)
        }

        override fun read(
            fd: Int,
            buffer: ByteArray,
            offset: Int,
            length: Int,
        ): Int = 0

        override fun write(
            fd: Int,
            buffer: ByteArray,
            offset: Int,
            length: Int,
        ): Int {
            written += buffer.copyOfRange(offset, offset + length)
            return length
        }

        override fun resize(
            fd: Int,
            size: LegacyTerminalGateway.LegacyPtySize,
        ): Int {
            resizedTo = size
            return 0
        }

        override fun close(fd: Int): Int {
            closedFd = fd
            return 0
        }

        override fun signal(pid: Int, signal: Int): Int {
            signalledPid = pid
            exit.countDown()
            return 0
        }

        override fun waitFor(pid: Int): Int {
            exit.await(5, TimeUnit.SECONDS)
            return 0
        }
    }
}
