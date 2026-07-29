package com.lingxi.code.terminal

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class TerminalViewModelTest {
    private val dispatcher = StandardTestDispatcher()

    @Before
    fun setUp() {
        Dispatchers.setMain(dispatcher)
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
    }

    @Test
    fun initCommandRunsOnlyForFreshSession() = runTest(dispatcher) {
        val gateway = FakeGateway(startResults = ArrayDeque(listOf(TerminalStartResult(created = true))))
        val viewModel = TerminalViewModel(
            args = TerminalRouteArgs(sessionId = "session-1", initCommand = "python3 -V"),
            gateway = gateway,
        )

        viewModel.dispatch(TerminalAction.Start)
        viewModel.dispatch(TerminalAction.Start)
        advanceUntilIdle()

        assertEquals(listOf("session-1"), gateway.startedSessions)
        assertEquals(listOf("python3 -V"), gateway.writes.map(ByteArray::decodeToString))
    }

    @Test
    fun initCommandIsSkippedWhenReattachingExistingSession() = runTest(dispatcher) {
        val gateway = FakeGateway(startResults = ArrayDeque(listOf(TerminalStartResult(created = false))))
        val viewModel = TerminalViewModel(
            args = TerminalRouteArgs(sessionId = "session-1", initCommand = "python3 -V"),
            gateway = gateway,
        )

        viewModel.dispatch(TerminalAction.Start)
        advanceUntilIdle()

        assertEquals(listOf("session-1"), gateway.startedSessions)
        assertTrue(gateway.writes.isEmpty())
    }

    @Test
    fun gatewayOutputFeedsRetainedEmulator() = runTest(dispatcher) {
        val gateway = FakeGateway()
        val viewModel = TerminalViewModel(
            args = TerminalRouteArgs(sessionId = "session-1"),
            gateway = gateway,
        )

        runCurrent()
        gateway.emitOutput("你".toByteArray().copyOfRange(0, 1))
        gateway.emitOutput(byteArrayOf("你".toByteArray()[1], "你".toByteArray()[2], 'A'.code.toByte()))
        advanceUntilIdle()

        assertTrue(viewModel.emulator.visibleLines().first().asString().startsWith("你A"))
    }

    private class FakeGateway(
        val startResults: ArrayDeque<TerminalStartResult> = ArrayDeque(),
    ) : TerminalSessionGateway {
        private val mutableOutput = MutableSharedFlow<ByteArray>(extraBufferCapacity = 16)
        private val mutableState = MutableStateFlow(TerminalConnectionState.IDLE)
        private val mutableError = MutableStateFlow<String?>(null)

        val startedSessions = mutableListOf<String>()
        val writes = mutableListOf<ByteArray>()

        override val output: Flow<ByteArray> = mutableOutput
        override val state: StateFlow<TerminalConnectionState> = mutableState.asStateFlow()
        override val error: StateFlow<String?> = mutableError.asStateFlow()

        override suspend fun start(sessionId: String): TerminalStartResult {
            startedSessions += sessionId
            mutableState.value = TerminalConnectionState.CONNECTED
            return startResults.removeFirstOrNull() ?: TerminalStartResult(created = true)
        }

        override suspend fun write(bytes: ByteArray) {
            writes += bytes
        }

        override suspend fun resize(columns: Int, rows: Int) = Unit

        override suspend fun clear() = Unit

        override suspend fun close() {
            mutableState.value = TerminalConnectionState.CLOSED
        }

        suspend fun emitOutput(bytes: ByteArray) {
            mutableOutput.emit(bytes)
        }
    }

    private fun Array<com.lingxi.code.terminal.emulator.TerminalCell>.asString() =
        buildString {
            this@asString.forEach { cell ->
                if (!cell.wideTrailer) appendCodePoint(cell.codePoint)
            }
        }
}
