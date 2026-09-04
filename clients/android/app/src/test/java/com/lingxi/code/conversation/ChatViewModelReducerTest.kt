package com.lingxi.code.conversation

import androidx.lifecycle.SavedStateHandle
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.CostDto
import com.lingxi.code.bindings.HeadlineKindDto
import com.lingxi.code.bindings.ImageRefDto
import com.lingxi.code.bindings.MessageBlockDto
import com.lingxi.code.bindings.MessageDto
import com.lingxi.code.bindings.PlanTaskDto
import com.lingxi.code.bindings.PlanTaskStateDto
import com.lingxi.code.bindings.TaskStatusDto
import com.lingxi.code.bindings.TaskRowDto
import com.lingxi.code.bindings.ToolHeaderDto
import com.lingxi.code.bindings.ToolResultDisplayDto
import com.lingxi.code.bindings.ToolVerbDto
import com.lingxi.code.bindings.TurnOutcomeDto
import com.lingxi.code.bindings.TurnRecoverySnapshotDto
import com.lingxi.code.bindings.TurnRecoveryStateDto
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.SessionRow
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * Reducer-level tests for [ChatViewModel] — the second half of the engine-path
 * coverage (the first being [ClientEventMapperTest]). These drive the `internal`
 * [ChatViewModel.reduce] directly with a stub source so no coroutine / engine is
 * involved: we assert the exact [ChatState] transition each [ReplyEvent] causes,
 * including the streaming-message accumulation that mirrors the iOS
 * `EngineConversationSource.appendDelta`.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class ChatViewModelReducerTest {

    private val dispatcher = UnconfinedTestDispatcher()

    @Before fun setMain() = Dispatchers.setMain(dispatcher)

    @After fun tearDown() = Dispatchers.resetMain()

    /** A source that supplies an empty transcript and never streams (reduce is driven directly). */
    private class StubSource(
        private val seededMessages: List<Message> = emptyList(),
    ) : ConversationSource {
        var closed = false
        override fun initialMessages(): List<Message> = seededMessages
        override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()
        override fun close() {
            closed = true
        }
    }

    /**
     * A source that records every [submit]/[cancel] and never terminates its
     * stream on its own — so the ViewModel stays in the streaming state, letting
     * the overlapping-submit guard and the cancel reset be asserted.
     */
    private class RecordingSource : ConversationSource {
        val submitted = mutableListOf<String>()
        val pending = mutableListOf<String>()
        val commands = mutableListOf<ClientCommand>()
        val cancelledTurnIds = mutableListOf<Long?>()
        var submittedTurnId: Long? = null
        var cancelCount = 0
        private val never = MutableSharedFlow<ReplyEvent>()
        override fun initialMessages(): List<Message> = emptyList()
        override fun submit(text: String): Flow<ReplyEvent> {
            submitted += text
            return never.asSharedFlow() // a turn that streams forever until cancelled
        }
        override suspend fun submitClientCommand(command: ClientCommand) {
            commands += command
            if (command is ClientCommand.SendPrompt) pending += command.text
        }
        override fun submit(
            text: String,
            images: List<ImageRefDto>,
            turnId: Long,
        ): Flow<ReplyEvent> {
            submittedTurnId = turnId
            return submit(text)
        }
        override suspend fun cancel(turnId: Long?) {
            cancelCount++
            cancelledTurnIds += turnId
        }
    }

    private class RecordingBackgroundExecution : ConversationBackgroundExecution {
        val activeStates = mutableListOf<Boolean>()

        override fun setTurnActive(active: Boolean) {
            if (activeStates.lastOrNull() != active) activeStates += active
        }
    }

    /**
     * Records session-control ordering and lets a test hold engine cancellation
     * open. Session switches must not submit Resume/New until this gate opens.
     */
    private class SessionControlSource(
        private val cancelGate: CompletableDeferred<Unit> = CompletableDeferred(Unit),
        private val resumeFailure: Throwable? = null,
    ) : ConversationSource {
        val operations = mutableListOf<String>()
        val active = MutableStateFlow<ActivatedSession?>(null)
        private val never = MutableSharedFlow<ReplyEvent>()
        var submitCount = 0
        var closed = false

        override fun submit(text: String): Flow<ReplyEvent> {
            submitCount++
            return never.asSharedFlow()
        }
        override val activeSessionState = active.asStateFlow()

        override suspend fun cancel() {
            operations += "cancel"
            cancelGate.await()
        }

        override suspend fun resumeSession(uuid: String) {
            operations += "resume:$uuid"
            resumeFailure?.let { throw it }
        }

        override suspend fun resumeEmptySession(uuid: String, title: String) {
            operations += "resume-empty:$uuid"
            resumeFailure?.let { throw it }
        }

        override suspend fun newSession() {
            operations += "new"
        }

        override fun close() {
            closed = true
        }
    }

    /**
     * A source whose reply stream is a hot [MutableSharedFlow] the test drives by
     * hand — so a turn can be left mid-stream, the session switched, and a STALE
     * event then pushed to prove the orphaned-turn guard drops it.
     */
    private class EmittingSource(
        private val initial: List<Message> = emptyList(),
    ) : ConversationSource {
        val stream = MutableSharedFlow<ReplyEvent>(extraBufferCapacity = 16)
        val active = MutableStateFlow<ActivatedSession?>(null)
        var cancelCount = 0
        override fun initialMessages(): List<Message> = initial
        override fun submit(text: String): Flow<ReplyEvent> = stream.asSharedFlow()
        override val activeSessionState = active.asStateFlow()
        override suspend fun cancel() { cancelCount++ }
        override suspend fun resumeSession(uuid: String) {
            active.value = ActivatedSession(uuid, emptyList(), SessionActivationKind.Resumed)
        }
        override suspend fun newSession() {
            active.value = ActivatedSession("new-engine", emptyList(), SessionActivationKind.Started)
        }
    }

    private class DurableAttachSource : ConversationSource {
        val active = MutableStateFlow<ActivatedSession?>(null)
        val attachRequests = mutableListOf<Long>()
        val submitted = mutableListOf<String>()
        val discarded = mutableListOf<Long>()

        override val activeSessionState = active.asStateFlow()
        override fun submit(text: String): Flow<ReplyEvent> {
            submitted += text
            return emptyFlow()
        }

        override suspend fun attachDurableTurnForUi(afterSequence: Long) {
            attachRequests += afterSequence
        }

        override suspend fun discardDurableTurn(turnId: Long) {
            discarded += turnId
        }
    }

    private class FailingDurableAttachSource : ConversationSource {
        val active = MutableStateFlow<ActivatedSession?>(null)
        val events = MutableSharedFlow<ClientEvent>(extraBufferCapacity = 16)
        val discarded = mutableListOf<Long>()
        val submitted = mutableListOf<String>()
        val sessionOperations = mutableListOf<String>()

        override val activeSessionState = active.asStateFlow()
        override val clientEvents: Flow<ClientEvent> = events.asSharedFlow()
        override fun submit(text: String): Flow<ReplyEvent> {
            submitted += text
            return emptyFlow()
        }

        override suspend fun attachDurableTurnForUi(afterSequence: Long) {
            val paused = TurnRecoverySnapshotDto(
                sessionId = "session-a",
                turnId = 94u,
                state = TurnRecoveryStateDto.PAUSED_RECOVERABLE,
                firstSequence = 0u,
                lastSequence = 1u,
                safeToResume = true,
                reason = "paused",
            )
            events.tryEmit(ClientEvent.TurnRecoveryState(paused))
            events.tryEmit(
                ClientEvent.TurnEventReplay(
                    sessionId = "session-a",
                    turnId = 94u,
                    sequence = 1u,
                    eventJson = """{"type":"tool_use_started","id":"read-1","tool":"Read","input_json":"{}"}""",
                ),
            )
            throw DurableAttachFailure(
                turnId = 94L,
                phase = "resume",
                cause = IllegalStateException("resume failed"),
            )
        }

        override suspend fun discardDurableTurn(turnId: Long) {
            discarded += turnId
        }

        override suspend fun resumeSession(uuid: String) {
            sessionOperations += "resume:$uuid"
        }

        override suspend fun newSession() {
            sessionOperations += "new"
        }
    }

    private class CloseTrackingSource : ConversationSource {
        var closeCount = 0
        var newSessionCount = 0
        override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()
        override suspend fun newSession() {
            newSessionCount++
        }
        override fun close() {
            closeCount++
        }
    }

    private fun newVm() = ChatViewModel(StubSource())

    @Test
    fun compactSlashUsesDedicatedCommandAndCliProgressThenSettles() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)

        vm.send("/compact")
        runCurrent()

        assertTrue(source.commands.single() is ClientCommand.ForceCompact)
        assertEquals("/compact", vm.state.value.messages.single().text)
        assertEquals(CompactionProgressStatus.Running, vm.state.value.compaction?.status)
        assertTrue(vm.state.value.requiresBackgroundExecution)
        assertEquals(0, compactProgressPercent(0))
        assertEquals(4, compactProgressPercent(4_000))
        assertEquals(63, compactProgressPercent(90_000))
        assertEquals(95, compactProgressPercent(10_000_000))
        vm.send("must not overlap compaction")
        assertEquals(1, vm.state.value.messages.size)

        vm.reduceClientEvent(ClientEvent.CompactionCompleted(20u, 7u, 4096u, "kept context"))
        val completed = vm.state.value.compaction
        assertEquals(CompactionProgressStatus.Completed, completed?.status)
        assertEquals(20, completed?.messagesBefore)
        assertEquals(7, completed?.messagesAfter)
        assertEquals(4096L, completed?.bytesSaved)
        assertFalse(vm.state.value.requiresBackgroundExecution)
    }

    @Test
    fun compactCommandFailureSettlesTheVisibleProgress() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)
        vm.send("/compact")
        runCurrent()

        vm.reduceClientEvent(ClientEvent.Error(
            kind = com.lingxi.code.bindings.ErrorKindDto.INTERNAL,
            message = "force_compact failed: handle action failed: rate limited",
        ))

        assertEquals(CompactionProgressStatus.Failed, vm.state.value.compaction?.status)
        assertEquals("rate limited", vm.state.value.compaction?.detail)
    }

    @Test
    fun activeTurnHoldsBackgroundExecutionUntilTerminalEvent() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(
            source = RecordingSource(),
            backgroundExecution = execution,
        )

        vm.send("keep running")
        runCurrent()
        assertEquals(listOf(true), execution.activeStates)

        vm.reduce(ReplyEvent.End)
        runCurrent()
        assertEquals(listOf(true, false), execution.activeStates)
    }

    @Test
    fun backgroundTaskKeepsExecutionLeaseAfterTurnEnds() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(
            source = RecordingSource(),
            backgroundExecution = execution,
        )

        vm.send("start an asynchronous task")
        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.RUNNING, null))
        vm.reduce(ReplyEvent.End)
        runCurrent()

        assertEquals(listOf(true), execution.activeStates)

        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.COMPLETED, null))
        runCurrent()
        assertEquals(listOf(true, false), execution.activeStates)
    }

    @Test
    fun durableAttachReplaysHistoryThenRendersLiveEventsOnce() = runTest(dispatcher) {
        val source = DurableAttachSource()
        val vm = ChatViewModel(source)
        source.active.value = ActivatedSession(
            "session-a",
            listOf(Message(Role.User, "prior"), Message(Role.Ai, "prior answer")),
            SessionActivationKind.Resumed,
        )
        runCurrent()
        val running = TurnRecoverySnapshotDto(
            sessionId = "session-a",
            turnId = 88u,
            state = TurnRecoveryStateDto.RUNNING,
            firstSequence = 1u,
            lastSequence = 1u,
            safeToResume = true,
            reason = null,
        )

        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(running))
        vm.reduceClientEvent(
            ClientEvent.TurnEventReplay(
                sessionId = "session-a",
                turnId = 88u,
                sequence = 1u,
                eventJson = """{"type":"text_delta","text":"history"}""",
            ),
        )
        // ResumeTurn emits its own state and closes the attach replay window.
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(running))
        vm.reduceClientEvent(ClientEvent.TextDelta("live"))
        vm.reduceClientEvent(
            ClientEvent.TurnEventReplay(
                sessionId = "session-a",
                turnId = 88u,
                sequence = 2u,
                eventJson = """{"type":"text_delta","text":"live"}""",
            ),
        )

        assertEquals(listOf(0L), source.attachRequests)
        assertEquals("historylive", vm.state.value.streamingMessage?.text)
    }

    @Test
    fun coldResumeUsesTerminalTranscriptInsteadOfProjectingCheckpointReplay() =
        runTest(dispatcher) {
            val source = DurableAttachSource()
            val restored = listOf(
                Message(Role.User, "question"),
                Message(Role.Ai, "authoritative assistant result"),
            )
            val vm = ChatViewModel(source)
            source.active.value = ActivatedSession(
                sessionId = "session-a",
                transcript = restored,
                kind = SessionActivationKind.Resumed,
            )
            runCurrent()

            val terminal = TurnRecoverySnapshotDto(
                sessionId = "session-a",
                turnId = 88u,
                state = TurnRecoveryStateDto.COMPLETED,
                firstSequence = 1u,
                lastSequence = 3u,
                safeToResume = false,
                reason = null,
            )
            vm.reduceClientEvent(ClientEvent.TurnRecoveryState(terminal))
            // These are the retained terminal checkpoints that can race past
            // the source-side replay gate after a process restart.  Neither
            // assistant prose nor a tool payload may be projected again.
            vm.reduceClientEvent(
                ClientEvent.TurnEventReplay(
                    sessionId = "session-a",
                    turnId = 88u,
                    sequence = 1u,
                    eventJson = """{"type":"text_delta","text":"duplicate assistant"}""",
                ),
            )
            vm.reduceClientEvent(
                ClientEvent.TurnEventReplay(
                    sessionId = "session-a",
                    turnId = 88u,
                    sequence = 2u,
                    eventJson = """{"type":"tool_use_started","id":"tool-1","tool":"Read","input_json":"{}"}""",
                ),
            )
            vm.reduceClientEvent(
                ClientEvent.TurnEventReplay(
                    sessionId = "session-a",
                    turnId = 88u,
                    sequence = 3u,
                    eventJson = """{"type":"tool_use_result","id":"tool-1","tool":"Read","result_json":"{}","is_error":false}""",
                ),
            )
            vm.reduceClientEvent(ClientEvent.TurnRecoveryState(terminal))

            assertEquals(restored, vm.state.value.messages)
            assertNull(vm.state.value.streamingMessage)
            assertFalse(vm.state.value.streaming)
            assertNull(vm.state.value.agentRun)
            assertEquals(0L, vm.durableReplayCursorForTesting())
        }

    @Test
    fun durableReplayCursorAdvancesWithinOneViewModelButResetsForANewOwner() = runTest(dispatcher) {
        val source = DurableAttachSource()
        val vm = ChatViewModel(source)
        source.active.value = ActivatedSession(
            "session-a",
            emptyList(),
            SessionActivationKind.Resumed,
        )
        runCurrent()
        val running = TurnRecoverySnapshotDto(
            sessionId = "session-a",
            turnId = 88u,
            state = TurnRecoveryStateDto.RUNNING,
            firstSequence = 1u,
            lastSequence = 1u,
            safeToResume = true,
            reason = null,
        )

        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(running))
        vm.reduceClientEvent(
            ClientEvent.TurnEventReplay(
                sessionId = "session-a",
                turnId = 88u,
                sequence = 1u,
                eventJson = """{"type":"text_delta","text":"history"}""",
            ),
        )
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(running))
        vm.reduceClientEvent(ClientEvent.TextDelta("live"))
        vm.reduceClientEvent(
            ClientEvent.TurnEventReplay(
                sessionId = "session-a",
                turnId = 88u,
                sequence = 2u,
                eventJson = """{"type":"text_delta","text":"live"}""",
            ),
        )
        val replacementVm = ChatViewModel(source)
        runCurrent()

        assertEquals(2L, vm.durableReplayCursorForTesting())
        assertEquals(0L, replacementVm.durableReplayCursorForTesting())
        assertEquals(listOf(0L, 0L), source.attachRequests)
        assertEquals("historylive", vm.state.value.streamingMessage?.text)
        assertNull(replacementVm.state.value.streamingMessage)
    }

    @Test
    fun notificationStopCancelsAReattachedTurnWithoutALocalCollector() = runTest(dispatcher) {
        val source = object : ConversationSource {
            val active = MutableStateFlow<ActivatedSession?>(null)
            val cancelled = mutableListOf<Long?>()
            override val activeSessionState = active.asStateFlow()
            override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()
            override suspend fun cancel(turnId: Long?) {
                cancelled += turnId
            }
        }
        val vm = ChatViewModel(source)
        source.active.value = ActivatedSession(
            "session-a",
            emptyList(),
            SessionActivationKind.Resumed,
        )
        runCurrent()
        vm.reduceClientEvent(
            ClientEvent.TurnRecoveryState(
                TurnRecoverySnapshotDto(
                    sessionId = "session-a",
                    turnId = 91u,
                    state = TurnRecoveryStateDto.RUNNING,
                    firstSequence = 0u,
                    lastSequence = 0u,
                    safeToResume = true,
                    reason = null,
                ),
            ),
        )

        vm.cancelFromSystem(91L)
        runCurrent()

        assertEquals(listOf(91L), source.cancelled)
        assertFalse(vm.state.value.streaming)
    }

    @Test
    fun inactiveWaitingCheckpointBlocksNewTurnsAndSessionsUntilCorrelatedDiscardTerminal() =
        runTest(dispatcher) {
            val source = DurableAttachSource()
            val vm = ChatViewModel(source)
            source.active.value = ActivatedSession(
                "session-a",
                emptyList(),
                SessionActivationKind.Resumed,
            )
            runCurrent()

            val waiting = TurnRecoverySnapshotDto(
                sessionId = "session-a",
                turnId = 92u,
                state = TurnRecoveryStateDto.WAITING_FOR_USER,
                firstSequence = 0u,
                lastSequence = 0u,
                safeToResume = false,
                reason = "waiting_for_user",
            )
            vm.reduceClientEvent(ClientEvent.TurnRecoveryState(waiting))
            assertTrue(vm.state.value.durableRecoveryBlocked)
            assertFalse(vm.state.value.streaming)

            // The checkpoint has no local executor. Prompt/session transitions
            // must not replace its durable identity before a correlated discard.
            vm.send("must remain parked")
            vm.openSession(SessionRef("session-b", "B"))
            vm.newChat()
            assertTrue(source.submitted.isEmpty())
            assertEquals("session-a", vm.state.value.session.id)

            // The foreground composer routes its Stop/Discard affordance through
            // the existing cancel callback; it must reach the correlated discard
            // path even though this is not active streaming work.
            vm.cancel()
            runCurrent()
            assertEquals(listOf(92L), source.discarded)
            vm.send("still parked until terminal")
            assertTrue(source.submitted.isEmpty())

            vm.reduceClientEvent(
                ClientEvent.TurnRecoveryState(
                    waiting.copy(state = TurnRecoveryStateDto.CANCELLED),
                ),
            )
            vm.send("after discard")
            runCurrent()
            assertEquals(listOf("after discard"), source.submitted)
        }

    @Test
    fun liveWaitingExecutorKeepsPromptAndSessionTransitionsGatedUntilNormalStop() =
        runTest(dispatcher) {
            val source = RecordingSource()
            val execution = RecordingBackgroundExecution()
            val vm = ChatViewModel(source, backgroundExecution = execution)
            vm.applyActivatedSession(
                ActivatedSession("session-a", emptyList(), SessionActivationKind.Started),
            )
            vm.send("first prompt")
            runCurrent()
            val liveTurnId = source.submittedTurnId ?: error("send did not reach source")

            vm.reduceClientEvent(
                ClientEvent.TurnRecoveryState(
                    TurnRecoverySnapshotDto(
                        sessionId = "session-a",
                        turnId = liveTurnId.toULong(),
                        state = TurnRecoveryStateDto.WAITING_FOR_USER,
                        firstSequence = 0u,
                        lastSequence = 0u,
                        safeToResume = false,
                        reason = "waiting_for_user",
                    ),
                ),
            )

            assertFalse(vm.state.value.streaming)
            assertTrue(vm.state.value.liveTurnWaitingForUser)
            assertTrue(vm.state.value.isStreaming) // Composer keeps ordinary Stop visible.
            assertTrue(vm.state.value.requiresBackgroundExecution)

            vm.send("must remain gated")
            vm.openSession(SessionRef("session-b", "B"))
            vm.newChat()
            assertEquals(listOf("first prompt"), source.submitted)
            assertEquals("session-a", vm.state.value.session.id)

            // Live WaitingForUser owns a local collector, so Stop sends the
            // ordinary Cancel(turn id), not the recovered Discard path.
            vm.cancel()
            runCurrent()
            assertEquals(listOf(liveTurnId), source.cancelledTurnIds)
            assertFalse(vm.state.value.liveTurnWaitingForUser)
            assertFalse(vm.state.value.requiresBackgroundExecution)
            assertEquals(listOf(true, false), execution.activeStates)
        }

    @Test
    fun inactiveRecoverySettlesOnlyCorrelatedRunAndShellLease() = runTest(dispatcher) {
        val source = DurableAttachSource()
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(source, backgroundExecution = execution)
        vm.applyActivatedSession(
            ActivatedSession("session-a", emptyList(), SessionActivationKind.Started),
        )
        vm.reduceClientEvent(
            ClientEvent.TaskRow(
                TaskRowDto("workflow-1", "workflow", TaskStatusDto.RUNNING, "review", false, null),
            ),
        )
        val waiting = TurnRecoverySnapshotDto(
            sessionId = "session-a",
            turnId = 93u,
            state = TurnRecoveryStateDto.WAITING_FOR_USER,
            firstSequence = 0u,
            lastSequence = 1u,
            safeToResume = false,
            reason = "waiting_for_user",
        )
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(waiting))
        vm.reduceClientEvent(
            ClientEvent.TurnEventReplay(
                sessionId = "session-a",
                turnId = 93u,
                sequence = 1u,
                eventJson = """{"type":"tool_use_started","id":"shell-1","tool":"shell","input_json":"{\"command\":\"npm test\"}"}""",
            ),
        )
        assertTrue(vm.state.value.requiresBackgroundExecution)

        // ResumeTurn's state closes the retained replay window. It must settle
        // the recovered main run/tool, but keep an unrelated workflow lease.
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(waiting))

        assertTrue(vm.state.value.durableRecoveryBlocked)
        assertFalse(vm.state.value.agentRun?.active == true)
        assertTrue(vm.state.value.agentRun?.tools.orEmpty().all { it.status != AgentToolStatus.Running })
        assertTrue(vm.state.value.activeBackgroundTaskIds.contains("workflow-1"))
        assertTrue(vm.state.value.requiresBackgroundExecution)
        // The remaining lease is the unrelated workflow task, not the parked
        // recovered turn's unmatched tool/run state.
        vm.reduceClientEvent(
            ClientEvent.TaskStatusChanged("workflow-1", TaskStatusDto.PAUSED, null),
        )
        runCurrent()
        assertFalse(vm.state.value.requiresBackgroundExecution)
        assertEquals(listOf(true, false), execution.activeStates)
    }

    @Test
    fun failedColdResumeKeepsPausedCheckpointIdentityAndGatesNewWork() = runTest(dispatcher) {
        val source = FailingDurableAttachSource()
        val vm = ChatViewModel(source)
        runCurrent()
        source.active.value = ActivatedSession(
            "session-a",
            emptyList(),
            SessionActivationKind.Resumed,
        )
        runCurrent()

        assertTrue(vm.state.value.durableRecoveryBlocked)
        assertFalse(vm.state.value.streaming)
        assertTrue(vm.state.value.error?.message?.contains("resume failed") == true)

        vm.send("must remain gated")
        vm.openSession(SessionRef("session-b", "B"))
        vm.newChat()
        assertTrue(source.sessionOperations.isEmpty())
        assertTrue(source.discarded.isEmpty())

        // The failed ResumeTurn did not lose its correlated durable id; the
        // only available foreground action is still Discard(94).
        vm.cancel()
        runCurrent()
        assertEquals(listOf(94L), source.discarded)
        assertEquals("session-a", vm.state.value.session.id)

        // A matching terminal clears the durable identity even though the
        // failed Resume left no render token. The next ordinary send is now
        // allowed and cannot overwrite the still-blocked checkpoint.
        vm.reduceClientEvent(
            ClientEvent.TurnRecoveryState(
                TurnRecoverySnapshotDto(
                    sessionId = "session-a",
                    turnId = 94u,
                    state = TurnRecoveryStateDto.CANCELLED,
                    firstSequence = 0u,
                    lastSequence = 1u,
                    safeToResume = false,
                    reason = null,
                ),
            ),
        )
        assertFalse(vm.state.value.durableRecoveryBlocked)
        vm.send("after discard")
        runCurrent()
        assertEquals(listOf("after discard"), source.submitted)
    }

    @Test
    fun failedColdResumeMatchingCompletedOrFailedClearsRecoveryGate() = runTest(dispatcher) {
        for (terminalState in listOf(
            TurnRecoveryStateDto.COMPLETED,
            TurnRecoveryStateDto.FAILED,
        )) {
            val source = FailingDurableAttachSource()
            val vm = ChatViewModel(source)
            runCurrent()
            source.active.value = ActivatedSession(
                "session-a",
                emptyList(),
                SessionActivationKind.Resumed,
            )
            runCurrent()
            assertTrue(vm.state.value.durableRecoveryBlocked)

            vm.cancel()
            runCurrent()
            vm.reduceClientEvent(
                ClientEvent.TurnRecoveryState(
                    TurnRecoverySnapshotDto(
                        sessionId = "session-a",
                        turnId = 94u,
                        state = terminalState,
                        firstSequence = 0u,
                        lastSequence = 1u,
                        safeToResume = false,
                        reason = "terminal",
                    ),
                ),
            )

            assertFalse("$terminalState must release durable recovery", vm.state.value.durableRecoveryBlocked)
            vm.send("after $terminalState")
            runCurrent()
            assertEquals(listOf("after $terminalState"), source.submitted)
        }
    }

    @Test
    fun pausedTaskStaysVisibleButReleasesBackgroundExecutionLease() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(
            source = RecordingSource(),
            backgroundExecution = execution,
        )

        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.RUNNING, null))
        runCurrent()
        assertEquals(listOf(true), execution.activeStates)

        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.PAUSED, null))
        runCurrent()

        assertFalse(vm.state.value.activeBackgroundTaskIds.contains("task-1"))
        assertEquals("后台任务 task-1 已暂停", vm.state.value.statusLine)
        assertEquals(listOf(true, false), execution.activeStates)
    }

    @Test
    fun taskStatusFromAnotherSessionIsIgnoredWithoutLeakingIntoVisibleStatus() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(
            source = RecordingSource(),
            backgroundExecution = execution,
        )
        vm.applyActivatedSession(
            ActivatedSession("session-a", emptyList(), SessionActivationKind.Started),
        )

        vm.reduceClientEvent(
            ClientEvent.TaskStatusChanged(
                taskId = "task-b",
                status = TaskStatusDto.RUNNING,
                originSessionId = "session-b",
            ),
        )
        runCurrent()

        assertNull(vm.state.value.statusLine)
        assertFalse(vm.state.value.activeBackgroundTaskIds.contains("task-b"))
        assertEquals(emptyList<Boolean>(), execution.activeStates)

        vm.reduceClientEvent(
            ClientEvent.TaskStatusChanged(
                taskId = "task-a",
                status = TaskStatusDto.RUNNING,
                originSessionId = "session-a",
            ),
        )
        assertEquals("后台任务 task-a 运行中", vm.state.value.statusLine)
    }

    @Test
    fun backgroundTaskLeaseIsReleasedWhenSwitchingSessions() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val source = EmittingSource()
        val vm = ChatViewModel(
            source = source,
            backgroundExecution = execution,
        )

        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.RUNNING, null))
        runCurrent()
        assertEquals(listOf(true), execution.activeStates)

        vm.openSession(SessionRef(id = "B", title = "B"))
        runCurrent()
        assertFalse(vm.state.value.activeBackgroundTaskIds.contains("task-1"))
        assertEquals(
            "session-scoped work must release its lease on a conversation switch",
            listOf(true, false),
            execution.activeStates,
        )

        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.COMPLETED, null))
        runCurrent()
        assertEquals(listOf(true, false), execution.activeStates)
    }

    @Test
    fun taskRowBootstrapKeepsExecutionLeaseUntilTerminalRow() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(
            source = RecordingSource(),
            backgroundExecution = execution,
        )

        vm.reduceClientEvent(
            ClientEvent.TaskRow(
                TaskRowDto("task-1", "shell", TaskStatusDto.RUNNING, "npm test", false, null),
            ),
        )
        runCurrent()
        assertEquals(listOf(true), execution.activeStates)

        vm.reduceClientEvent(
            ClientEvent.TaskRow(
                TaskRowDto("task-1", "shell", TaskStatusDto.COMPLETED, "npm test", false, null),
            ),
        )
        runCurrent()
        assertEquals(listOf(true, false), execution.activeStates)
    }

    @Test
    fun shellAndWorkerKeepExecutionLeaseAfterTurnEnds() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(
            source = RecordingSource(),
            backgroundExecution = execution,
        )

        vm.send("run in parallel")
        vm.reduce(ReplyEvent.Coordinator(activeWorkers = 1, team = "review"))
        vm.reduce(
            ReplyEvent.ShellTool(
                ShellToolUpdate.Started("shell-1", "npm test", null),
            ),
        )
        vm.reduce(ReplyEvent.End)
        runCurrent()
        assertEquals(listOf(true), execution.activeStates)

        vm.reduce(ReplyEvent.Coordinator(activeWorkers = 0, team = "review"))
        vm.reduce(
            ReplyEvent.ShellTool(
                ShellToolUpdate.Finished(
                    taskId = "shell-1",
                    stdout = "ok",
                    stderr = "",
                    exitCode = 0,
                    elapsedMs = 10,
                    status = ShellToolStatus.Completed,
                    truncated = false,
                ),
            ),
        )
        runCurrent()
        assertEquals(listOf(true, false), execution.activeStates)
    }

    @Test
    fun workerKeepsExecutionLeaseAfterTurnEndsUntilCoordinatorReportsIdle() =
        runTest(dispatcher) {
            val execution = RecordingBackgroundExecution()
            val vm = ChatViewModel(
                source = RecordingSource(),
                backgroundExecution = execution,
            )

            vm.send("delegate work")
            vm.reduce(ReplyEvent.Coordinator(activeWorkers = 1, team = "review"))
            vm.reduce(ReplyEvent.End)
            runCurrent()
            assertEquals(listOf(true), execution.activeStates)

            // Coordinator transitions are also delivered on the source's
            // out-of-band event feed after the per-turn ReplyEvent flow ended.
            vm.reduceClientEvent(ClientEvent.CoordinatorStatus(activeWorkers = 0u, team = "review"))
            runCurrent()
            assertEquals(listOf(true, false), execution.activeStates)
            assertTrue(vm.state.value.agentRunsByMessageId.values.all { it.activeWorkers == 0 })
        }

    @Test
    fun coordinatorIdleClearsWorkersFromAnEarlierTurn() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(
            source = RecordingSource(),
            backgroundExecution = execution,
        )

        vm.send("delegate work")
        vm.reduce(ReplyEvent.Coordinator(activeWorkers = 1, team = "review"))
        vm.reduce(ReplyEvent.End)
        vm.send("follow up")
        vm.reduceClientEvent(ClientEvent.CoordinatorStatus(activeWorkers = 0u, team = "review"))
        vm.reduce(ReplyEvent.End)
        runCurrent()

        assertTrue(vm.state.value.agentRunsByMessageId.values.all { it.activeWorkers == 0 })
        assertEquals(listOf(true, false), execution.activeStates)
    }

    // --- delta accumulation ----------------------------------------------

    @Test
    fun firstDelta_opensAssistantMessage_andSetsStreaming() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Delta("Hel"))

        val s = vm.state.value
        assertTrue(s.streaming)
        assertEquals(Role.Ai, s.streamingMessage?.role)
        assertEquals("Hel", s.streamingMessage?.text)
    }

    @Test
    fun subsequentDeltas_appendIntoSameMessage() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Delta("Hel"))
        val unchangedTranscript = vm.state.value.messages
        val streamingId = vm.state.value.streamingMessage?.id
        vm.reduce(ReplyEvent.Delta("lo "))
        vm.reduce(ReplyEvent.Delta("world"))

        val s = vm.state.value
        assertSame(unchangedTranscript, s.messages)
        assertEquals(streamingId, s.streamingMessage?.id)
        assertEquals("Hello world", s.streamingMessage?.text)
        assertTrue(s.streaming)
    }

    @Test
    fun largeTranscript_deltasDoNotCopyCompletedMessages() {
        val history = List(10_000) { index ->
            Message(
                role = if (index % 2 == 0) Role.User else Role.Ai,
                text = "message-$index",
            )
        }
        val vm = ChatViewModel(StubSource(history))

        vm.reduce(ReplyEvent.Delta("a"))
        val transcript = vm.state.value.messages
        val streamingId = vm.state.value.streamingMessage?.id
        repeat(100) { vm.reduce(ReplyEvent.Delta("b")) }

        val state = vm.state.value
        assertSame(history, transcript)
        assertSame(transcript, state.messages)
        assertEquals(streamingId, state.streamingMessage?.id)
        assertEquals(101, state.streamingMessage?.text?.length)
    }

    @Test
    fun completedMessageKeepsStreamingRowIdentity() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Delta("draft"))
        val streamingId = vm.state.value.streamingMessage?.id

        vm.reduce(ReplyEvent.Completed(Message(role = Role.Ai, text = "final")))

        val state = vm.state.value
        assertNull(state.streamingMessage)
        assertEquals(streamingId, state.messages.last().id)
        assertEquals("final", state.messages.last().text)
    }

    @Test
    fun thinking_setsStreaming_withoutAddingMessage() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Thinking)
        val s = vm.state.value
        assertTrue(s.streaming)
        assertTrue(s.messages.isEmpty())
        assertTrue(s.agentRun!!.reasoningActive)
    }

    @Test
    fun reasoningDelta_accumulatesInTransientRunTrace() {
        val vm = newVm()
        vm.reduce(ReplyEvent.ReasoningDelta("先检查"))
        vm.reduce(ReplyEvent.ReasoningDelta("项目结构"))

        val run = vm.state.value.agentRun!!
        assertEquals("先检查项目结构", run.reasoning)
        assertTrue(run.reasoningActive)
        assertTrue(vm.state.value.messages.isEmpty())
    }

    // --- status line ------------------------------------------------------

    @Test
    fun toolActivity_setsStatusLine() {
        val vm = newVm()
        vm.reduce(ReplyEvent.ToolActivity("调用工具 bash…"))
        assertEquals("调用工具 bash…", vm.state.value.statusLine)
    }

    @Test
    fun correlatedToolActivity_updatesSingleTimelineRow() {
        val vm = newVm()
        vm.reduce(
            ReplyEvent.ToolActivity(
                label = "调用工具 Read…",
                id = "tool-1",
                tool = "Read",
                status = AgentToolStatus.Running,
                inputSummary = "/workspace/index.html",
            ),
        )
        vm.reduce(
            ReplyEvent.ToolActivity(
                label = "工具 Read 完成",
                id = "tool-1",
                tool = "Read",
                status = AgentToolStatus.Completed,
                elapsedMs = 120,
            ),
        )

        val tools = vm.state.value.agentRun!!.tools
        assertEquals(1, tools.size)
        assertEquals("/workspace/index.html", tools.single().summary)
        assertEquals(AgentToolStatus.Completed, tools.single().status)
        assertEquals(120L, tools.single().elapsedMs)
    }

    @Test
    fun usageRetryAndCost_areVisibleInTimeline() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Usage(AgentRunUsage(100, 20, 50, 0)))
        vm.reduce(ReplyEvent.Retry("rate limited", 2, 5, 1_000))
        vm.reduce(ReplyEvent.Cost("$0.0042"))

        val run = vm.state.value.agentRun!!
        assertEquals(100, run.usage!!.inputTokens)
        assertEquals("$0.0042", run.formattedCost)
        assertTrue(run.notices.single().text.contains("2/5"))
    }

    @Test
    fun error_setsPersistentBanner_clearsStreaming_andStatusLine() {
        val vm = newVm()
        vm.reduce(ReplyEvent.ToolActivity("调用工具 bash…"))
        vm.reduce(ReplyEvent.Delta("partial"))
        vm.reduce(ReplyEvent.Error("kaboom"))

        val s = vm.state.value
        assertFalse(s.streaming)
        // Error surfaces in the PERSISTENT banner, not the dim status line.
        assertNull("statusLine cleared on error", s.statusLine)
        assertEquals("kaboom", s.error!!.message)
        assertEquals(AgentRunOutcome.Failed, s.agentRun!!.outcome)
    }

    @Test
    fun trailingEnd_afterError_doesNotRewriteFailureAsSuccess() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Error("kaboom"))
        vm.reduce(ReplyEvent.End)

        assertEquals(AgentRunOutcome.Failed, vm.state.value.agentRun!!.outcome)
    }

    @Test
    fun error_isKindAware_authVsNetworkVsGeneric() {
        val auth = newVm().also { it.reduce(ReplyEvent.Error("HTTP 401 invalid api key")) }
        assertEquals(ChatErrorKind.AUTH, auth.state.value.error!!.kind)

        val net = newVm().also { it.reduce(ReplyEvent.Error("connection timed out")) }
        assertEquals(ChatErrorKind.NETWORK, net.state.value.error!!.kind)

        val generic = newVm().also { it.reduce(ReplyEvent.Error("something odd happened")) }
        assertEquals(ChatErrorKind.GENERIC, generic.state.value.error!!.kind)
    }

    @Test
    fun dismissError_clearsBanner() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Error("kaboom"))
        assertEquals("kaboom", vm.state.value.error!!.message)
        vm.dismissError()
        assertNull(vm.state.value.error)
    }

    // --- terminal ---------------------------------------------------------

    @Test
    fun end_clearsStreaming_keepsTranscript() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Delta("done"))
        vm.reduce(ReplyEvent.End)

        val s = vm.state.value
        assertFalse(s.streaming)
        assertEquals(1, s.messages.size)
        assertEquals("done", s.messages[0].text)
        assertEquals(AgentRunOutcome.Completed, s.agentRun!!.outcome)
    }

    @Test
    fun deltaAfterEnd_opensFreshMessage_notAppendToPrior() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Delta("turn1"))
        vm.reduce(ReplyEvent.End)
        // A new turn's first delta must NOT append into turn 1's message.
        vm.reduce(ReplyEvent.Delta("turn2"))

        val s = vm.state.value
        assertEquals(1, s.messages.size)
        assertEquals("turn1", s.messages[0].text)
        assertEquals("turn2", s.streamingMessage?.text)
    }

    @Test
    fun completed_appendsWholeMessage_clearsStreaming() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Thinking)
        vm.reduce(ReplyEvent.Completed(Message(role = Role.Ai, tag = "思考了 8 秒", text = "已记入。")))

        val s = vm.state.value
        assertFalse(s.streaming)
        assertEquals("已记入。", s.messages.last().text)
        assertEquals("思考了 8 秒", s.messages.last().tag)
    }

    @Test
    fun newChat_clearsStreamingMessage_soNextDeltaOpensFresh() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Delta("old turn"))
        vm.newChat()
        vm.reduce(ReplyEvent.Delta("brand new"))

        val s = vm.state.value
        assertTrue(s.isNew)
        assertTrue(s.messages.isEmpty())
        assertEquals("brand new", s.streamingMessage?.text)
        assertNull("statusLine cleared on newChat", ChatViewModel(StubSource()).state.value.statusLine)
    }

    // --- streaming + pending-message submission ----------------------------

    @Test
    fun send_setsStreaming_andIsStreaming() = runTest(dispatcher) {
        val src = RecordingSource()
        val vm = ChatViewModel(src)
        vm.send("hi")
        assertTrue(vm.state.value.streaming)
        assertTrue(vm.state.value.isStreaming)
        assertEquals(listOf("hi"), src.submitted)
        assertEquals(AgentRunOutcome.Running, vm.state.value.agentRun!!.outcome)
    }

    @Test
    fun send_whileStreaming_submitsPendingMessageWithoutReplacingCollector() = runTest(dispatcher) {
        val src = RecordingSource()
        val vm = ChatViewModel(src)
        vm.send("first")
        vm.send("second")
        runCurrent()

        assertEquals("only the first turn owns a reply collector", listOf("first"), src.submitted)
        assertEquals(listOf("second"), src.pending)
        assertEquals(2, vm.state.value.messages.count { it.role == Role.User })
        assertTrue(vm.state.value.streaming)
    }

    @Test
    fun send_clearsPriorError() = runTest(dispatcher) {
        val src = RecordingSource()
        val vm = ChatViewModel(src)
        vm.reduce(ReplyEvent.Error("boom"))
        assertEquals("boom", vm.state.value.error!!.message)
        vm.send("retry")
        assertNull("a fresh turn clears the prior error", vm.state.value.error)
    }

    // --- cancel -----------------------------------------------------------

    @Test
    fun cancel_firesSourceCancel_andResetsStreaming() = runTest(dispatcher) {
        val src = RecordingSource()
        val vm = ChatViewModel(src)
        vm.send("hi")
        assertTrue(vm.state.value.streaming)

        vm.cancel()
        assertEquals(1, src.cancelCount)
        assertFalse("streaming reset immediately on cancel", vm.state.value.streaming)
        assertFalse(vm.state.value.isStreaming)
        assertEquals(AgentRunOutcome.Cancelled, vm.state.value.agentRun!!.outcome)
    }

    @Test
    fun cancel_whenIdle_isNoOp() = runTest(dispatcher) {
        val src = RecordingSource()
        val vm = ChatViewModel(src)
        vm.cancel()
        assertEquals("no cancel sent when no turn in flight", 0, src.cancelCount)
    }

    @Test
    fun workspaceSwitchBuildFailureKeepsCurrentSourceAlive() = runTest(dispatcher) {
        val original = StubSource()
        val vm = ChatViewModel(original)

        val switched = vm.switchWorkspaceSource(
            projectId = "10000000-0000-4000-8000-000000000001",
            createSource = { UnavailableConversationSource("PRoot 授权阻塞") },
        )

        assertFalse(switched)
        assertFalse("the current source must survive replacement build failure", original.closed)
        assertNull(vm.sourceProjectId.value)
        assertTrue(vm.state.value.error!!.message.contains("PRoot 授权阻塞"))
    }

    @Test
    fun workspaceSwitchClosesOldSourceAndStartsRealNewSession() = runTest(dispatcher) {
        val original = StubSource()
        val replacement = SessionControlSource()
        val vm = ChatViewModel(original)
        val projectId = "10000000-0000-4000-8000-000000000001"

        val switched = vm.switchWorkspaceSource(
            projectId = projectId,
            createSource = { replacement },
        )
        runCurrent()

        assertTrue(switched)
        assertTrue(original.closed)
        assertEquals(projectId, vm.sourceProjectId.value)
        assertEquals(listOf("new"), replacement.operations)
    }

    @Test
    fun workspaceSwitchPersistenceFailureKeepsCurrentSourceAlive() = runTest(dispatcher) {
        val original = StubSource()
        val replacement = SessionControlSource()
        val vm = ChatViewModel(original)

        val switched = vm.switchWorkspaceSource(
            projectId = "10000000-0000-4000-8000-000000000001",
            createSource = { replacement },
            persistSelection = { error("injected index write failure") },
        )

        assertFalse(switched)
        assertFalse("the current source must survive persistence failure", original.closed)
        assertTrue("the uncommitted replacement must be released", replacement.closed)
        assertNull(vm.sourceProjectId.value)
        assertTrue(vm.state.value.error!!.message.contains("index write failure"))
    }

    @Test
    fun projectRecoveryReplacesWrongInitialResumeAfterProcessDeath() = runTest(dispatcher) {
        val savedState = SavedStateHandle(
            mapOf(
                "chat.session.id" to "project-session",
                "chat.session.title" to "项目会话",
                "chat.isNew" to false,
            ),
        )
        val wrongGlobalSource = SessionControlSource()
        val projectSource = SessionControlSource()
        val vm = ChatViewModel(wrongGlobalSource, savedState)
        runCurrent()

        assertTrue(vm.state.value.sessionTransitioning)
        assertEquals(listOf("resume:project-session"), wrongGlobalSource.operations)

        val switched = vm.switchWorkspaceSource(
            projectId = "10000000-0000-4000-8000-000000000001",
            target = SessionRef("project-session", "项目会话"),
            newSession = false,
            replacePendingTransition = true,
            createSource = { projectSource },
        )
        runCurrent()

        assertTrue(switched)
        assertTrue(wrongGlobalSource.closed)
        assertEquals(listOf("resume:project-session"), projectSource.operations)
        assertEquals(
            "10000000-0000-4000-8000-000000000001",
            vm.sourceProjectId.value,
        )
    }

    @Test
    fun send_afterCancel_isAllowed() = runTest(dispatcher) {
        val src = RecordingSource()
        val vm = ChatViewModel(src)
        vm.send("first")
        vm.cancel()
        // Once cancelled, a new turn must be accepted.
        vm.send("second")
        assertEquals(listOf("first", "second"), src.submitted)
    }

    // --- orphaned-turn guard (session switch mid-stream) ------------------

    @Test
    fun openSession_midStream_dropsStaleDelta_doesNotMutateNewSession() = runTest(dispatcher) {
        val src = EmittingSource()
        val vm = ChatViewModel(src)

        // Turn 1 begins streaming into session A.
        vm.send("hi from A")
        src.stream.emit(ReplyEvent.Delta("partial A"))
        assertTrue(vm.state.value.streaming)
        assertEquals("partial A", vm.state.value.streamingMessage?.text)

        // Switch to a DIFFERENT session mid-stream (the orphaned-turn scenario).
        val sessB = SessionRef(id = "B", title = "会话 B")
        vm.openSession(sessB)
        assertEquals(sessB, vm.state.value.session)
        assertFalse("switching sessions clears streaming", vm.state.value.streaming)
        val transcriptAfterSwitch = vm.state.value.messages

        // The OLD turn's coroutine is still alive for one more emission: a late
        // Delta + End from turn 1 must NOT land in session B's transcript.
        src.stream.emit(ReplyEvent.Delta(" LATE-LEAK"))
        src.stream.emit(ReplyEvent.End)

        assertEquals(
            "stale turn must not append to the new session",
            transcriptAfterSwitch,
            vm.state.value.messages,
        )
        assertFalse("stale End must not flip streaming back", vm.state.value.streaming)
    }

    @Test
    fun newChat_midStream_dropsStaleEnd_keepsFreshChatEmpty() = runTest(dispatcher) {
        val src = EmittingSource()
        val vm = ChatViewModel(src)

        vm.send("hi")
        src.stream.emit(ReplyEvent.Delta("streaming…"))
        vm.newChat()
        assertTrue(vm.state.value.isNew)
        assertTrue("a fresh chat starts empty", vm.state.value.messages.isEmpty())

        // Stale events from the abandoned turn arrive after the reset.
        src.stream.emit(ReplyEvent.Delta("ghost"))
        src.stream.emit(ReplyEvent.End)

        assertTrue("stale delta must not populate the new chat", vm.state.value.messages.isEmpty())
        assertFalse(vm.state.value.streaming)
    }

    @Test
    fun newTurnAfterSwitch_streamsNormally_intoNewSession() = runTest(dispatcher) {
        val src = EmittingSource()
        val vm = ChatViewModel(src)

        vm.send("A")
        src.stream.emit(ReplyEvent.Delta("a-reply"))
        vm.openSession(SessionRef(id = "B", title = "B"))

        // A genuinely new turn in session B must stream normally (the guard only
        // drops the SUPERSEDED turn, never the current one).
        vm.send("B")
        src.stream.emit(ReplyEvent.Delta("b-reply"))
        src.stream.emit(ReplyEvent.End)

        val ai = vm.state.value.messages.filter { it.role == Role.Ai }
        assertEquals(1, ai.size)
        assertEquals("b-reply", ai.single().text)
        assertFalse(vm.state.value.streaming)
    }

    @Test
    fun resumeSession_cancelsEngineTurnBeforeSubmittingResume() = runTest(dispatcher) {
        val gate = CompletableDeferred<Unit>()
        val src = SessionControlSource(cancelGate = gate)
        val vm = ChatViewModel(src)
        vm.send("still running")

        vm.resumeSession(SessionRow("B", "会话 B", 1, "刚刚"))
        runCurrent()

        assertEquals(listOf("cancel"), src.operations)
        assertTrue(vm.state.value.sessionTransitioning)
        assertFalse(vm.state.value.sessionReady)

        gate.complete(Unit)
        runCurrent()

        assertEquals(listOf("cancel", "resume:B"), src.operations)
        assertTrue("resume remains gated until SessionResumed arrives", vm.state.value.sessionTransitioning)

        src.active.value = ActivatedSession(
            sessionId = "B",
            transcript = listOf(Message(Role.User, "authoritative")),
            kind = SessionActivationKind.Resumed,
        )
        runCurrent()

        assertFalse(vm.state.value.sessionTransitioning)
        assertTrue(vm.state.value.sessionReady)
        assertEquals(listOf("authoritative"), vm.state.value.messages.map { it.text })
    }

    @Test
    fun newChat_cancelsEngineTurnBeforeSubmittingNewSession() = runTest(dispatcher) {
        val gate = CompletableDeferred<Unit>()
        val src = SessionControlSource(cancelGate = gate)
        val vm = ChatViewModel(src)
        vm.send("still running")

        vm.newChat()
        runCurrent()
        assertEquals(listOf("cancel"), src.operations)

        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf("cancel", "new"), src.operations)
        assertFalse(vm.state.value.sessionReady)
    }

    @Test
    fun resumeFailure_isVisibleAndKeepsComposerBlocked() = runTest(dispatcher) {
        val src = SessionControlSource(resumeFailure = IllegalStateException("session file missing"))
        val vm = ChatViewModel(src)

        vm.resumeSession(SessionRow("missing", "丢失会话", 0, "刚刚"))
        runCurrent()

        assertFalse(vm.state.value.sessionTransitioning)
        assertFalse(vm.state.value.sessionReady)
        assertTrue(vm.state.value.error!!.message.contains("session file missing"))

        vm.send("must not reach the wrong engine session")
        assertEquals(0, src.submitCount)
        assertTrue(vm.state.value.messages.isEmpty())
    }

    @Test
    fun zeroMessageSessionUsesExplicitEmptyResumeAndKeepsItsId() = runTest(dispatcher) {
        val src = SessionControlSource()
        val vm = ChatViewModel(src)
        val uuid = "19587a33-0725-48db-abca-8a2aed345f6b"

        vm.resumeSession(SessionRow(uuid, "空会话", 0, "刚刚"))
        runCurrent()

        assertEquals(listOf("resume-empty:$uuid"), src.operations)
        assertEquals(uuid, vm.state.value.session.id)
        assertTrue(vm.state.value.sessionTransitioning)
    }

    // --- SavedStateHandle persistence / restore --------------------------

    @Test
    fun savedState_resumesEngineBeforeShowingAuthoritativeTranscript() = runTest(dispatcher) {
        val handle = SavedStateHandle(
            mapOf(
                "chat.session.id" to "s-keep",
                "chat.session.title" to "保留会话",
                // Compatibility fixture from an older build. It must be removed
                // without decoding or displaying any of its cached messages.
                "chat.transcript" to arrayListOf("legacy cached transcript"),
                "chat.draft" to "half-typed",
                "chat.isNew" to false,
            ),
        )
        val src = SessionControlSource()
        val vm = ChatViewModel(src, handle)
        runCurrent()

        assertEquals(listOf("resume:s-keep"), src.operations)
        assertEquals("s-keep", vm.state.value.session.id)
        assertTrue("cached transcript must not impersonate engine context", vm.state.value.messages.isEmpty())
        assertTrue(vm.state.value.sessionTransitioning)
        assertFalse(vm.state.value.sessionReady)
        assertEquals("half-typed", vm.restoredDraft)
        assertFalse("legacy transcript key is migrated away", handle.contains("chat.transcript"))

        src.active.value = ActivatedSession(
            sessionId = "s-keep",
            transcript = listOf(
                Message(Role.User, "engine question"),
                Message(Role.Ai, "engine answer"),
            ),
            kind = SessionActivationKind.Resumed,
        )
        runCurrent()

        assertTrue(vm.state.value.sessionReady)
        assertFalse(vm.state.value.sessionTransitioning)
        assertEquals(listOf("engine question", "engine answer"), vm.state.value.messages.map { it.text })
        val restoredAnswer = vm.state.value.messages.last()
        assertEquals(
            AgentRunOutcome.Finished,
            vm.state.value.agentRunsByMessageId[restoredAnswer.id]?.outcome,
        )
    }

    @Test
    fun savedState_neverPersistsTranscript_evenAfterStreamingMessages() = runTest(dispatcher) {
        val handle = SavedStateHandle()
        val source = EmittingSource()
        val vm = ChatViewModel(source, handle)

        vm.send("question")
        source.stream.emit(ReplyEvent.Delta("answer"))
        source.stream.emit(ReplyEvent.End)
        runCurrent()

        assertEquals(listOf("question", "answer"), vm.state.value.messages.map { it.text })
        assertFalse("messages must stay out of Android's saved-state Bundle", handle.contains("chat.transcript"))
        assertEquals("new", handle.get<String>("chat.session.id"))
        assertEquals("新对话", handle.get<String>("chat.session.title"))
    }

    @Test
    fun savedState_clearsDraftOnSend() = runTest(dispatcher) {
        val handle = SavedStateHandle()
        val vm = ChatViewModel(RecordingSource(), handle)
        vm.onDraftChanged("about to send")
        assertEquals("about to send", vm.restoredDraft)
        vm.send("about to send")
        assertEquals("a sent draft is cleared from saved state", "", vm.restoredDraft)
    }

    @Test
    fun noSavedState_behavesAsBefore_restoredDraftEmpty() {
        // The reducer-test default (null handle) still starts from an explicit
        // empty "new chat" state with a blank restored draft.
        val vm = ChatViewModel(StubSource())
        assertEquals("", vm.restoredDraft)
        assertEquals(SessionRef(id = "new", title = "新对话"), vm.state.value.session)
        assertTrue(vm.state.value.isNew)
    }

    @Test
    fun sourceGeneration_survivesRecomposition_andReplacesOnlyOnReconnect() = runTest(dispatcher) {
        val original = CloseTrackingSource()
        val replacement = CloseTrackingSource()
        val vm = ChatViewModel(source = original, sourceGeneration = 7)
        var factoryCalls = 0

        vm.ensureSource(7) {
            factoryCalls++
            replacement
        }
        assertEquals("rotation with the same token must retain the live source", 0, factoryCalls)
        assertEquals(0, original.closeCount)

        vm.ensureSource(8) {
            factoryCalls++
            replacement
        }
        runCurrent()

        assertEquals(1, factoryCalls)
        assertEquals("reconnect must release the superseded native source", 1, original.closeCount)
        assertEquals("replacement must establish a real engine session", 1, replacement.newSessionCount)
    }

    @Test
    fun sourceGeneration_ignoresRestoredOlderToken() = runTest(dispatcher) {
        val original = CloseTrackingSource()
        val vm = ChatViewModel(source = original, sourceGeneration = 7)
        var factoryCalls = 0

        vm.ensureSource(0) {
            factoryCalls++
            CloseTrackingSource()
        }
        runCurrent()

        assertEquals(0, factoryCalls)
        assertEquals(0, original.closeCount)
    }

    @Test
    fun reconnectFailure_keepsWorkingSource_andSurfacesError() = runTest(dispatcher) {
        val original = CloseTrackingSource()
        val vm = ChatViewModel(source = original, sourceGeneration = 1)

        vm.ensureSource(2) {
            UnavailableConversationSource("native engine failed to boot")
        }
        runCurrent()

        assertEquals("failed replacement must not destroy the current engine", 0, original.closeCount)
        assertTrue(vm.state.value.sessionReady)
        assertFalse(vm.state.value.sessionTransitioning)
        assertTrue(vm.state.value.error!!.message.contains("failed to boot"))
    }

    // --- pre-derived tool presentation ------------------------------------

    @Test
    fun toolRow_keepsItsHeaderWhenTheResultArrives_andGainsTheDisplay() {
        // The header rides the CALL and the display rides the RESULT — two wire
        // events for one row. Neither may be erased by the other's null.
        val vm = newVm()
        vm.reduce(
            ReplyEvent.ToolActivity(
                label = "调用工具 Edit…",
                id = "e1",
                tool = "Edit",
                status = AgentToolStatus.Running,
                header = header(),
            ),
        )
        vm.reduce(
            ReplyEvent.ToolActivity(
                label = "工具 Edit 完成",
                id = "e1",
                tool = "Edit",
                status = AgentToolStatus.Completed,
                display = display(),
            ),
        )

        val row = vm.state.value.agentRun!!.tools.single()
        assertEquals("e1", row.id)
        assertEquals(AgentToolStatus.Completed, row.status)
        assertEquals(ToolVerbUi.Update, row.header?.verb)
        assertEquals(HeadlineKindUi.Added, row.display?.headlineKind)

        val call = row.toToolCall()
        assertEquals("e1", call.id)
        assertEquals(AgentToolStatus.Completed, call.status)
        assertEquals(ToolVerbUi.Update, call.header?.verb)
    }

    @Test
    fun toolHeartbeat_withoutHeader_doesNotEraseTheOneTheCallDelivered() {
        val vm = newVm()
        vm.reduce(
            ReplyEvent.ToolActivity(
                label = "调用工具 Edit…", id = "e1", tool = "Edit",
                status = AgentToolStatus.Running, header = header(),
            ),
        )
        vm.reduce(
            ReplyEvent.ToolActivity(
                label = "工具 Edit 运行中…", id = "e1", tool = "Edit",
                status = AgentToolStatus.Running, elapsedMs = 1_500L,
            ),
        )

        val row = vm.state.value.agentRun!!.tools.single()
        assertEquals(ToolVerbUi.Update, row.header?.verb)
        assertEquals(1_500L, row.elapsedMs)
    }

    @Test
    fun toggleToolCall_livesInTheViewModel_soRecycledRowsCannotLoseIt() {
        val vm = newVm()
        assertTrue(vm.state.value.expandedToolCalls.isEmpty())

        vm.toggleToolCall("e1")
        assertEquals(setOf("e1"), vm.state.value.expandedToolCalls)

        vm.toggleToolCall("e2")
        assertEquals(setOf("e1", "e2"), vm.state.value.expandedToolCalls)

        vm.toggleToolCall("e1")
        assertEquals(setOf("e2"), vm.state.value.expandedToolCalls)
    }

    @Test
    fun settledTurn_absorbsItsToolCalls_drivenThroughTheRealMessageCompletePath() = runTest(dispatcher) {
        // Every event here is a WIRE event, mapped by the same `mapReplyStream` /
        // `clientEventToReply` the engine source uses. The MessageComplete payload
        // is deliberately text-only because that is the ONLY shape the engine can
        // produce: `streaming_loop.rs` accumulates text/thinking into
        // `assistant_blocks` and routes ToolUse to a separate `tool_uses` field,
        // and `synthesize_message` lowers `assistant_blocks` alone. So the turn's
        // tool calls can only come from the live run trace.
        val vm = newVm()
        vm.driveWire(
            ClientEvent.TurnStarted(turnId = null),
            ClientEvent.TextDelta("好的"),
            ClientEvent.ToolUseStarted(
                id = "e1",
                tool = "Edit",
                inputJson = """{"file_path":"src/host.rs"}""",
                header = wireHeader(),
            ),
            ClientEvent.ToolUseResult(
                id = "e1", tool = "Edit", resultJson = """"ok"""", isError = false,
                display = wireDisplay(),
            ),
            ClientEvent.MessageComplete(
                stopReason = "end_turn",
                message = MessageDto(role = "assistant", blocks = listOf(MessageBlockDto.Text("好的"))),
            ),
        )

        // Premise check: the wire message really does arrive with no tool block.
        val wireOnly = messageDtoToMessage(
            MessageDto(role = "assistant", blocks = listOf(MessageBlockDto.Text("好的"))),
        )
        assertTrue(wireOnly.blocks.none { it is MessageContent.Tool })

        val settled = vm.state.value.messages.single()
        assertEquals(listOf("好的"), settled.blocks.filterIsInstance<MessageContent.Text>().map { it.text })
        val call = settled.blocks.filterIsInstance<MessageContent.Tool>().single().call
        assertEquals("e1", call.id)
        assertEquals(ToolVerbUi.Update, call.header?.verb)
        assertEquals(HeadlineKindUi.Added, call.display?.headlineKind)
        assertEquals(AgentToolStatus.Completed, call.status)
        // Moved, not copied: the run card must not draw the same row again.
        assertTrue("the run trace hands its rows over", vm.state.value.agentRun!!.tools.isEmpty())
    }

    @Test
    fun settledTurn_absorbsItsToolCalls_onTheTurnEndedPathTheMobileEngineActuallyTakes() =
        runTest(dispatcher) {
            // The mobile host never emits MessageComplete — `TurnWrapper::complete`
            // has no production caller there — so a real turn settles on TurnEnded.
            val vm = newVm()
            vm.driveWire(
                ClientEvent.TurnStarted(turnId = null),
                ClientEvent.TextDelta("查完了"),
                ClientEvent.ToolUseStarted(id = "g1", tool = "Grep", inputJson = "{}", header = wireHeader()),
                ClientEvent.ToolUseResult(
                    id = "g1", tool = "Grep", resultJson = """"ok"""", isError = false,
                    display = wireDisplay(),
                ),
                ClientEvent.TurnEnded(outcome = TurnOutcomeDto.END_TURN, stopReason = "end_turn", cost = cost()),
            )

            val settled = vm.state.value.messages.single()
            assertEquals("查完了", settled.text)
            assertEquals(listOf("查完了"), settled.blocks.filterIsInstance<MessageContent.Text>().map { it.text })
            assertEquals("g1", settled.blocks.filterIsInstance<MessageContent.Tool>().single().call.id)
            assertTrue(vm.state.value.agentRun!!.tools.isEmpty())
        }

    @Test
    fun aSettledTurnsToolCalls_surviveTheNextTurnStarting() = runTest(dispatcher) {
        // The user-visible outcome: `send()` replaces `agentRun` wholesale, so a
        // turn whose tool calls live only there loses them the moment the user
        // asks the next question — and the conversation then reads differently
        // before and after a restart.
        val source = RecordingSource()
        val vm = ChatViewModel(source)
        vm.driveWire(
            ClientEvent.TurnStarted(turnId = null),
            ClientEvent.TextDelta("改好了"),
            ClientEvent.ToolUseStarted(id = "e1", tool = "Edit", inputJson = "{}", header = wireHeader()),
            ClientEvent.ToolUseResult(
                id = "e1", tool = "Edit", resultJson = """"ok"""", isError = false, display = wireDisplay(),
            ),
            ClientEvent.TurnEnded(outcome = TurnOutcomeDto.END_TURN, stopReason = "end_turn", cost = cost()),
        )
        val turnOne = vm.state.value.messages.single()

        vm.send("再改一处")

        assertTrue("the fresh run starts empty", vm.state.value.agentRun!!.tools.isEmpty())
        val kept = vm.state.value.messages.first { it.id == turnOne.id }
        assertEquals(
            "turn N-1 keeps its tool call after turn N starts",
            "e1",
            kept.blocks.filterIsInstance<MessageContent.Tool>().single().call.id,
        )
    }

    @Test
    fun aToolOnlyTurn_stillLandsItsCallsInTheTranscript() = runTest(dispatcher) {
        // No text at all: a resume would rebuild this as an assistant bubble of
        // tool rows, so the live path must not silently drop them.
        val vm = newVm()
        vm.driveWire(
            ClientEvent.TurnStarted(turnId = null),
            ClientEvent.ToolUseStarted(id = "r1", tool = "Read", inputJson = "{}", header = wireHeader()),
            ClientEvent.ToolUseResult(
                id = "r1", tool = "Read", resultJson = """"ok"""", isError = false, display = wireDisplay(),
            ),
            ClientEvent.TurnEnded(outcome = TurnOutcomeDto.END_TURN, stopReason = "end_turn", cost = cost()),
        )

        val settled = vm.state.value.messages.single()
        assertEquals(Role.Ai, settled.role)
        assertEquals("r1", settled.blocks.filterIsInstance<MessageContent.Tool>().single().call.id)
    }

    @Test
    fun shellCalls_stayInTheirTerminalCard_andAreNotAbsorbedTwice() = runTest(dispatcher) {
        val vm = newVm()
        vm.driveWire(
            ClientEvent.TurnStarted(turnId = null),
            ClientEvent.TextDelta("跑一下"),
            ClientEvent.ToolUseStarted(
                id = "sh1",
                tool = "bash",
                inputJson = """{"command":"echo ok","cwd":"/workspace"}""",
                header = null,
            ),
            ClientEvent.TurnEnded(outcome = TurnOutcomeDto.END_TURN, stopReason = "end_turn", cost = cost()),
        )

        assertEquals(1, vm.state.value.shellTools.size)
        assertTrue(
            "a shell call already owns a terminal card — it must not become a bubble row too",
            vm.state.value.messages.single().blocks.none { it is MessageContent.Tool },
        )
        assertEquals(listOf("sh1"), vm.state.value.agentRun!!.tools.map { it.id })
    }

    @Test
    fun settlingTwice_doesNotDuplicateTheAbsorbedRows() {
        // MessageComplete then TurnEnded (the shape a non-mobile host emits) must
        // settle once — the MOVE out of the run trace is what makes it idempotent.
        val vm = newVm()
        vm.reduce(
            ReplyEvent.ToolActivity(
                label = "调用工具 Edit…", id = "e1", tool = "Edit",
                status = AgentToolStatus.Running, header = header(),
            ),
        )
        vm.reduce(ReplyEvent.Completed(Message(role = Role.Ai, text = "好的")))
        vm.reduce(ReplyEvent.End)

        assertEquals(1, vm.state.value.messages.size)
        assertEquals(
            listOf("e1"),
            vm.state.value.messages.single().blocks
                .filterIsInstance<MessageContent.Tool>().map { it.call.id },
        )
    }

    @Test
    fun completedMessageWithNoBlocks_isLeftExactlyAsItArrived() {
        val vm = newVm()
        val message = Message(role = Role.Ai, text = "plain")
        vm.reduce(ReplyEvent.Completed(message))
        assertSame(message, vm.state.value.messages.single())
    }

    @Test
    fun exactTurnCompletionCarriesOriginOutcomeAndFinalText() = runTest(dispatcher) {
        val vm = newVm()
        vm.send("hello", origin = ConversationTurnOrigin.Flow)
        val completion = async(start = CoroutineStart.UNDISPATCHED) {
            vm.turnCompletions.first()
        }

        vm.reduce(ReplyEvent.Delta("final answer"))
        vm.reduce(ReplyEvent.Completed(Message(role = Role.Ai, text = "final answer")))

        assertEquals(
            ConversationTurnCompletion(
                token = 1,
                origin = ConversationTurnOrigin.Flow,
                outcome = ConversationTurnOutcome.Completed,
                finalAssistantText = "final answer",
            ),
            completion.await(),
        )
    }

    // --- plan checklist ---------------------------------------------------

    @Test
    fun planUpdated_replacesTheWholeChecklist_andAnEmptyListClearsIt() {
        val vm = newVm()
        vm.reduceClientEvent(
            ClientEvent.PlanUpdated(
                tasks = listOf(
                    PlanTaskDto(id = null, subject = "写代码", activeForm = null, state = PlanTaskStateDto.IN_PROGRESS),
                    PlanTaskDto(id = null, subject = "跑测试", activeForm = null, state = PlanTaskStateDto.PENDING),
                ),
            ),
        )
        assertEquals(listOf("写代码", "跑测试"), vm.state.value.planTasks.map { it.subject })

        // FULL-LIST replace, not a merge.
        vm.reduceClientEvent(
            ClientEvent.PlanUpdated(
                tasks = listOf(
                    PlanTaskDto(id = null, subject = "只剩这个", activeForm = null, state = PlanTaskStateDto.COMPLETED),
                ),
            ),
        )
        assertEquals(listOf("只剩这个"), vm.state.value.planTasks.map { it.subject })

        vm.reduceClientEvent(ClientEvent.PlanUpdated(tasks = emptyList()))
        assertTrue(vm.state.value.planTasks.isEmpty())
    }

    @Test
    fun newChat_clearsThePlanAndEveryToolExpansion() {
        val vm = newVm()
        vm.reduceClientEvent(
            ClientEvent.PlanUpdated(
                tasks = listOf(
                    PlanTaskDto(id = null, subject = "旧会话的任务", activeForm = null, state = PlanTaskStateDto.PENDING),
                ),
            ),
        )
        vm.toggleToolCall("e1")
        vm.togglePlanExpanded()
        assertTrue(vm.state.value.planTasks.isNotEmpty())

        vm.newChat()

        assertTrue("a plan belongs to the session that produced it", vm.state.value.planTasks.isEmpty())
        assertTrue(vm.state.value.expandedToolCalls.isEmpty())
        assertFalse(vm.state.value.planExpanded)
    }

    private fun header() = ToolHeaderUi(
        verb = ToolVerbUi.Update,
        label = "Update",
        primary = "src/host.rs",
        title = "Update(src/host.rs)",
    )

    private fun display() = ToolResultDisplayUi(
        headline = "Added 2 lines",
        headlineKind = HeadlineKindUi.Added,
        headlineArgs = listOf(2),
    )

    // --- real-wire fixtures ------------------------------------------------

    /**
     * Drive REAL engine events the way `EngineConversationSource.submit` does —
     * through [mapReplyStream], which maps each [ClientEvent] with
     * [clientEventToReply] (including the `MessageDto` → [Message] lowering) and
     * stops at the first terminal reply. Nothing about the resulting transcript
     * is hand-shaped, so a test written against it cannot pass on a [Message]
     * the engine could never emit.
     */
    private suspend fun ChatViewModel.driveWire(vararg events: ClientEvent) {
        mapReplyStream(flowOf(*events)).collect { reduce(it) }
    }

    private fun wireHeader() = ToolHeaderDto(
        verb = ToolVerbDto.UPDATE,
        icon = null,
        label = "Update",
        primary = "src/host.rs",
        qualifier = null,
        count = null,
        subLine = null,
        title = "Update(src/host.rs)",
    )

    private fun wireDisplay() = ToolResultDisplayDto(
        headline = "Added 2 lines",
        headlineKind = HeadlineKindDto.ADDED,
        headlineArgs = listOf(2u),
        diff = null,
        body = null,
        bodyLines = 0u,
        bodyTruncated = false,
        collapsed = false,
    )

    private fun cost() = CostDto(
        totalUsd = 0.0,
        inputTokens = 0u,
        outputTokens = 0u,
        apiCalls = 0u,
        sessionDurationSecs = 0u,
        formatted = "$0.00",
    )
}
