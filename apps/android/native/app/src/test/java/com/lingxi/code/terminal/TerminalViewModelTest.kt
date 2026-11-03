package com.lingxi.code.terminal

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.CompletableDeferred
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
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import com.lingxi.code.bindings.android.MobileLinuxEventFfi
import com.lingxi.code.bindings.android.MobileLinuxEventKindFfi
import com.lingxi.code.bindings.android.MobileLinuxPtyOpenRequestFfi
import com.lingxi.code.bindings.android.MobileLinuxPtySessionHandleFfi
import com.lingxi.code.bindings.android.MobileLinuxPtySizeFfi
import com.lingxi.code.settings.LinuxRuntimeMode
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.assertNotEquals
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
    fun initCommandWaitsForExplicitConfirmation() = runTest(dispatcher) {
        val gateway = FakeGateway(startResults = ArrayDeque(listOf(TerminalStartResult(created = true))))
        val viewModel = TerminalViewModel(
            args = TerminalRouteArgs(sessionId = "session-1", cwd = "/home/user/.brand/code", initCommand = "python3 -V"),
            gateway = gateway,
        )

        viewModel.dispatch(TerminalAction.Start)
        viewModel.dispatch(TerminalAction.Start)
        advanceUntilIdle()

        assertEquals(listOf("session-1"), gateway.startedSessions)
        assertTrue(gateway.writes.isEmpty())
        assertEquals("python3 -V", viewModel.uiState.value.commandDraft)
        assertEquals("/home/user/.brand/code", viewModel.uiState.value.args.cwd)
        viewModel.dispatch(TerminalAction.ConfirmCommand)
        advanceUntilIdle()
        assertEquals(listOf("python3 -V\n"), gateway.writes.map(ByteArray::decodeToString))
    }

    @Test
    fun initCommandIsSkippedWhenReattachingExistingSession() = runTest(dispatcher) {
        val gateway = FakeGateway(startResults = ArrayDeque(listOf(TerminalStartResult(created = false))))
        val viewModel = TerminalViewModel(
            args = TerminalRouteArgs(sessionId = "session-1", cwd = "/home/user/.brand/code", initCommand = "python3 -V"),
            gateway = gateway,
        )

        viewModel.dispatch(TerminalAction.Start)
        advanceUntilIdle()

        assertEquals(listOf("session-1"), gateway.startedSessions)
        assertTrue(gateway.writes.isEmpty())
    }

    @Test
    fun multilineCommandCancellationNeverWritesToPty() = runTest(dispatcher) {
        val gateway = FakeGateway()
        val viewModel = TerminalViewModel(TerminalRouteArgs("s1", "/home/user/.brand/code", "echo one\necho two"), gateway)
        viewModel.dispatch(TerminalAction.Start)
        advanceUntilIdle()
        assertTrue(gateway.writes.isEmpty())
        viewModel.dispatch(TerminalAction.DismissCommand)
        viewModel.dispatch(TerminalAction.ConfirmCommand)
        advanceUntilIdle()
        assertTrue(gateway.writes.isEmpty())
        assertEquals(null, viewModel.uiState.value.commandDraft)
    }

    @Test
    fun gatewayOutputFeedsRetainedEmulator() = runTest(dispatcher) {
        val gateway = FakeGateway()
        val viewModel = TerminalViewModel(
            args = TerminalRouteArgs(sessionId = "session-1", cwd = "/home/user/.brand/code"),
            gateway = gateway,
        )

        runCurrent()
        gateway.emitOutput("你".toByteArray().copyOfRange(0, 1))
        gateway.emitOutput(byteArrayOf("你".toByteArray()[1], "你".toByteArray()[2], 'A'.code.toByte()))
        advanceUntilIdle()

        assertTrue(viewModel.emulator.visibleLines().first().asString().startsWith("你A"))
    }

    @Test
    fun hiddenTerminalKeepsOutputAndPendingCommandsAcrossReopen() = runTest(dispatcher) {
        val gateway = FakeGateway()
        val viewModel = TerminalViewModel(TerminalRouteArgs("s1", "/home/user/.brand/chat", "first"), gateway)
        viewModel.acceptLaunch(1, "first")
        viewModel.dispatch(TerminalAction.Start)
        advanceUntilIdle()
        viewModel.dispatch(TerminalAction.EditCommand("edited first"))
        gateway.emitOutput("hidden output".toByteArray())
        gateway.emitOutput("\u001b[6n".toByteArray())
        viewModel.acceptLaunch(2, null)
        viewModel.acceptLaunch(3, "second")
        viewModel.acceptLaunch(3, "second")
        viewModel.dispatch(TerminalAction.Start)
        advanceUntilIdle()

        assertEquals(listOf("s1"), gateway.startedSessions)
        assertEquals(0, gateway.closeCalls)
        assertEquals("edited first", viewModel.uiState.value.commandDraft)
        assertTrue(viewModel.emulator.visibleLines().first().asString().startsWith("hidden output"))
        assertEquals("\u001b[1;14R", gateway.writes.single().decodeToString())
        viewModel.dispatch(TerminalAction.ConfirmCommand)
        advanceUntilIdle()
        assertEquals(listOf("\u001b[1;14R", "edited first\n"), gateway.writes.map(ByteArray::decodeToString))
        assertEquals("second", viewModel.uiState.value.commandDraft)
        viewModel.dispatch(TerminalAction.DismissCommand)
        advanceUntilIdle()
        assertEquals(null, viewModel.uiState.value.commandDraft)
    }

    @Test
    fun reopenWaitsForExplicitCloseBeforeStartingAnotherPty() = runTest(dispatcher) {
        val release = CompletableDeferred<Unit>()
        val gateway = FakeGateway(closeGate = release)
        val viewModel = TerminalViewModel(TerminalRouteArgs("s1", "/home/user/.brand/chat"), gateway)
        viewModel.dispatch(TerminalAction.Start)
        advanceUntilIdle()
        viewModel.dispatch(TerminalAction.Close)
        runCurrent()
        viewModel.dispatch(TerminalAction.Start)
        runCurrent()
        assertEquals(listOf("s1"), gateway.startedSessions)
        release.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf("s1", "s1"), gateway.startedSessions)
        assertEquals(1, gateway.closeCalls)
    }

    @Test
    fun aFailedOpenCanBeRetriedWhenTheSameOwnerReopens() = runTest(dispatcher) {
        val gateway = FakeGateway(failFirstStart = true)
        val viewModel = TerminalViewModel(TerminalRouteArgs("s1", "/home/user/.brand/chat"), gateway)
        viewModel.dispatch(TerminalAction.Start)
        runCurrent()
        viewModel.dispatch(TerminalAction.Start)
        runCurrent()
        assertEquals(listOf("s1", "s1"), gateway.startedSessions)
        assertEquals(TerminalConnectionState.CONNECTED, viewModel.uiState.value.connectionState)
    }

    @Test
    fun explicitCloseWaitsForAnInFlightOpenAndClosesItsExactHandle() = runTest(dispatcher) {
        MobileLinuxTerminalGateway.resetSharedSessions()
        val gate = CompletableDeferred<Unit>()
        val runtime = ControllablePtyRuntime(openGate = gate)
        val gateway = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux,
            "/home/user/.brand/chat", "conversation:a", dispatcher, UnconfinedTestDispatcher(testScheduler))
        val viewModel = TerminalViewModel(TerminalRouteArgs("s1", "/home/user/.brand/chat"), gateway)
        viewModel.dispatch(TerminalAction.Start)
        runCurrent()
        assertEquals(1, runtime.openCalls)
        viewModel.dispatch(TerminalAction.Close)
        runCurrent()
        assertTrue(runtime.closedHandles.isEmpty())
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf("pty-1"), runtime.closedHandles)
        viewModel.dispatch(TerminalAction.Start)
        runCurrent()
        assertEquals(2, runtime.openCalls)
        viewModel.dispatch(TerminalAction.Close)
        runCurrent()
        assertEquals(listOf("pty-1", "pty-2"), runtime.closedHandles)
    }

    @Test
    fun realGatewayAndViewModelRetryOnlyAFailureBeforeNativeOpen() = runTest(dispatcher) {
        MobileLinuxTerminalGateway.resetSharedSessions()
        val runtime = ControllablePtyRuntime(readFailuresRemaining = 1)
        val gateway = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux,
            "/home/user/.brand/chat", "conversation:a", dispatcher, UnconfinedTestDispatcher(testScheduler))
        val viewModel = TerminalViewModel(TerminalRouteArgs("s1", "/home/user/.brand/chat"), gateway)
        viewModel.dispatch(TerminalAction.Start)
        runCurrent()
        assertEquals(0, runtime.openCalls)
        viewModel.dispatch(TerminalAction.Start)
        runCurrent()
        assertEquals(1, runtime.openCalls)
        assertEquals(TerminalConnectionState.CONNECTED, viewModel.uiState.value.connectionState)
        viewModel.dispatch(TerminalAction.Close)
        runCurrent()
        assertEquals(listOf("pty-1"), runtime.closedHandles)
    }

    @Test
    fun immediatePtyOutputAndTerminalQuerySurviveBeforeBridgeDispatcherRuns() = runTest(dispatcher) {
        MobileLinuxTerminalGateway.resetSharedSessions()
        val runtime = ControllablePtyRuntime(initialOutput = "hello\u001b[6n".toByteArray())
        val gateway = MobileLinuxTerminalGateway(runtime, LinuxRuntimeMode.MobileLinux,
            "/home/user/.brand/code", "conversation:b", dispatcher, UnconfinedTestDispatcher(testScheduler))
        val viewModel = TerminalViewModel(TerminalRouteArgs("s2", "/home/user/.brand/code"), gateway)
        viewModel.dispatch(TerminalAction.Start)
        runCurrent()
        assertTrue(viewModel.emulator.visibleLines().first().asString().startsWith("hello"))
        assertEquals(listOf("\u001b[1;6R"), runtime.writes.map(ByteArray::decodeToString))
        viewModel.dispatch(TerminalAction.Close)
        runCurrent()
    }

    @Test
    fun viewIdentityIncludesAllOwnerAndRuntimeCoordinates() {
        val first = terminalOwnerViewKey("session", "owner-a", "/project", "linux")
        assertNotEquals(first, terminalOwnerViewKey("session", "owner-b", "/project", "linux"))
        assertNotEquals(first, terminalOwnerViewKey("session", "owner-a", "/other", "linux"))
        assertNotEquals(terminalOwnerViewKey("a:b", "c", "/p", "linux"),
            terminalOwnerViewKey("a", "b:c", "/p", "linux"))
    }

    private class FakeGateway(
        val startResults: ArrayDeque<TerminalStartResult> = ArrayDeque(),
        val closeGate: CompletableDeferred<Unit>? = null,
        val failFirstStart: Boolean = false,
    ) : TerminalSessionGateway {
        private val mutableOutput = MutableSharedFlow<ByteArray>(extraBufferCapacity = 16)
        private val mutableState = MutableStateFlow(TerminalConnectionState.IDLE)
        private val mutableError = MutableStateFlow<String?>(null)

        val startedSessions = mutableListOf<String>()
        val writes = mutableListOf<ByteArray>()
        var closeCalls = 0

        override val output: Flow<ByteArray> = mutableOutput
        override val state: StateFlow<TerminalConnectionState> = mutableState.asStateFlow()
        override val error: StateFlow<String?> = mutableError.asStateFlow()

        override suspend fun start(sessionId: String): TerminalStartResult {
            startedSessions += sessionId
            if (failFirstStart && startedSessions.size == 1) {
                mutableState.value = TerminalConnectionState.FAILED
                error("controlled opening failure")
            }
            mutableState.value = TerminalConnectionState.CONNECTED
            return startResults.removeFirstOrNull() ?: TerminalStartResult(created = true)
        }

        override suspend fun write(bytes: ByteArray) {
            writes += bytes
        }

        override suspend fun resize(columns: Int, rows: Int) = Unit

        override suspend fun clear() = Unit

        override suspend fun close() {
            closeCalls += 1
            closeGate?.await()
            mutableState.value = TerminalConnectionState.CLOSED
        }

        suspend fun emitOutput(bytes: ByteArray) {
            mutableOutput.emit(bytes)
        }
    }

    private class ControllablePtyRuntime(
        val openGate: CompletableDeferred<Unit>? = null,
        val initialOutput: ByteArray? = null,
        var readFailuresRemaining: Int = 0,
    ) : MobileLinuxTerminalGateway.TerminalRuntime {
        var openCalls = 0
        private var readCalls = 0
        val closedHandles = mutableListOf<String>()
        val writes = mutableListOf<ByteArray>()
        override suspend fun readEvents(afterSequence: ULong?, limit: UInt?): List<MobileLinuxEventFfi> {
            if (readFailuresRemaining > 0) {
                readFailuresRemaining -= 1
                error("controlled preflight failure")
            }
            readCalls += 1
            return if (readCalls == 2 && initialOutput != null) listOf(MobileLinuxEventFfi(
                sequence = 1uL, taskId = null, kind = MobileLinuxEventKindFfi.PTY_OUTPUT,
                data = initialOutput, text = null, sessionId = "pty-1", status = null,
                exitCode = null, timedOut = null, cancelled = null, detail = null,
            )) else emptyList()
        }
        override suspend fun openPty(request: MobileLinuxPtyOpenRequestFfi): MobileLinuxPtySessionHandleFfi {
            openCalls += 1
            val id = "pty-$openCalls"
            openGate?.await()
            return MobileLinuxPtySessionHandleFfi(id)
        }
        override suspend fun writePty(handle: MobileLinuxPtySessionHandleFfi, input: ByteArray) { writes += input }
        override suspend fun resizePty(handle: MobileLinuxPtySessionHandleFfi, size: MobileLinuxPtySizeFfi) = Unit
        override suspend fun closePty(handle: MobileLinuxPtySessionHandleFfi) { closedHandles += handle.id }
    }

    private fun Array<com.lingxi.code.terminal.emulator.TerminalCell>.asString() =
        buildString {
            this@asString.forEach { cell ->
                if (!cell.wideTrailer) appendCodePoint(cell.codePoint)
            }
        }
}
