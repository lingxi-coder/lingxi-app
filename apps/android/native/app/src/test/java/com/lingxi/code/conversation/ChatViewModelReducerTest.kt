package com.lingxi.code.conversation

import androidx.lifecycle.SavedStateHandle
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ClientException
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientTextDto
import com.lingxi.code.bindings.client.SessionAgentSummaryDto
import com.lingxi.code.bindings.client.PromptModeDto
import com.lingxi.code.bindings.client.QueuedTurnStatusDto
import com.lingxi.code.bindings.client.CostDto
import com.lingxi.code.bindings.client.HeadlineKindDto
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.MessageBlockDto
import com.lingxi.code.bindings.client.MessageDto
import com.lingxi.code.bindings.client.ModSessionEventPayload
import com.lingxi.code.bindings.client.PlanTaskDto
import com.lingxi.code.bindings.client.PlanTaskStateDto
import com.lingxi.code.bindings.client.TaskStatusDto
import com.lingxi.code.bindings.client.TaskRowDto
import com.lingxi.code.bindings.client.ToolHeaderDto
import com.lingxi.code.bindings.client.ToolResultDisplayDto
import com.lingxi.code.bindings.client.ToolVerbDto
import com.lingxi.code.bindings.client.TurnOutcomeDto
import com.lingxi.code.bindings.client.TurnRecoverySnapshotDto
import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.SessionRow
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertNotNull
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
    private class RecordingSource : ConversationSource, SlashCommandConversationSource {
        val submitted = mutableListOf<String>()
        val pending = mutableListOf<List<UShort>>()
        val commands = mutableListOf<ClientCommand>()
        val slashRequests = mutableListOf<Pair<List<UShort>, Long>>()
        val slashImageRequests = mutableListOf<List<ImageRefDto>>()
        val cancelledTurnIds = mutableListOf<Long?>()
        var submittedTurnId: Long? = null
        val submittedPromptModes = mutableListOf<PromptModeDto?>()
        var cancelCount = 0
        private val never = MutableSharedFlow<ReplyEvent>()
        /** Drivable so a test can make a permission request arrive and settle. */
        val permissions = MutableStateFlow<PermissionPromptState?>(null)
        override val pendingPermission: StateFlow<PermissionPromptState?> get() = permissions.asStateFlow()
        override fun initialMessages(): List<Message> = emptyList()
        override fun submit(text: String): Flow<ReplyEvent> {
            submitted += text
            return never.asSharedFlow() // a turn that streams forever until cancelled
        }
        override suspend fun submitClientCommand(command: ClientCommand) {
            commands += command
            if (command is ClientCommand.SendPrompt) pending += command.text.codeUnits.toList()
        }
        override fun submitSlashCommand(raw: ClientTextDto, images: List<ImageRefDto>, turnId: Long): Flow<ReplyEvent> {
            slashRequests += raw.codeUnits.toList() to turnId
            slashImageRequests += images
            return never.asSharedFlow()
        }
        override fun submit(
            text: String,
            images: List<ImageRefDto>,
            turnId: Long,
        ): Flow<ReplyEvent> {
            submittedTurnId = turnId
            return submit(text)
        }
        override fun submit(
            text: String,
            images: List<ImageRefDto>,
            turnId: Long,
            visualizationContext: VisualizationRef?,
            promptMode: PromptModeDto?,
        ): Flow<ReplyEvent> {
            submittedTurnId = turnId
            submittedPromptModes += promptMode
            return submit(text)
        }
        override suspend fun cancel(turnId: Long?) {
            cancelCount++
            cancelledTurnIds += turnId
        }
    }

    private class RecordingBackgroundExecution : ConversationBackgroundExecution {
        val activeStates = mutableListOf<Boolean>()
        val finishedTasks = mutableListOf<Triple<String, String?, Boolean>>()
        val armedPermissions = mutableListOf<Pair<ULong, String>>()
        val settledPermissions = mutableListOf<ULong>()

        override fun setTurnActive(active: Boolean) {
            if (activeStates.lastOrNull() != active) activeStates += active
        }

        override fun notifyTaskFinished(
            snapshot: ConversationBackgroundSnapshot,
            taskId: String,
            label: String?,
            failed: Boolean,
        ) {
            finishedTasks += Triple(taskId, label, failed)
        }

        override fun notifyPermissionRequested(
            snapshot: ConversationBackgroundSnapshot,
            requestId: ULong,
            toolName: String,
        ) {
            armedPermissions += requestId to toolName
        }

        override fun settlePermission(requestId: ULong) {
            settledPermissions += requestId
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
        val workspace = MutableStateFlow<com.lingxi.code.project.SessionWorkspaceState?>(null)
        override val workspaceState = workspace.asStateFlow()
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

        override suspend fun newSession() { operations += "new" }
        override suspend fun newSession(cwd: String) { operations += "new:$cwd" }

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

    private class DurableAttachSource(private val turnId: Long = 88L) : ConversationSource {
        val active = MutableStateFlow<ActivatedSession?>(null)
        val attachRequests = mutableListOf<Long>()
        val submitted = mutableListOf<String>()
        val discarded = mutableListOf<Long>()

        override val activeSessionState = active.asStateFlow()
        override fun submit(text: String): Flow<ReplyEvent> {
            submitted += text
            return emptyFlow()
        }

        override suspend fun attachDurableTurnForUi(afterSequence: Long, onAttach: (String, Long, Boolean, String) -> Unit) {
            attachRequests += afterSequence
            active.value?.let { onAttach(it.sessionId, turnId, true, "") }
        }

        override suspend fun discardDurableTurn(turnId: Long) {
            discarded += turnId
        }
    }

    private class FailingDurableAttachSource : ConversationSource {
        val active = MutableStateFlow<ActivatedSession?>(null)
        val deliveries = MutableSharedFlow<ConversationDelivery>(extraBufferCapacity = 16)
        val discarded = mutableListOf<Long>()
        val submitted = mutableListOf<String>()
        val sessionOperations = mutableListOf<String>()

        override val activeSessionState = active.asStateFlow()
        override val conversationDeliveries: Flow<ConversationDelivery> = deliveries.asSharedFlow()
        override fun submit(text: String): Flow<ReplyEvent> {
            submitted += text
            return emptyFlow()
        }

        override suspend fun attachDurableTurnForUi(afterSequence: Long, onAttach: (String, Long, Boolean, String) -> Unit) {
            onAttach("session-a", 94L, true, "")
            val paused = TurnRecoverySnapshotDto(
                sessionId = "session-a",
                turnId = 94u,
                revision = 1uL,
                state = TurnRecoveryStateDto.PAUSED_RECOVERABLE,
                firstSequence = 0u,
                lastSequence = 1u,
                safeToResume = true,
                projectionUncertain= false,
                reason = "paused",
            )
            deliveries.tryEmit(ConversationDelivery.Event(ClientEvent.TurnRecoveryState(paused)))
            deliveries.tryEmit(
                ConversationDelivery.TurnEvent(
                    sessionId = "session-a",
                    turnId = 94u,
                    sequence = 1u,
                    event = ClientEvent.TextDelta("retained before failure".toClientTextDto()),
                ),
            )
            deliveries.tryEmit(ConversationDelivery.AttachReplayEnded("session-a", 94L, true))
            val failure = DurableAttachFailure(
                turnId = 94L,
                phase = "resume",
                cause = IllegalStateException("resume failed"),
            )
            failure.orderedDelivery = true
            deliveries.tryEmit(ConversationDelivery.AttachFailed("session-a", 94L, failure))
            throw failure
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

    @Test fun busyCompactRefusalPreservesStreamingAndAutomaticCompaction() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)
        vm.send("working")
        runCurrent()
        vm.reduceClientEvent(ClientEvent.CompactionStatus(phase = "summarizing", error = null))
        val before = vm.state.value
        vm.send("/compact")
        assertNotNull(vm.state.value.compactRejectionId)
        assertEquals(before.messages, vm.state.value.messages)
        assertEquals(before.compaction, vm.state.value.compaction)
        vm.reduceClientEvent(ClientEvent.Error(
            kind = com.lingxi.code.bindings.client.ErrorKindDto.PROTOCOL,
            message = "cannot compact the session while a turn is in flight".toClientTextDto(),
        ))
        assertTrue(vm.state.value.streaming)
        assertNull(vm.state.value.error)
        assertEquals(before.agentRun, vm.state.value.agentRun)
        assertEquals(before.compaction, vm.state.value.compaction)
        assertTrue(source.commands.none { it is ClientCommand.ForceCompact })
        vm.dismissCompactRejection()
        assertNull(vm.state.value.compactRejectionId)
    }

    @Test
    fun userTranscriptIdentityUsesExactSubmittedTurnAndNeverTextMatching() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source = source)
        vm.send("same text")
        runCurrent()
        val firstTurn = checkNotNull(source.submittedTurnId)
        vm.send("same text")
        runCurrent()
        val queuedTurn = source.commands.filterIsInstance<ClientCommand.SendPrompt>().last().turnId!!
        val uuid = "f4e89fba-1c3d-4a5c-9a6d-1b75d92ca85f"
        vm.reduceClientEvent(ClientEvent.UserTranscriptRowIdentity("client-turn:$queuedTurn", uuid))
        assertNull(vm.state.value.messages.first().transcriptUuid)
        assertEquals(uuid, vm.state.value.messages.last().transcriptUuid)
        vm.reduceClientEvent(ClientEvent.UserTranscriptRowIdentity("client-turn:$firstTurn", "9d34f2be-a1b2-42fa-bcde-2fe8d407f1ab"))
        assertEquals("9d34f2be-a1b2-42fa-bcde-2fe8d407f1ab", vm.state.value.messages.first().transcriptUuid)
    }

    @Test
    fun coalescedUserIdentityMapsEveryExplicitMemberToken() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source = source)
        vm.send("first")
        runCurrent()
        val firstTurn = checkNotNull(source.submittedTurnId)
        vm.send("second")
        runCurrent()
        val queuedTurn = source.commands.filterIsInstance<ClientCommand.SendPrompt>().last().turnId!!
        val uuid = "f4e89fba-1c3d-4a5c-9a6d-1b75d92ca85f"
        vm.reduceClientEvent(ClientEvent.UserTranscriptRowIdentity("client-turn:$firstTurn,$queuedTurn", uuid))
        assertTrue(vm.state.value.messages.all { it.transcriptUuid == uuid })
    }

    @Test fun fixedScheduleNoticeArrivesBeforeTurnWithoutLoopFoldMarker() {
        val vm = newVm()
        vm.reduceClientEvent(ClientEvent.ScheduledTaskFire("Fixed task is ready"))
        val message = vm.state.value.messages.single()
        assertEquals("Fixed task is ready", message.text)
        assertEquals(null, message.loopWakeupStreak)
        assertTrue(message.loopFoldedItemIds.isEmpty())
        assertFalse(vm.state.value.streaming)
        vm.reduceClientEvent(ClientEvent.SystemNotice("Scheduled task text is still turn-scoped".toClientTextDto(), false))
        assertEquals(listOf(message), vm.state.value.messages)
    }

    @Test fun loopFoldDoesNotHideAShellFromBeforeTheFire() {
        val vm = newVm()
        vm.reduce(ReplyEvent.ShellTool(ShellToolUpdate.Started("old", "pwd", null)))
        vm.reduceClientEvent(ClientEvent.LoopWakeup("first", null, 0u, 0uL))
        vm.reduceClientEvent(ClientEvent.LoopWakeup("second", null, 1u, 1uL))
        assertTrue(buildChatRenderItems(vm.state.value).any { it.key == "shell-old" })
    }

    @Test fun loopWakeupFoldsQuietRowsOutsideTheTurnStream() {
        val vm = newVm()
        vm.reduceClientEvent(ClientEvent.LoopWakeup("first", null, 0u, 0uL))
        assertEquals("first", vm.state.value.messages.last().text)
        vm.reduceClientEvent(ClientEvent.LoopWakeup("second", "healthy", 1u, 1uL))
        assertEquals(listOf("second", "healthy"), buildChatRenderItems(vm.state.value)
            .filterIsInstance<ChatRenderItem.Message>().map { it.message.text })
        vm.reduceClientEvent(ClientEvent.LoopWakeup("third", "still healthy", 2u, 1uL))
        assertEquals(listOf("third", "still healthy"), buildChatRenderItems(vm.state.value)
            .filterIsInstance<ChatRenderItem.Message>().map { it.message.text })
        assertEquals(5, vm.state.value.messages.size)
        vm.reduceClientEvent(ClientEvent.LoopWakeup("actionable", null, 0u, 0uL))
        assertEquals(3, buildChatRenderItems(vm.state.value).filterIsInstance<ChatRenderItem.Message>().size)
    }

    private fun newVm() = ChatViewModel(StubSource())

    @Test
    fun promptSuggestionPreservesWireUtf16ForEditableDraft() {
        val vm = newVm()
        val suggestion = "继续检查 \uD83D\uDD0Dx\uD800".toClientTextDto()

        vm.reduceClientEvent(ClientEvent.PromptSuggestion(suggestion, "suggestion-1", "new"))

        val retained = vm.state.value.promptSuggestion
        assertEquals(suggestion.codeUnits, retained?.codeUnits)
        assertEquals(suggestion.displayText(), retained?.displayText())
        assertEquals(suggestion.sourceText(), retained?.sourceText())
    }

    @Test
    fun retractAssistantMessage_usesIdentityAndPreservesAnotherResponse() {
        val vm = newVm()
        vm.reduce(ReplyEvent.ReasoningDelta("failed reasoning"))
        vm.reduce(ReplyEvent.Delta("failed"))
        vm.reduce(ReplyEvent.MessageIdentity("failed-id"))
        vm.reduce(ReplyEvent.MessageComplete(Message(role = Role.Ai, text = "failed")))
        vm.reduce(ReplyEvent.ReasoningDelta("retained reasoning"))
        vm.reduce(ReplyEvent.Delta("retained"))
        vm.reduce(ReplyEvent.MessageIdentity("retained-id"))
        vm.reduce(ReplyEvent.MessageComplete(Message(role = Role.Ai, text = "retained")))
        vm.reduce(ReplyEvent.MessageRetracted("unknown"))
        assertEquals(listOf("failed", "retained"), vm.state.value.messages.map { it.text })
        vm.reduce(ReplyEvent.MessageRetracted("failed-id"))
        assertEquals(listOf("retained"), vm.state.value.messages.map { it.text })
        assertEquals("retained reasoning", vm.state.value.agentRun?.reasoning)
        vm.reduce(ReplyEvent.MessageRetracted("failed-id"))
        assertEquals(listOf("retained"), vm.state.value.messages.map { it.text })
    }

    @Test
    fun compactSlashUsesDedicatedCommandAndCliProgressThenSettles() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)

        vm.send("/compact")
        runCurrent()

        assertEquals(1, source.commands.count { it is ClientCommand.ForceCompact })
        assertEquals("/compact", vm.state.value.messages.single().text)
        assertEquals(CompactionProgressStatus.Running, vm.state.value.compaction?.status)
        assertNull(vm.state.value.compaction?.startedAtMillis)
        vm.reduceClientEvent(ClientEvent.CompactionStatus("summarizing", null))
        val summaryStart = vm.state.value.compaction?.startedAtMillis
        assertNotNull(summaryStart)
        vm.reduceClientEvent(ClientEvent.CompactionStatus("summarizing", null))
        vm.reduceClientEvent(ClientEvent.CompactionStatus("restoring", null))
        assertEquals(summaryStart, vm.state.value.compaction?.startedAtMillis)
        assertTrue(vm.state.value.requiresBackgroundExecution)
        assertEquals(10, compactProgressPercent("summarizing", 0))
        assertEquals(13, compactProgressPercent("summarizing", 4_000))
        assertEquals(57, compactProgressPercent("summarizing", 90_000))
        assertEquals(84, compactProgressPercent("summarizing", 10_000_000))
        vm.send("must not overlap compaction")
        assertEquals(1, vm.state.value.messages.size)

        vm.reduceClientEvent(
            ClientEvent.CompactionCompleted(
                20u,
                7u,
                4096u,
                ClientTextDto(codeUnits = "kept context".map { it.code.toUShort() }),
            ),
        )
        val completed = vm.state.value.compaction
        assertEquals(CompactionProgressStatus.Completed, completed?.status)
        assertEquals(20, completed?.messagesBefore)
        assertEquals(7, completed?.messagesAfter)
        assertEquals(4096L, completed?.bytesSaved)
        assertFalse(vm.state.value.requiresBackgroundExecution)
    }

    @Test
    fun leadingBangInitialPromptUsesBashAndPreservesCommandBody() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)

        vm.send("!  printf !!  ")
        runCurrent()

        assertEquals(listOf(PromptModeDto.BASH), source.submittedPromptModes)
        assertEquals("  printf !!  ", source.submitted.single())
        assertEquals("!  printf !!", vm.state.value.messages.last().text)
    }

    @Test
    fun leadingBangQueuedPromptKeepsItsOwnBashModeAndCommandBody() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)

        vm.send("first")
        runCurrent()
        vm.send("!  printf !!  ")
        runCurrent()

        assertEquals(listOf(null), source.submittedPromptModes)
        val queued = source.commands.filterIsInstance<ClientCommand.SendPrompt>().last()
        assertEquals("  printf !!  ", queued.text.displayText())
        assertEquals(PromptModeDto.BASH, queued.promptMode)
        assertEquals("!  printf !!", vm.state.value.messages.last().text)
    }

    @Test
    fun slashCommandOutput_isDisplayedAndSettlesItsTurn() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)
        vm.send("/help")
        runCurrent()

        val slashTurnId = source.slashRequests.single().second
        assertTrue(source.submitted.isEmpty())
        assertTrue(source.commands.none { it is ClientCommand.SendPrompt })
        vm.reduce(ReplyEvent.CommandOutput("Available commands", isError = false, turnId = slashTurnId.toULong()))

        assertEquals("Available commands", vm.state.value.messages.last().text)
        assertFalse(vm.state.value.streaming)
        assertNull(vm.state.value.error)
        assertNull(vm.state.value.agentRun)
    }

    @Test
    fun slashCommandWithImageKeepsTheCommandAndAttachmentOnTheSlashPath() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)
        val image = ImageRefDto(mediaType = "image/png", base64 = "cG5nLWltYWdl")

        vm.send("/inspect", listOf(image))
        runCurrent()

        assertEquals(1, source.slashRequests.size)
        assertEquals(listOf(image), source.slashImageRequests.single())
        assertTrue(source.submitted.isEmpty())
        assertTrue(source.commands.none { it is ClientCommand.SendPrompt })
    }

    @Test
    fun slashCommandFailure_displaysItsResultAndErrorState() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)
        val raw = "\uFEFF/unknown arg \uD800  "
        vm.send(raw)
        runCurrent()

        val (rawUnits, slashTurnId) = source.slashRequests.single()
        assertTrue(isSlashCommandLine(raw))
        assertFalse(isSlashCommandLine("ordinary prompt /unknown"))
        assertFalse(isSlashCommandLine("\u001C/unknown"))
        assertEquals(raw.toClientTextDto().codeUnits, rawUnits)
        assertTrue(source.submitted.isEmpty())
        assertTrue(source.commands.none { it is ClientCommand.SendPrompt })
        vm.reduce(ReplyEvent.CommandOutput("stale result", isError = true, turnId = (slashTurnId + 1).toULong()))
        assertTrue(vm.state.value.streaming)
        vm.reduce(ReplyEvent.CommandOutput("Unknown command", isError = true, turnId = slashTurnId.toULong()))

        assertEquals("Unknown command", vm.state.value.messages.last().text)
        assertEquals("Unknown command", vm.state.value.error?.message)
        assertFalse(vm.state.value.streaming)
    }

    @Test
    fun runSlashCommandClientCommand_preservesUtf16ImagesAndTurnIdentity() {
        val raw = "/custom arg \uD800".toClientTextDto()
        val images = listOf(ImageRefDto("image/png", "AQID"), ImageRefDto("image/jpeg", "BAUG"))
        val command = runSlashCommandClientCommand("session-a", raw, images, 73L)

        assertEquals("session-a", command.sessionId)
        assertEquals(raw.codeUnits, command.raw.codeUnits)
        assertEquals(images, command.images)
        assertEquals(73uL, command.turnId)
    }

    @Test
    fun compactCommandFailureSettlesTheVisibleProgress() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)
        vm.send("/compact")
        runCurrent()

        vm.reduceClientEvent(ClientEvent.Error(
            kind = com.lingxi.code.bindings.client.ErrorKindDto.INTERNAL,
            message = "force_compact failed: handle action failed: rate limited".toClientTextDto(),
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

    // --- notification wiring ------------------------------------------------
    // These assert CALL COUNTS on the notifier, not state fields: the whole
    // class of defect here is "computed but never wired", which renders and
    // reduces perfectly while notifying nobody.

    @Test
    fun aFinishedBackgroundTaskIsHandedToTheNotifierWithItsLabel() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(source = RecordingSource(), backgroundExecution = execution)
        vm.applyActivatedSession(
            ActivatedSession("session-a", emptyList(), SessionActivationKind.Started),
        )
        runCurrent()

        vm.reduceClientEvent(
            ClientEvent.TaskRow(
                TaskRowDto("task-1", "agent", TaskStatusDto.RUNNING, "Reindex the docs".toClientTextDto(), false, null, null),
            ),
        )
        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.COMPLETED, null, null))
        runCurrent()

        assertEquals(
            listOf(Triple("task-1", "Reindex the docs", false)),
            execution.finishedTasks,
        )
    }

    @Test
    fun anUnfinishedBackgroundTaskIsNotHandedToTheNotifier() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(source = RecordingSource(), backgroundExecution = execution)
        vm.applyActivatedSession(
            ActivatedSession("session-a", emptyList(), SessionActivationKind.Started),
        )
        runCurrent()

        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.RUNNING, null, null))
        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.PAUSED, null, null))
        runCurrent()

        assertEquals(emptyList<Triple<String, String?, Boolean>>(), execution.finishedTasks)
    }

    @Test
    fun unknownExitCannotBeReopenedOrReportedAsSuccessByLateEvents() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(source = RecordingSource(), backgroundExecution = execution)
        vm.applyActivatedSession(
            ActivatedSession("session-a", emptyList(), SessionActivationKind.Started),
        )
        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.EXITED_UNKNOWN, null, null))
        vm.reduceClientEvent(
            ClientEvent.TaskRow(
                TaskRowDto("task-1", "shell", TaskStatusDto.RUNNING, "npm test".toClientTextDto(), false, null, null),
            ),
        )
        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.COMPLETED, null, null))
        runCurrent()

        assertEquals(TaskStatusDto.EXITED_UNKNOWN, vm.state.value.backgroundTasks["task-1"]?.status)
        assertTrue(vm.state.value.activeBackgroundTaskIds.isEmpty())
        assertEquals(emptyList<Triple<String, String?, Boolean>>(), execution.finishedTasks)
    }

    @Test
    fun aFailedBackgroundTaskFallsBackToItsIdWhenNoRowNamedIt() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(source = RecordingSource(), backgroundExecution = execution)
        vm.applyActivatedSession(
            ActivatedSession("session-a", emptyList(), SessionActivationKind.Started),
        )
        runCurrent()

        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-9", TaskStatusDto.FAILED, null, "boom"))
        runCurrent()

        assertEquals(listOf(Triple("task-9", null, true)), execution.finishedTasks)
    }

    @Test
    fun backgroundTaskKeepsExecutionLeaseAfterTurnEnds() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(
            source = RecordingSource(),
            backgroundExecution = execution,
        )

        vm.send("start an asynchronous task")
        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.RUNNING, null, null))
        vm.reduce(ReplyEvent.End)
        runCurrent()

        assertEquals(listOf(true), execution.activeStates)

        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.COMPLETED, null, null))
        runCurrent()
        assertEquals(listOf(true, false), execution.activeStates)
    }

    @Test
    fun durableAttachReplaysHistoryThenRendersLiveEventsOnce() = runTest(dispatcher) {
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
            revision = 1uL,
            state = TurnRecoveryStateDto.RUNNING,
            firstSequence = 1u,
            lastSequence = 1u,
            safeToResume = true,
            projectionUncertain= false,
            reason = null,
        )

        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(running))
        vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", 88uL, 1uL,
            ClientEvent.TextDelta("history".toClientTextDto())))
        // ResumeTurn emits its own state and closes the attach replay window.
        vm.finishDurableAttachReplay(ConversationDelivery.AttachReplayEnded("session-a", 88L, true))
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(running))
        vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", 88uL, 2uL,
            ClientEvent.TextDelta("live".toClientTextDto())))

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
                revision = 1uL,
                state = TurnRecoveryStateDto.COMPLETED,
                firstSequence = 1u,
                lastSequence = 3u,
                safeToResume = false,
                projectionUncertain= false,
                reason = null,
            )
            vm.reduceClientEvent(ClientEvent.TurnRecoveryState(terminal))
            // These are the retained terminal checkpoints that can race past
            // the source-side replay gate after a process restart.  Neither
            // assistant prose nor a tool payload may be projected again.
            vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", 88uL, 1uL,
                ClientEvent.TextDelta("duplicate assistant".toClientTextDto())))
            vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", 88uL, 2uL,
                ClientEvent.ToolUseStarted("tool-1", "Read", "{}", wireHeader())))
            vm.finishDurableAttachReplay(ConversationDelivery.AttachReplayEnded("session-a", 88L, true))
            vm.reduceClientEvent(ClientEvent.TurnRecoveryState(terminal))

            assertEquals(restored, vm.state.value.messages)
            assertNull(vm.state.value.streamingMessage)
            assertFalse(vm.state.value.streaming)
            assertNull(vm.state.value.agentRun)
            assertEquals(0L, vm.durableReplayCursorForTesting())
            assertNotNull(vm.state.value.error)
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
            revision = 1uL,
            state = TurnRecoveryStateDto.RUNNING,
            firstSequence = 1u,
            lastSequence = 1u,
            safeToResume = true,
            projectionUncertain= false,
            reason = null,
        )

        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(running))
        vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", 88uL, 1uL,
            ClientEvent.TextDelta("history".toClientTextDto())))
        vm.finishDurableAttachReplay(ConversationDelivery.AttachReplayEnded("session-a", 88L, true))
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(running))
        vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", 88uL, 2uL,
            ClientEvent.TextDelta("live".toClientTextDto())))
        val replacementVm = ChatViewModel(source)
        runCurrent()

        assertEquals(2L, vm.durableReplayCursorForTesting())
        assertEquals(0L, replacementVm.durableReplayCursorForTesting())
        assertEquals(listOf(0L, 0L), source.attachRequests)
        assertEquals("historylive", vm.state.value.streamingMessage?.text)
        assertNull(replacementVm.state.value.streamingMessage)
    }

    @Test
    fun duplicateTerminalCannotCreateAnotherRecoveredTurnOrRenderLateRaw() = runTest(dispatcher) {
        val source = DurableAttachSource()
        val vm = ChatViewModel(source)
        vm.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        vm.noteDurableAttach("session-a", 88L, true)
        val terminal = TurnRecoverySnapshotDto(
            sessionId = "session-a", turnId = 88uL, revision = 4uL,
            state = TurnRecoveryStateDto.CANCELLED, firstSequence = 1uL, lastSequence = 1uL,
            safeToResume = false, projectionUncertain = false, reason = "attach",
        )
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(terminal))
        vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", 88uL, 1uL,
            ClientEvent.TextDelta("retained".toClientTextDto())))
        vm.finishDurableAttachReplay(ConversationDelivery.AttachReplayEnded("session-a", 88L, true))
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(terminal.copy(reason = "resume")))
        val messages = vm.state.value.messages
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(terminal.copy(reason = "duplicate")))
        vm.reduceClientEvent(ClientEvent.TextDelta("late raw".toClientTextDto()))

        assertEquals(messages, vm.state.value.messages)
        assertNull(vm.state.value.streamingMessage)
        assertFalse(vm.state.value.streaming)
        assertFalse(vm.state.value.durableRecoveryBlocked)
    }

    @Test
    fun singleAttachSettlesTerminalAfterReplayBoundaryWithoutResume() = runTest(dispatcher) {
        val vm = ChatViewModel(DurableAttachSource())
        vm.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        vm.noteDurableAttach("session-a", 88L, false)
        val terminal = TurnRecoverySnapshotDto(
            sessionId = "session-a", turnId = 88uL, revision = 1uL,
            state = TurnRecoveryStateDto.COMPLETED, firstSequence = 1uL, lastSequence = 1uL,
            safeToResume = false, projectionUncertain = false, reason = "attach",
        )
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(terminal))
        vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", 88uL, 1uL,
            ClientEvent.TextDelta("retained".toClientTextDto())))
        vm.finishDurableAttachReplay(ConversationDelivery.AttachReplayEnded("session-a", 88L, false))
        val messages = vm.state.value.messages
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(terminal))
        vm.reduceClientEvent(ClientEvent.TextDelta("late raw".toClientTextDto()))
        assertEquals(messages, vm.state.value.messages)
        assertTrue(messages.any { it.text.contains("retained") })
        assertNull(vm.state.value.streamingMessage)
        assertFalse(vm.state.value.streaming)
    }

    @Test
    fun explicitlyQueuedPromptAndBashOwnOnlyTheirNextTurn() = runTest(dispatcher) {
        for (queuedText in listOf("queued prompt", "!printf hi")) {
            val source = RecordingSource()
            val vm = ChatViewModel(source)
            vm.applyActivatedSession(ActivatedSession("queue-session", emptyList(), SessionActivationKind.Started))
            vm.send("first")
            val first = source.submittedTurnId!!
            vm.send(queuedText)
            runCurrent()
            val queued = source.commands.filterIsInstance<ClientCommand.SendPrompt>().last().turnId!!
            vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("queue-session", first.toULong(), 1uL,
                ClientEvent.TurnStarted(first.toULong())))
            vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("queue-session", first.toULong(), 2uL,
                ClientEvent.TurnEnded(TurnOutcomeDto.END_TURN, null, cost())))
            val terminal = TurnRecoverySnapshotDto("queue-session", first.toULong(), 2uL,
                TurnRecoveryStateDto.COMPLETED, 1uL, 2uL, false, false, null)
            vm.reduceClientEvent(ClientEvent.TurnRecoveryState(terminal))
            val next = TurnRecoverySnapshotDto("queue-session", queued, 1uL,
                TurnRecoveryStateDto.RUNNING, 1uL, 1uL, false, false, null)
            vm.reduceClientEvent(ClientEvent.TurnRecoveryState(next.copy(sessionId = "other-session")))
            vm.reduceClientEvent(ClientEvent.TurnRecoveryState(next.copy(turnId = queued + 1uL)))
            assertFalse(vm.state.value.streaming)
            vm.reduceClientEvent(ClientEvent.TurnRecoveryState(next))
            vm.reduceClientEvent(ClientEvent.TurnRecoveryState(terminal)) // late A cannot settle B
            vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("queue-session", queued, 1uL,
                ClientEvent.TurnStarted(queued)))
            vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("queue-session", queued, 2uL,
                ClientEvent.TextDelta("queued output".toClientTextDto())))
            assertEquals(2L, vm.durableReplayCursorForTesting())
            assertEquals("queued output", vm.state.value.streamingMessage?.text)
            vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("queue-session", queued, 3uL,
                ClientEvent.TurnEnded(TurnOutcomeDto.END_TURN, null, cost())))
            assertFalse(vm.state.value.streaming)
        }
    }

    @Test
    fun queuedTurnTimeoutWarnsWithoutReassigningTheActiveTurn() = runTest(dispatcher) {
        val deliveries = MutableSharedFlow<ConversationDelivery>(extraBufferCapacity = 8)
        val commands = mutableListOf<ClientCommand>()
        var first = 0L
        val source = object : ConversationSource {
            val never = MutableSharedFlow<ReplyEvent>()
            override val conversationDeliveries: Flow<ConversationDelivery> = deliveries.asSharedFlow()
            override fun submit(text: String, images: List<ImageRefDto>, turnId: Long,
                visualizationContext: VisualizationRef?, promptMode: PromptModeDto?): Flow<ReplyEvent> {
                first = turnId
                return never.asSharedFlow()
            }
            override suspend fun submitClientCommand(command: ClientCommand) { commands += command }
        }
        val vm = ChatViewModel(source)
        vm.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        vm.send("first")
        vm.send("!next")
        runCurrent()
        val queued = commands.filterIsInstance<ClientCommand.SendPrompt>().last().turnId!!
        deliveries.emit(ConversationDelivery.QueuedTurnIncomplete("session-a", queued))
        runCurrent()
        assertNotNull(vm.state.value.error)
        vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", first.toULong(), 1uL,
            ClientEvent.TextDelta("original still active".toClientTextDto())))
        assertEquals("original still active", vm.state.value.streamingMessage?.text)
        assertEquals(1L, vm.durableReplayCursorForTesting())
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(TurnRecoverySnapshotDto(
            "session-a", queued, 1uL, TurnRecoveryStateDto.RUNNING, 1uL, 1uL, false, false, null,
        )))
        assertEquals("original still active", vm.state.value.streamingMessage?.text)
    }

    @Test
    fun durableUnretainedCrashReopenUsesPersistedUncertaintyAndKeepsTranscript() = runTest(dispatcher) {
        val firstSource = RecordingSource()
        val first = ChatViewModel(firstSource)
        first.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        first.send("question")
        val turn = firstSource.submittedTurnId!!.toULong()
        first.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", turn, 1uL,
            ClientEvent.TurnStarted(turn)))
        first.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", turn, null,
            ClientEvent.TextDelta("visible before crash".toClientTextDto())))
        assertEquals("visible before crash", first.state.value.streamingMessage?.text)
        assertEquals(1L, first.durableReplayCursorForTesting())

        val reopened = ChatViewModel(DurableAttachSource(turn.toLong()))
        reopened.applyActivatedSession(ActivatedSession("session-a", listOf(Message(Role.Ai, "authoritative")),
            SessionActivationKind.Resumed))
        reopened.noteDurableAttach("session-a", turn.toLong(), true)
        reopened.reduceClientEvent(ClientEvent.TurnRecoveryState(TurnRecoverySnapshotDto(
            "session-a", turn, 2uL, TurnRecoveryStateDto.PAUSED_RECOVERABLE, 1uL, 1uL, false, true, null,
        )))
        reopened.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", turn, 1uL,
            ClientEvent.TurnStarted(turn)))
        assertNotNull(reopened.state.value.error)
        assertEquals(listOf("authoritative"), reopened.state.value.messages.map { it.text })
        assertEquals(1L, reopened.durableReplayCursorForTesting())
    }

    @Test
    fun detachedModEventsStayInTheirOriginSessionAndOutOfTheActiveRun() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)
        vm.applyActivatedSession(ActivatedSession("session-b", emptyList(), SessionActivationKind.Started))
        vm.send("active")
        val before = vm.state.value.agentRun
        vm.reduceClientEvent(ClientEvent.ModSessionEvent("session-a", "opaque-mod-turn",
            ModSessionEventPayload.Notice("old notice".toClientTextDto(), false)))
        assertNull(vm.state.value.statusLine)
        vm.reduceClientEvent(ClientEvent.ModSessionEvent("session-b", "opaque-mod-turn",
            ModSessionEventPayload.Notify("plugin", "notification".toClientTextDto())))
        assertEquals("plugin: notification", vm.state.value.statusLine)
        assertEquals(before, vm.state.value.agentRun)
    }

    @Test
    fun healthyLiveUncertaintyDoesNotShowRecoveryWarning() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)
        vm.applyActivatedSession(ActivatedSession("live-session", emptyList(), SessionActivationKind.Started))
        vm.send("hello")
        val turn = source.submittedTurnId!!.toULong()
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(TurnRecoverySnapshotDto(
            "live-session", turn, 1uL, TurnRecoveryStateDto.RUNNING, 1uL, 1uL, false, true, null,
        )))
        vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("live-session", turn, 1uL,
            ClientEvent.TextDelta("live output".toClientTextDto())))
        assertNull(vm.state.value.error)
        assertEquals("live output", vm.state.value.streamingMessage?.text)
    }

    @Test
    fun cancellationWaitsForLateBlockResultBeforeSettlingRun() = runTest(dispatcher) {
        val release = CompletableDeferred<Unit>()
        val deliveries = MutableSharedFlow<ConversationDelivery>(extraBufferCapacity = 8)
        var turn = 0L
        val source = object : ConversationSource {
            val never = MutableSharedFlow<ReplyEvent>()
            override val orderedCancellationBoundaries = true
            override val conversationDeliveries: Flow<ConversationDelivery> = deliveries.asSharedFlow()
            override fun submit(text: String, images: List<ImageRefDto>, turnId: Long,
                visualizationContext: VisualizationRef?, promptMode: PromptModeDto?): Flow<ReplyEvent> {
                turn = turnId
                return never.asSharedFlow()
            }
            override suspend fun cancel(turnId: Long?) {
                release.await()
                deliveries.emit(ConversationDelivery.CancellationDrained("session-a", turn, true))
            }
        }
        val vm = ChatViewModel(source)
        vm.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        vm.send("start")
        vm.reduceDurableTurnDelivery(ConversationDelivery.TurnEvent("session-a", turn.toULong(), 1uL,
            ClientEvent.ToolUseStarted("block-1", "Read", "{}", wireHeader())))
        vm.cancel()
        runCurrent()
        deliveries.emit(ConversationDelivery.Event(ClientEvent.TurnRecoveryState(TurnRecoverySnapshotDto(
            "session-a", turn.toULong(), 2uL, TurnRecoveryStateDto.CANCELLED, 1uL, 1uL, false, false, null,
        ))))
        deliveries.emit(ConversationDelivery.TurnEvent("session-a", turn.toULong(), null,
            ClientEvent.ToolUseResult("block-1", "Read", "{}", false, wireDisplay())))
        runCurrent()
        assertEquals(AgentToolStatus.Completed, vm.state.value.agentRun?.tools?.first { it.id == "block-1" }?.status)
        assertTrue(vm.state.value.cancellationInFlight)
        assertNotNull(vm.state.value.error) // the live-only result may not survive reopening
        release.complete(Unit)
        runCurrent()
        assertFalse(vm.state.value.streaming)
        assertFalse(vm.state.value.cancellationInFlight)
    }

    @Test
    fun failedCancellationRestoresOwnerBeforeTerminalAndQueuedTurn() = runTest(dispatcher) {
        val deliveries = MutableSharedFlow<ConversationDelivery>(extraBufferCapacity = 16)
        val commands = mutableListOf<ClientCommand>()
        var first = 0L
        val source = object : ConversationSource {
            val never = MutableSharedFlow<ReplyEvent>()
            override val orderedCancellationBoundaries = true
            override val conversationDeliveries: Flow<ConversationDelivery> = deliveries.asSharedFlow()
            override fun submit(text: String, images: List<ImageRefDto>, turnId: Long,
                visualizationContext: VisualizationRef?, promptMode: PromptModeDto?): Flow<ReplyEvent> {
                first = turnId
                return never.asSharedFlow()
            }
            override suspend fun submitClientCommand(command: ClientCommand) { commands += command }
            override suspend fun cancel(turnId: Long?) {
                deliveries.emit(ConversationDelivery.CancellationAborted("session-a", first, false))
                throw IllegalStateException("cancel failed")
            }
        }
        val vm = ChatViewModel(source)
        vm.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        vm.send("first")
        vm.send("!next")
        runCurrent()
        val next = commands.filterIsInstance<ClientCommand.SendPrompt>().last().turnId!!
        vm.cancel()
        runCurrent()
        assertTrue(vm.state.value.streaming)
        assertNotNull(vm.state.value.error)
        deliveries.emit(ConversationDelivery.TurnEvent("session-a", first.toULong(), 1uL,
            ClientEvent.TurnEnded(TurnOutcomeDto.END_TURN, null, cost())))
        deliveries.emit(ConversationDelivery.Event(ClientEvent.TurnRecoveryState(TurnRecoverySnapshotDto(
            "session-a", first.toULong(), 1uL, TurnRecoveryStateDto.COMPLETED, 1uL, 1uL, false, false, null,
        ))))
        deliveries.emit(ConversationDelivery.Event(ClientEvent.TurnRecoveryState(TurnRecoverySnapshotDto(
            "session-a", next, 1uL, TurnRecoveryStateDto.RUNNING, 1uL, 1uL, false, false, null,
        ))))
        deliveries.emit(ConversationDelivery.TurnEvent("session-a", next, 1uL,
            ClientEvent.TextDelta("next".toClientTextDto())))
        runCurrent()
        assertEquals("next", vm.state.value.streamingMessage?.text)
        assertEquals(1L, vm.durableReplayCursorForTesting())
    }

    @Test
    fun liveReplayAcknowledgementRequiresTheSubmittedSessionAndTurn() = runTest(dispatcher) {
        val wire = MutableSharedFlow<ConversationDelivery>(extraBufferCapacity = 16)
        var submittedTurn = 0L
        val source = object : ConversationSource {
            override val conversationDeliveries: Flow<ConversationDelivery> = wire.asSharedFlow()
            override fun submit(
                text: String, images: List<ImageRefDto>, turnId: Long,
                visualizationContext: VisualizationRef?, promptMode: PromptModeDto?,
            ): Flow<ReplyEvent> {
                submittedTurn = turnId
                return mapDeliveryReplyStream(wire, acceptsTurn = { session, turn ->
                    session == "session-a" && turn == turnId.toULong()
                })
            }
        }
        val vm = ChatViewModel(source)
        vm.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        vm.send("question")
        runCurrent()
        wire.emit(ConversationDelivery.TurnEvent("session-a", (submittedTurn + 1).toULong(), 100uL,
            ClientEvent.TextDelta("wrong turn".toClientTextDto())))
        wire.emit(ConversationDelivery.TurnEvent("session-b", submittedTurn.toULong(), 200uL,
            ClientEvent.TextDelta("wrong session".toClientTextDto())))
        runCurrent()
        assertEquals(0L, vm.durableReplayCursorForTesting())

        wire.emit(ConversationDelivery.TurnEvent("session-a", submittedTurn.toULong(), 1uL,
            ClientEvent.TextDelta("live".toClientTextDto())))
        runCurrent()
        assertEquals(1L, vm.durableReplayCursorForTesting())
        assertEquals("live", vm.state.value.streamingMessage?.text)
    }

    @Test
    fun overlappingUiOwnersReplayByTheirOwnCursorsAndDrainEveryConsumer() = runTest(dispatcher) {
        val relay = LosslessEventRelay<ConversationDelivery>(backgroundScope)
        val consumers = AttachConsumerRegistry()
        val ingress = DurableRecoveryIngress().newSource("store")
        var submittedTurn = 0L
        var subscriberCount = 0
        val holdFirst = CompletableDeferred<Unit>()
        val source = object : ConversationSource {
            val never = MutableSharedFlow<ReplyEvent>()
            override val conversationDeliveries: Flow<ConversationDelivery> get() = flow {
                val index = subscriberCount++
                consumers.track(relay.events).collect { delivery ->
                    if (index == 0 && delivery is ConversationDelivery.TurnEvent && delivery.sequence == 2uL &&
                        delivery.excludedAttachRequestId == "r2") holdFirst.await()
                    emit(delivery)
                }
            }
            override fun submit(text: String, images: List<ImageRefDto>, turnId: Long,
                visualizationContext: VisualizationRef?, promptMode: PromptModeDto?): Flow<ReplyEvent> {
                submittedTurn = turnId
                return never.asSharedFlow()
            }
        }
        val first = ChatViewModel(source)
        first.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        first.send("first")
        runCurrent()
        val turn = submittedTurn.toULong()
        ingress.acquireInterest("session-a", turn)
        val oldFence = ingress.fence("session-a", turn)
        relay.offer(ConversationDelivery.TurnEvent("session-a", turn, 1uL, ClientEvent.TurnStarted(turn), isActive = oldFence))
        runCurrent()

        val second = ChatViewModel(source)
        second.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        second.noteDurableAttach("session-a", submittedTurn, expectsResume = true, requestId = "r2")
        runCurrent()
        relay.offer(ConversationDelivery.TurnEvent("session-a", turn, 2uL,
            ClientEvent.TextDelta("middle".toClientTextDto()), excludedAttachRequestId = "r2", isActive = oldFence))
        val oldLease = ingress.detachInterest("session-a", turn)!!
        val retirement = consumers.retirement { ingress.releaseInterest(oldLease) }!!
        relay.offer(ConversationDelivery.RecoveryInterestEnded(retirement))
        ingress.acquireInterest("session-a", turn)
        val newFence = ingress.fence("session-a", turn)
        val snapshot = TurnRecoverySnapshotDto("session-a", turn, 1uL, TurnRecoveryStateDto.RUNNING,
            1uL, 2uL, false, false, null)
        relay.offer(ConversationDelivery.AttachStarted("r2", snapshot, newFence))
        relay.offer(ConversationDelivery.TurnEvent("session-a", turn, 1uL,
            ClientEvent.TurnStarted(turn), attachRequestId = "r2", isActive = newFence))
        relay.offer(ConversationDelivery.TurnEvent("session-a", turn, 2uL,
            ClientEvent.TextDelta("middle".toClientTextDto()), attachRequestId = "r2", isActive = newFence))
        val finished = CompletableDeferred<Boolean>()
        val barrier = consumers.barrier(finished)
        relay.offer(ConversationDelivery.AttachReplayEnded("session-a", submittedTurn, true, "r2",
            barrier = barrier, isActive = newFence))
        runCurrent()
        assertFalse(finished.isCompleted)
        assertTrue(oldFence())
        holdFirst.complete(Unit)
        runCurrent()
        assertTrue(finished.await())
        assertFalse(oldFence())
        relay.offer(ConversationDelivery.TurnEvent("session-a", turn, 3uL,
            ClientEvent.TextDelta(" end".toClientTextDto()), isActive = newFence))
        runCurrent()
        assertEquals("middle end", first.state.value.streamingMessage?.text)
        assertEquals("middle end", second.state.value.streamingMessage?.text)
        assertEquals(3L, first.durableReplayCursorForTesting())
        assertEquals(3L, second.durableReplayCursorForTesting())
    }

    @Test
    fun failedSecondAttachDoesNotStopTheExistingUi() = runTest(dispatcher) {
        val relay = LosslessEventRelay<ConversationDelivery>(backgroundScope)
        val consumers = AttachConsumerRegistry()
        var turn = 0L
        val source = object : ConversationSource {
            val never = MutableSharedFlow<ReplyEvent>()
            override val conversationDeliveries: Flow<ConversationDelivery> get() = consumers.track(relay.events)
            override fun submit(text: String, images: List<ImageRefDto>, turnId: Long,
                visualizationContext: VisualizationRef?, promptMode: PromptModeDto?): Flow<ReplyEvent> {
                turn = turnId
                return never.asSharedFlow()
            }
        }
        val first = ChatViewModel(source)
        first.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        first.send("question")
        val second = ChatViewModel(source)
        second.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        second.noteDurableAttach("session-a", turn, expectsResume = false, requestId = "r2")
        runCurrent()
        val failure = DurableAttachFailure(turn, "attach", IllegalStateException("timeout"))
        relay.offer(ConversationDelivery.AttachFailed("session-a", turn, failure, "r2"))
        relay.offer(ConversationDelivery.TurnEvent("session-a", turn.toULong(), 1uL,
            ClientEvent.TextDelta("still live".toClientTextDto())))
        relay.offer(ConversationDelivery.TurnEvent("session-a", turn.toULong(), null,
            ClientEvent.TextDelta(" unretained".toClientTextDto())))
        runCurrent()
        assertEquals("still live unretained", first.state.value.streamingMessage?.text)
        assertTrue(first.state.value.streaming)
        assertNotNull(first.state.value.error)
        assertTrue(second.state.value.durableRecoveryBlocked)
        assertEquals(0L, second.durableReplayCursorForTesting())
        relay.offer(ConversationDelivery.TurnEvent("session-a", turn.toULong(), 2uL,
            ClientEvent.TurnEnded(TurnOutcomeDto.END_TURN, null, cost())))
        runCurrent()
        assertFalse(first.state.value.streaming)
    }

    @Test
    fun retirementWaitsForTheReplyCollectorToConsumeItsTerminal() = runTest(dispatcher) {
        val relay = LosslessEventRelay<ConversationDelivery>(backgroundScope)
        val consumers = AttachConsumerRegistry()
        val ingress = DurableRecoveryIngress().newSource("store")
        ingress.acquireInterest("session-a", 1uL)
        val fence = ingress.fence("session-a", 1uL)
        val holdingReply = CompletableDeferred<Unit>()
        val received = mutableListOf<ReplyEvent>()
        val ui = backgroundScope.launch { consumers.track(relay.events).collect {} }
        val reply = backgroundScope.launch {
            val delayed = flow {
                consumers.track(relay.events, ui = false).collect { item ->
                    if (item is ConversationDelivery.TurnEvent) holdingReply.await()
                    emit(item)
                }
            }
            mapDeliveryReplyStream(delayed, acceptsTurn = { session, turn -> session == "session-a" && turn == 1uL })
                .collect { received += it }
        }
        runCurrent()
        relay.offer(ConversationDelivery.TurnEvent("session-a", 1uL, 1uL,
            ClientEvent.TurnEnded(TurnOutcomeDto.END_TURN, null, cost()), isActive = fence))
        val old = ingress.detachInterest("session-a", 1uL)!!
        val barrier = consumers.retirement { ingress.releaseInterest(old) }!!
        relay.offer(ConversationDelivery.RecoveryInterestEnded(barrier))
        runCurrent()
        assertTrue(fence())
        assertFalse(reply.isCompleted)
        holdingReply.complete(Unit)
        runCurrent()
        assertEquals(1, received.count { it is ReplyEvent.DurableTurnStreamEnded })
        assertTrue(reply.isCompleted)
        assertFalse(fence())
        ui.cancel()
    }

    @Test
    fun stopWaitsForPendingHostReservationAndUsesTheOriginalSession() = runTest(dispatcher) {
        val reservation = CompletableDeferred<Unit>()
        val entered = CompletableDeferred<Unit>()
        val calls = mutableListOf<String>()
        val source = object : ConversationSource {
            override val supportsSubmissionAdmission = true
            override val activeSessionState = MutableStateFlow<ActivatedSession?>(
                ActivatedSession("session-a", emptyList(), SessionActivationKind.Bootstrap))
            override fun submit(text: String, images: List<ImageRefDto>, turnId: Long,
                visualizationContext: VisualizationRef?, promptMode: PromptModeDto?, sessionId: String,
                admission: TurnSubmissionAdmission): Flow<ReplyEvent> = flow {
                assertTrue(admission.begin(sessionId))
                entered.complete(Unit)
                reservation.await()
                calls += "reserved:$sessionId:$turnId"
                admission.finished()
                kotlinx.coroutines.awaitCancellation()
            }
            override suspend fun cancelForSession(sessionId: String, turnId: Long?) {
                calls += "cancel:$sessionId:$turnId"
            }
        }
        val vm = ChatViewModel(source)
        vm.send("first")
        entered.await()
        vm.cancel()
        runCurrent()
        assertTrue(calls.isEmpty())
        assertTrue(vm.state.value.cancellationInFlight)
        reservation.complete(Unit)
        runCurrent()
        assertEquals(2, calls.size)
        assertTrue(calls[0].startsWith("reserved:session-a:"))
        assertEquals(calls[0].replace("reserved:", "cancel:"), calls[1])
        assertFalse(vm.state.value.cancellationInFlight)
    }

    @Test
    fun freshPromptWaitsForItsWorkspaceOwnerBeforeProjectingEnvelopes() = runTest(dispatcher) {
        val owner = CompletableDeferred<String>()
        val deliveries = MutableSharedFlow<ConversationDelivery>(extraBufferCapacity = 8)
        var checkpoint: Pair<String, Long>? = null
        val source = object : ConversationSource {
            val active = MutableStateFlow<ActivatedSession?>(null)
            override val activeSessionState = active.asStateFlow()
            override val conversationDeliveries = deliveries.asSharedFlow()
            override val supportsSubmissionAdmission = true
            override suspend fun submissionSession(): String = owner.await()
            override fun submit(text: String, images: List<ImageRefDto>, turnId: Long,
                visualizationContext: VisualizationRef?, promptMode: PromptModeDto?, sessionId: String,
                admission: TurnSubmissionAdmission): Flow<ReplyEvent> = flow {
                assertTrue(admission.begin(sessionId))
                checkpoint = sessionId to turnId
                admission.finished()
                kotlinx.coroutines.awaitCancellation()
            }
        }
        val vm = ChatViewModel(source)
        vm.send("first")
        runCurrent()
        assertNull(checkpoint)
        source.active.value = ActivatedSession("session-a", emptyList(), SessionActivationKind.Bootstrap)
        owner.complete("session-a")
        runCurrent()
        val (session, turn) = checkpoint!!
        assertEquals("session-a", session)
        deliveries.emit(ConversationDelivery.TurnEvent(session, turn.toULong(), 1uL, ClientEvent.TurnStarted(turn.toULong())))
        deliveries.emit(ConversationDelivery.TurnEvent(session, turn.toULong(), 2uL,
            ClientEvent.TextDelta("first answer".toClientTextDto())))
        runCurrent()
        assertEquals("first answer", vm.state.value.streamingMessage?.text)
        assertEquals(2L, vm.durableReplayCursorForTesting())
    }

    @Test
    fun stopBeforeWorkspaceBootstrapPreventsTheHostSubmission() = runTest(dispatcher) {
        val owner = CompletableDeferred<String>()
        var submissions = 0
        val abandoned = mutableListOf<Long>()
        val source = object : ConversationSource {
            override val supportsSubmissionAdmission = true
            override suspend fun submissionSession(): String = owner.await()
            override fun submit(text: String, images: List<ImageRefDto>, turnId: Long,
                visualizationContext: VisualizationRef?, promptMode: PromptModeDto?, sessionId: String,
                admission: TurnSubmissionAdmission): Flow<ReplyEvent> {
                submissions++
                return emptyFlow()
            }
            override suspend fun abandonUnsubmittedTurn(sessionId: String?, turnId: Long) { abandoned += turnId }
        }
        val vm = ChatViewModel(source)
        vm.send("first")
        runCurrent()
        vm.cancel()
        runCurrent()
        owner.complete("session-a")
        runCurrent()
        assertEquals(0, submissions)
        assertEquals(1, abandoned.size)
        assertFalse(vm.state.value.streaming)
        assertFalse(vm.state.value.cancellationInFlight)
    }

    @Test
    fun confirmedNotAdmittedSubmissionIsAbandonedWithoutHostCancel() = runTest(dispatcher) {
        val result = CompletableDeferred<Unit>()
        val entered = CompletableDeferred<Unit>()
        val calls = mutableListOf<String>()
        val source = object : ConversationSource {
            override val supportsSubmissionAdmission = true
            override val activeSessionState = MutableStateFlow<ActivatedSession?>(
                ActivatedSession("session-a", emptyList(), SessionActivationKind.Bootstrap))
            override fun submit(text: String, images: List<ImageRefDto>, turnId: Long,
                visualizationContext: VisualizationRef?, promptMode: PromptModeDto?, sessionId: String,
                admission: TurnSubmissionAdmission): Flow<ReplyEvent> = flow {
                assertTrue(admission.begin(sessionId))
                entered.complete(Unit)
                result.await()
                admission.finished(ClientException.SubmissionNotAdmitted("rejected"))
                kotlinx.coroutines.awaitCancellation()
            }
            override suspend fun abandonUnsubmittedTurn(sessionId: String?, turnId: Long) { calls += "abandon:$sessionId:$turnId" }
            override suspend fun cancelForSession(sessionId: String, turnId: Long?) { calls += "cancel:$sessionId:$turnId" }
        }
        val vm = ChatViewModel(source)
        vm.send("first")
        entered.await()
        vm.cancel()
        result.complete(Unit)
        runCurrent()
        assertEquals(1, calls.size)
        assertTrue(calls.single().startsWith("abandon:session-a:"))
        assertFalse(vm.state.value.durableRecoveryBlocked)
    }

    @Test
    fun unknownSubmissionNeverCompletesAndCanRetryTheOriginalOwner() = runTest(dispatcher) {
        val relay = LosslessEventRelay<ConversationDelivery>(backgroundScope)
        val consumers = AttachConsumerRegistry()
        val queries = mutableListOf<Pair<String, Long>>()
        val source = object : ConversationSource {
            override val supportsSubmissionAdmission = true
            override val activeSessionState = MutableStateFlow<ActivatedSession?>(
                ActivatedSession("session-a", emptyList(), SessionActivationKind.Bootstrap))
            override val conversationDeliveries: Flow<ConversationDelivery> get() = consumers.track(relay.events)
            override fun submit(text: String, images: List<ImageRefDto>, turnId: Long,
                visualizationContext: VisualizationRef?, promptMode: PromptModeDto?, sessionId: String,
                admission: TurnSubmissionAdmission): Flow<ReplyEvent> = flow {
                assertTrue(admission.begin(sessionId))
                admission.finished(ClientException.Rejected("uncertain"))
                emit(ReplyEvent.Error("request failed"))
                emit(ReplyEvent.End)
            }
            override suspend fun reconcileDurableTurnForUi(sessionId: String, turnId: Long, afterSequence: Long, trustNotFound: Boolean,
                onAttach: (String, Long, Boolean, String) -> Unit) {
                queries += sessionId to turnId
                onAttach(sessionId, turnId, false, "query-${queries.size}")
                if (queries.size == 1) throw IllegalStateException("offline")
                val completion = CompletableDeferred<Boolean>()
                val barrier = consumers.barrier(completion)
                relay.offer(ConversationDelivery.ReconciliationNotFound(sessionId, turnId, "query-2", barrier))
                assertTrue(completion.await())
            }
        }
        val vm = ChatViewModel(source)
        val completions = mutableListOf<ConversationTurnCompletion>()
        backgroundScope.launch { vm.turnCompletions.collect { completions += it } }
        runCurrent()
        vm.send("first")
        runCurrent()
        assertTrue(completions.isEmpty())
        assertTrue(vm.state.value.durableRecoveryBlocked)
        assertTrue(vm.state.value.recoveryRetryAvailable)
        assertEquals("session-a", queries.single().first)
        assertFalse(vm.openSessionFromNotification(SessionRef("session-b", "B")))
        assertEquals("session-a", vm.state.value.session.id)
        vm.retryDurableRecovery()
        runCurrent()
        assertEquals(2, queries.size)
        assertEquals(queries[0], queries[1])
        assertFalse(vm.state.value.durableRecoveryBlocked)
        assertFalse(vm.state.value.recoveryRetryAvailable)
        assertTrue(completions.isEmpty())
    }

    @Test
    fun restoredQueuedIdentityCanStartAfterTheEarlierTurnEnds() = runTest(dispatcher) {
        val deliveries = MutableSharedFlow<ConversationDelivery>(extraBufferCapacity = 8)
        val source = object : ConversationSource {
            override val conversationDeliveries: Flow<ConversationDelivery> = deliveries.asSharedFlow()
        }
        val vm = ChatViewModel(source)
        vm.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Resumed))
        runCurrent()
        deliveries.emit(ConversationDelivery.QueuedTurnRegistered("session-a", 72uL))
        deliveries.emit(ConversationDelivery.RecoveryState(TurnRecoverySnapshotDto(
            "session-a", 72uL, 1uL, TurnRecoveryStateDto.RUNNING, 1uL, 1uL, false, false, null,
        )))
        deliveries.emit(ConversationDelivery.TurnEvent("session-a", 72uL, 1uL, ClientEvent.TurnStarted(72uL)))
        deliveries.emit(ConversationDelivery.TurnEvent("session-a", 72uL, 2uL,
            ClientEvent.TextDelta("queued bash result".toClientTextDto())))
        runCurrent()
        assertEquals("queued bash result", vm.state.value.streamingMessage?.text)
        assertEquals(2L, vm.durableReplayCursorForTesting())
    }

    @Test
    fun terminalReceiptsWarnAfterReopenUntilTheUserReviewsThem() = runTest(dispatcher) {
        val source = object : ConversationSource {
            val active = MutableStateFlow<ActivatedSession?>(ActivatedSession("session-a", emptyList(), SessionActivationKind.Bootstrap))
            val receipts = mutableListOf(10L)
            override val activeSessionState = active.asStateFlow()
            override suspend fun terminalRecoveryReceipts(sessionId: String): List<Long> =
                if (sessionId == "session-a") receipts.toList() else emptyList()
            override suspend fun acknowledgeTerminalRecovery(sessionId: String, turns: List<Long>): Boolean {
                if (sessionId == "session-a") receipts.removeAll(turns.toSet())
                return true
            }
        }
        val vm = ChatViewModel(source)
        runCurrent()
        assertTrue(vm.state.value.terminalRecoveryReviewAvailable)
        assertTrue(vm.state.value.terminalRecoveryUncertain)
        vm.acknowledgeTerminalRecovery()
        runCurrent()
        assertFalse(vm.state.value.terminalRecoveryReviewAvailable)
        assertTrue(source.receipts.isEmpty())
    }

    @Test
    fun cancelledQueuedInputIsShownNowAndAfterReopenUntilReviewed() = runTest(dispatcher) {
        val preferences = FakeSharedPreferences()
        assertTrue(DurableConversationTurnClientStore(preferences, "scope-a").enqueue("session-a", 11L))
        class Source : ConversationSource {
            val store = DurableConversationTurnClientStore(preferences, "scope-a")
            val deliveries = MutableSharedFlow<ConversationDelivery>(extraBufferCapacity = 8)
            val queries = mutableListOf<String>()
            override val activeSessionState = MutableStateFlow<ActivatedSession?>(
                ActivatedSession("session-a", emptyList(), SessionActivationKind.Resumed))
            override val conversationDeliveries = deliveries.asSharedFlow()
            override suspend fun pendingQueuedRecoveryTurns(sessionId: String) = store.queued(sessionId).map { it.turnId }
            override suspend fun terminalRecoveryReceipts(sessionId: String) = store.terminalReceipts(sessionId).map { it.turnId }
            override suspend fun cancelledQueuedReceipts(sessionId: String) =
                store.terminalReceipts(sessionId).filter { it.cancelledBeforeStart }.map { it.turnId }
            override suspend fun acknowledgeTerminalRecovery(sessionId: String, turns: List<Long>) =
                store.acknowledgeTerminals(sessionId, turns)
            override suspend fun queryQueuedRecovery(sessionId: String, turnId: Long, requestId: String) { queries += requestId }
            override suspend fun resolveQueuedRecovery(sessionId: String, turns: List<Long>, resolution: QueuedRecoveryResolution): Boolean {
                assertEquals(QueuedRecoveryResolution.CancelledBeforeStart, resolution)
                val saved = turns.all { store.finishQueued(sessionId, it, cancelledBeforeStart = true) }
                if (saved) turns.forEach { deliveries.emit(ConversationDelivery.QueuedTurnSettled(
                    sessionId, it.toULong(), reviewRequired = true, cancelledBeforeStart = true)) }
                return saved
            }
        }
        val original = Source()
        val vm = ChatViewModel(original)
        runCurrent()
        original.deliveries.emit(ConversationDelivery.Event(ClientEvent.QueuedTurnStatus(
            "session-a", 11uL, original.queries.single(), QueuedTurnStatusDto.Cancelled)))
        runCurrent()
        assertTrue(vm.state.value.queuedCancellationNotice)
        assertTrue(vm.state.value.terminalRecoveryReviewAvailable)
        assertFalse(vm.state.value.terminalRecoveryUncertain)
        assertFalse(vm.state.value.queuedRecoveryWarning)
        assertNull(vm.state.value.error)
        assertTrue(original.store.queued("session-a").isEmpty())

        // Recreate the source before any review, as after a process restart.
        val reopened = Source()
        val restored = ChatViewModel(reopened)
        runCurrent()
        assertTrue(restored.state.value.queuedCancellationNotice)
        assertTrue(restored.state.value.terminalRecoveryReviewAvailable)
        assertFalse(restored.state.value.terminalRecoveryUncertain)
        restored.acknowledgeTerminalRecovery()
        runCurrent()
        assertFalse(restored.state.value.queuedCancellationNotice)
        assertFalse(restored.state.value.terminalRecoveryReviewAvailable)
        assertTrue(reopened.store.terminalReceipts("session-a").isEmpty())
    }

    @Test
    fun recoveryStorageFailureBlocksNewTurnsUntilTheAppRestarts() = runTest(dispatcher) {
        val deliveries = MutableSharedFlow<ConversationDelivery>(extraBufferCapacity = 8)
        var submissions = 0
        val source = object : ConversationSource {
            override val conversationDeliveries = deliveries.asSharedFlow()
            override fun submit(text: String): Flow<ReplyEvent> { submissions++; return emptyFlow() }
        }
        val vm = ChatViewModel(source)
        vm.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        runCurrent()
        deliveries.emit(ConversationDelivery.RecoveryStorageFailed)
        runCurrent()
        assertTrue(vm.state.value.recoveryStorageFailed)
        assertNotNull(vm.state.value.error)
        vm.dismissError()
        vm.send("must not start")
        runCurrent()
        assertEquals(0, submissions)
        assertNotNull(vm.state.value.error)
    }

    @Test
    fun queuedStatusRequiresRequestSessionTurnAndTheProjectedAggregateUuid() = runTest(dispatcher) {
        val deliveries = MutableSharedFlow<ConversationDelivery>(extraBufferCapacity = 8)
        val queries = mutableListOf<Pair<Long, String>>()
        val settled = mutableListOf<List<Long>>()
        val row = Message(Role.User, "same text", transcriptUuid = "aggregate-a")
        val source = object : ConversationSource {
            override val activeSessionState = MutableStateFlow<ActivatedSession?>(ActivatedSession(
                "session-a", listOf(row), SessionActivationKind.Resumed,
                fullyVisibleUserUuids = setOf("aggregate-a")))
            override val conversationDeliveries = deliveries.asSharedFlow()
            override suspend fun pendingQueuedRecoveryTurns(sessionId: String) = listOf(11L, 12L)
            override suspend fun queryQueuedRecovery(sessionId: String, turnId: Long, requestId: String) {
                queries += turnId to requestId
            }
            override suspend fun resolveQueuedRecovery(sessionId: String, turns: List<Long>, resolution: QueuedRecoveryResolution): Boolean {
                assertEquals("session-a", sessionId)
                assertEquals(QueuedRecoveryResolution.Delivered, resolution)
                settled += turns
                return true
            }
        }
        val vm = ChatViewModel(source)
        runCurrent()
        val request = queries.first { it.first == 11L }.second
        val status = QueuedTurnStatusDto.Delivered(11uL, listOf(11uL, 12uL), "aggregate-a")
        deliveries.emit(ConversationDelivery.Event(ClientEvent.QueuedTurnStatus("session-b", 11uL, request, status)))
        deliveries.emit(ConversationDelivery.Event(ClientEvent.QueuedTurnStatus("session-a", 12uL, request, status)))
        runCurrent()
        assertTrue(settled.isEmpty())
        deliveries.emit(ConversationDelivery.Event(ClientEvent.QueuedTurnStatus("session-a", 11uL, request, status)))
        runCurrent()
        assertEquals(listOf(listOf(11L, 12L)), settled)
    }

    @Test
    fun overlappingDeliveredResponsesForOneBatchDoNotReportAFalseFailure() = runTest(dispatcher) {
        val preferences = FakeSharedPreferences()
        val store = DurableConversationTurnClientStore(preferences, "scope-a")
        assertTrue(store.enqueue("session-a", 11L))
        assertTrue(store.enqueue("session-a", 12L))
        val deliveries = MutableSharedFlow<ConversationDelivery>(extraBufferCapacity = 8)
        val queries = mutableListOf<Pair<Long, String>>()
        val results = mutableListOf<Boolean>()
        val row = Message(Role.User, "both", transcriptUuid = "aggregate-a")
        val source = object : ConversationSource {
            override val activeSessionState = MutableStateFlow<ActivatedSession?>(ActivatedSession(
                "session-a", listOf(row), SessionActivationKind.Resumed, fullyVisibleUserUuids = setOf("aggregate-a")))
            override val conversationDeliveries = deliveries.asSharedFlow()
            override suspend fun pendingQueuedRecoveryTurns(sessionId: String) = store.queued(sessionId).map { it.turnId }
            override suspend fun queryQueuedRecovery(sessionId: String, turnId: Long, requestId: String) { queries += turnId to requestId }
            override suspend fun resolveQueuedRecovery(sessionId: String, turns: List<Long>, resolution: QueuedRecoveryResolution): Boolean {
                assertEquals(QueuedRecoveryResolution.Delivered, resolution)
                val saved = turns.all { store.removeDeliveredQueued(sessionId, it) }
                results += saved
                // Intentionally hold the settled relay marker until both responses arrive.
                return saved
            }
        }
        val vm = ChatViewModel(source)
        runCurrent()
        val status = QueuedTurnStatusDto.Delivered(11uL, listOf(11uL, 12uL), "aggregate-a")
        queries.forEach { (turn, request) ->
            deliveries.emit(ConversationDelivery.Event(ClientEvent.QueuedTurnStatus("session-a", turn.toULong(), request, status)))
        }
        runCurrent()
        assertEquals(listOf(true, true), results)
        assertTrue(store.queued("session-a").isEmpty())
        assertTrue(store.isHealthy())
        assertFalse(vm.state.value.statusLine?.contains("暂时无法确认") == true)
    }

    @Test
    fun queuedStatusQueriesAdvancePastTheFirst512PendingItems() = runTest(dispatcher) {
        for (respondToPending in listOf(true, false)) {
            val relay = LosslessEventRelay<ConversationDelivery>(backgroundScope)
            val consumers = AttachConsumerRegistry()
            val seen = mutableListOf<Long>()
            val settled = mutableListOf<Long>()
            val row = Message(Role.User, "last", transcriptUuid = "aggregate-last")
            val source = object : ConversationSource {
                override val activeSessionState = MutableStateFlow<ActivatedSession?>(ActivatedSession(
                    "session-a", listOf(row), SessionActivationKind.Resumed,
                    fullyVisibleUserUuids = setOf("aggregate-last")))
                override val conversationDeliveries: Flow<ConversationDelivery> get() = consumers.track(relay.events)
                override suspend fun pendingQueuedRecoveryTurns(sessionId: String) = (1L..513L).toList()
                override suspend fun queryQueuedRecovery(sessionId: String, turnId: Long, requestId: String) {
                    consumers.awaitSubscriber()
                    seen += turnId
                    val status = if (turnId == 513L) {
                        QueuedTurnStatusDto.Delivered(513uL, listOf(513uL), "aggregate-last")
                    } else QueuedTurnStatusDto.Pending
                    if (respondToPending || turnId == 513L) {
                        relay.offer(ConversationDelivery.Event(ClientEvent.QueuedTurnStatus(sessionId, turnId.toULong(), requestId, status)))
                    }
                }
                override suspend fun resolveQueuedRecovery(sessionId: String, turns: List<Long>, resolution: QueuedRecoveryResolution): Boolean {
                    settled += turns
                    return true
                }
            }
            ChatViewModel(source)
            runCurrent()
            if (!respondToPending) {
                testScheduler.advanceTimeBy(160_001)
                runCurrent()
            }
            assertEquals(513, seen.size)
            assertEquals(listOf(513L), settled)
        }
    }
    @Test
    fun queuedPromptKeepsActionSessionWhenNotificationSwitchesBeforeDelivery() = runTest(dispatcher) {
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        val commands = mutableListOf<ClientCommand.SendPrompt>()
        val accepted = mutableListOf<String>()
        val source = object : ConversationSource {
            val active = MutableStateFlow<ActivatedSession?>(ActivatedSession("session-a", emptyList(), SessionActivationKind.Bootstrap))
            override val activeSessionState = active.asStateFlow()
            override val supportsSubmissionAdmission = true
            override fun submit(text: String, images: List<ImageRefDto>, turnId: Long,
                visualizationContext: VisualizationRef?, promptMode: PromptModeDto?, sessionId: String,
                admission: TurnSubmissionAdmission): Flow<ReplyEvent> = flow {
                assertTrue(admission.begin(sessionId))
                admission.finished()
                kotlinx.coroutines.awaitCancellation()
            }
            override suspend fun cancelForSession(sessionId: String, turnId: Long?) = Unit
            override suspend fun resumeSession(uuid: String) {
                active.value = ActivatedSession(uuid, emptyList(), SessionActivationKind.Resumed)
            }
            override suspend fun submitClientCommand(command: ClientCommand) {
                if (command !is ClientCommand.SendPrompt) return
                commands += command
                entered.complete(Unit)
                release.await()
                if (command.sessionId == active.value?.sessionId) accepted += command.sessionId!!
            }
        }
        val vm = ChatViewModel(source)
        vm.send("first")
        vm.send("!queued")
        entered.await()
        assertTrue(vm.openSessionFromNotification(SessionRef("session-b", "B")))
        runCurrent()
        assertEquals("session-b", vm.state.value.session.id)
        release.complete(Unit)
        runCurrent()
        assertEquals("session-a", commands.single().sessionId)
        assertTrue(accepted.isEmpty())
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
        vm.noteDurableAttach("session-a", 91L, true)
        vm.reduceClientEvent(
            ClientEvent.TurnRecoveryState(
                TurnRecoverySnapshotDto(
                    sessionId = "session-a",
                    turnId = 91u,
                    revision = 1uL,
                    state = TurnRecoveryStateDto.RUNNING,
                    firstSequence = 0u,
                    lastSequence = 0u,
                    safeToResume = true,
                    projectionUncertain= false,
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
            val source = DurableAttachSource(turnId = 92L)
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
                revision = 1uL,
                state = TurnRecoveryStateDto.WAITING_FOR_USER,
                firstSequence = 0u,
                lastSequence = 0u,
                safeToResume = false,
                projectionUncertain= false,
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
                    waiting.copy(revision = 2uL, state = TurnRecoveryStateDto.CANCELLED),
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
                        revision = 1uL,
                        state = TurnRecoveryStateDto.WAITING_FOR_USER,
                        firstSequence = 0u,
                        lastSequence = 0u,
                        safeToResume = false,
                        projectionUncertain= false,
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
                TaskRowDto("workflow-1", "workflow", TaskStatusDto.RUNNING, "review".toClientTextDto(), false, null, null),
            ),
        )
        val waiting = TurnRecoverySnapshotDto(
            sessionId = "session-a",
            turnId = 93u,
            revision = 1uL,
            state = TurnRecoveryStateDto.WAITING_FOR_USER,
            firstSequence = 0u,
            lastSequence = 1u,
            safeToResume = false,
            projectionUncertain= false,
            reason = "waiting_for_user",
        )
        vm.noteDurableAttach("session-a", 93L, true)
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
        vm.finishDurableAttachReplay(ConversationDelivery.AttachReplayEnded("session-a", 93L, true))
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(waiting))

        assertTrue(vm.state.value.durableRecoveryBlocked)
        assertFalse(vm.state.value.agentRun?.active == true)
        assertTrue(vm.state.value.agentRun?.tools.orEmpty().all { it.status != AgentToolStatus.Running })
        assertTrue(vm.state.value.activeBackgroundTaskIds.contains("workflow-1"))
        assertTrue(vm.state.value.requiresBackgroundExecution)
        // The remaining lease is the unrelated workflow task, not the parked
        // recovered turn's unmatched tool/run state.
        vm.reduceClientEvent(
            ClientEvent.TaskStatusChanged("workflow-1", TaskStatusDto.PAUSED, null, null),
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
                    revision = 2uL,
                    state = TurnRecoveryStateDto.CANCELLED,
                    firstSequence = 0u,
                    lastSequence = 1u,
                    safeToResume = false,
                    projectionUncertain= false,
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
    fun retryAfterOrderedResumeFailureKeepsProjectedCursorAndResumes() = runTest(dispatcher) {
        val source = FailingDurableAttachSource()
        val vm = ChatViewModel(source)
        source.active.value = ActivatedSession("session-a", emptyList(), SessionActivationKind.Resumed)
        runCurrent()
        assertTrue(vm.state.value.durableRecoveryBlocked)
        assertEquals(1L, vm.durableReplayCursorForTesting())

        vm.noteDurableAttach("session-a", 94L, true)
        val paused = TurnRecoverySnapshotDto(
            sessionId = "session-a", turnId = 94uL, revision = 1uL,
            state = TurnRecoveryStateDto.PAUSED_RECOVERABLE, firstSequence = 0uL, lastSequence = 1uL,
            safeToResume = true, projectionUncertain = false, reason = "retry attach",
        )
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(paused))
        vm.finishDurableAttachReplay(ConversationDelivery.AttachReplayEnded("session-a", 94L, true))
        vm.reduceClientEvent(ClientEvent.TurnRecoveryState(paused.copy(
            revision = 2uL, state = TurnRecoveryStateDto.RUNNING, reason = "resumed",
        )))
        assertEquals(1L, vm.durableReplayCursorForTesting())
        assertTrue(vm.state.value.streaming)
        assertFalse(vm.state.value.durableRecoveryBlocked)
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
                        revision = 2uL,
                        state = terminalState,
                        firstSequence = 0u,
                        lastSequence = 1u,
                        safeToResume = false,
                        projectionUncertain= false,
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

        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.RUNNING, null, null))
        runCurrent()
        assertEquals(listOf(true), execution.activeStates)

        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.PAUSED, null, null))
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
                error = null,
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
                error = null,
            ),
        )
        assertEquals("后台任务 task-a 运行中", vm.state.value.statusLine)
    }

    @Test
    fun lateTaskCompletionDoesNotNotifyForAnotherSessionOrSuppressItsTask() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val vm = ChatViewModel(source = RecordingSource(), backgroundExecution = execution)
        vm.applyActivatedSession(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started))
        vm.reduceClientEvent(ClientEvent.TaskRow(
            TaskRowDto("task-1", "shell", TaskStatusDto.EXITED_UNKNOWN, "A".toClientTextDto(), false, null, null),
        ))
        vm.applyActivatedSession(ActivatedSession("session-b", emptyList(), SessionActivationKind.Resumed))
        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.COMPLETED, "session-a", null))
        assertTrue(execution.finishedTasks.isEmpty())

        vm.reduceClientEvent(ClientEvent.TaskRow(
            TaskRowDto("task-1", "shell", TaskStatusDto.RUNNING, "B".toClientTextDto(), false, null, null),
        ))
        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.COMPLETED, "session-b", null))
        assertEquals(listOf(Triple("task-1", "B", false)), execution.finishedTasks)
    }

    @Test
    fun backgroundTaskLeaseIsReleasedWhenSwitchingSessions() = runTest(dispatcher) {
        val execution = RecordingBackgroundExecution()
        val source = EmittingSource()
        val vm = ChatViewModel(
            source = source,
            backgroundExecution = execution,
        )

        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.RUNNING, null, null))
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

        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("task-1", TaskStatusDto.COMPLETED, null, null))
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
                TaskRowDto("task-1", "shell", TaskStatusDto.RUNNING, "npm test".toClientTextDto(), false, null, null),
            ),
        )
        runCurrent()
        assertEquals(listOf(true), execution.activeStates)

        vm.reduceClientEvent(
            ClientEvent.TaskRow(
                TaskRowDto("task-1", "shell", TaskStatusDto.COMPLETED, "npm test".toClientTextDto(), false, null, null),
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
        assertEquals(true, s.streamingMessage?.streamed)
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
        assertTrue(state.messages.last().streamed)
    }

    @Test
    fun messageCompletePreservesStreamedDisclosureAndRestoredHistoryStaysCompact() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Delta("draft"))
        val streamingId = vm.state.value.streamingMessage?.id
        vm.reduce(ReplyEvent.MessageComplete(Message(role = Role.Ai, text = "final")))
        assertEquals(streamingId, vm.state.value.messages.last().id)
        assertTrue(vm.state.value.messages.last().streamed)
        assertFalse(Message(role = Role.Ai, text = "restored history").streamed)
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
                tool = "Shell",
                status = AgentToolStatus.Running,
                syntheticShellCommand = "/home/lingxi/lingxi/index.html",
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
        assertEquals("/home/lingxi/lingxi/index.html", tools.single().syntheticShellCommand)
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
        assertEquals(listOf("second".map { it.code.toUShort() }), src.pending)
        assertEquals(2, vm.state.value.messages.count { it.role == Role.User })
        assertTrue(vm.state.value.streaming)
    }

    @Test
    fun send_whileStreamingPreservesExactUtf16Units() = runTest(dispatcher) {
        val source = RecordingSource()
        val vm = ChatViewModel(source)
        val prompt = String(charArrayOf('x', 0xD800.toChar(), 0xD83D.toChar(), 0xDE03.toChar()))

        vm.send("first")
        vm.send(prompt)
        runCurrent()

        val command = source.commands.filterIsInstance<ClientCommand.SendPrompt>().single()
        assertEquals(prompt.map { it.code.toUShort() }, command.text.codeUnits)
        assertEquals(prompt.map { it.code.toUShort() }, source.pending.single())
    }

    @Test
    fun sessionAgentTextProjectsToDisplayOnlyAtTheUiBoundary() {
        val summary = SessionAgentSummaryDto(
            agentId = "agent:worker",
            name = ClientTextDto(codeUnits = listOf('A'.code.toUShort(), 0xD800.toUShort())),
            agentType = "worker",
            model = null,
            modelProfile = null,
            status = "running",
            latestActivity = ClientTextDto(
                codeUnits = listOf('x'.code.toUShort(), 0xD83D.toUShort(), 0xDE03.toUShort()),
            ),
            updatedAtMs = null,
        )

        val ui = summary.toSessionAgentUi()

        assertEquals("A�", ui.name)
        assertEquals("x😃", ui.latestActivity)
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
    fun workspaceSwitchReusingCurrentSourceKeepsItAliveForSuccessiveNewSessions() = runTest(dispatcher) {
        val reused = SessionControlSource()
        reused.active.value = ActivatedSession("existing", emptyList(), SessionActivationKind.Started)
        val vm = ChatViewModel(reused)

        repeat(2) { index ->
            assertTrue(vm.switchWorkspaceSource(projectId = null, createSource = { reused }))
            runCurrent()
            assertFalse("a reused source still owns the next session", reused.closed)
            assertEquals(List(index + 1) { "new" }, reused.operations)
            reused.active.value = ActivatedSession("created-$index", emptyList(), SessionActivationKind.Started)
            runCurrent()
            assertEquals("created-$index", vm.state.value.session.id)
            assertTrue(vm.state.value.sessionReady)
            assertFalse(vm.state.value.sessionTransitioning)
            assertSame(reused, vm.engineSource.value)
        }
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
    fun workspaceSwitchRefusesRunningBackgroundTaskBeforeBuildingReplacement() = runTest(dispatcher) {
        val original = StubSource()
        val vm = ChatViewModel(original)
        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("background", TaskStatusDto.RUNNING, null, null))
        var built = false

        val switched = vm.switchWorkspaceSource(
            projectId = "project-b",
            createSource = { built = true; SessionControlSource() },
        )

        assertFalse(switched)
        assertFalse(built)
        assertFalse(original.closed)
        assertTrue(vm.state.value.activeBackgroundTaskIds.contains("background"))
        assertSame(original, vm.engineSource.value)
    }

    @Test
    fun backgroundWorkPublishedDuringBuildKeepsOriginalSource() = runTest(dispatcher) {
        val original = StubSource()
        val replacement = SessionControlSource()
        val vm = ChatViewModel(original)
        var persisted = false

        val switched = vm.switchWorkspaceSource(
            projectId = "project-b",
            createSource = {
                vm.reduceClientEvent(ClientEvent.TaskStatusChanged("background", TaskStatusDto.RUNNING, null, null))
                replacement
            },
            persistSelection = { persisted = true },
        )

        assertFalse(switched)
        assertFalse(persisted)
        assertFalse(original.closed)
        assertTrue(replacement.closed)
        assertSame(original, vm.engineSource.value)
    }

    @Test
    fun backgroundWorkArrivingDuringPersistenceRollsBackAndKeepsRuntime() = runTest(dispatcher) {
        val original = StubSource()
        val replacement = SessionControlSource()
        val vm = ChatViewModel(original)
        val persisted = CompletableDeferred<Unit>()
        val releasePersistence = CompletableDeferred<Unit>()
        var durableSelection = "global"
        var rollbackCount = 0
        val switch = async(start = CoroutineStart.UNDISPATCHED) {
            vm.switchWorkspaceSource(
                projectId = "project-b",
                createSource = { replacement },
                persistSelection = {
                    durableSelection = "project-b"
                    persisted.complete(Unit)
                    releasePersistence.await()
                },
                rollbackSelection = { durableSelection = "global"; rollbackCount++ },
            )
        }
        persisted.await()
        vm.reduceClientEvent(ClientEvent.TaskStatusChanged("background", TaskStatusDto.RUNNING, null, null))
        releasePersistence.complete(Unit)

        assertFalse(switch.await())
        assertEquals("global", durableSelection)
        assertEquals(1, rollbackCount)
        assertFalse(original.closed)
        assertTrue(replacement.closed)
        assertTrue(vm.state.value.activeBackgroundTaskIds.contains("background"))
        assertSame(original, vm.engineSource.value)
    }

    @Test
    fun unknownAdmissionDuringWorkspaceBuildOrPersistenceKeepsTheOriginalOwner() = runTest(dispatcher) {
        for (pauseDuringPersistence in listOf(false, true)) {
            val entered = CompletableDeferred<Unit>()
            val release = CompletableDeferred<Unit>()
            val queried = mutableListOf<Pair<String, Long>>()
            val original = object : ConversationSource {
                var closed = false
                override val supportsSubmissionAdmission = true
                override val activeSessionState = MutableStateFlow<ActivatedSession?>(
                    ActivatedSession("session-a", emptyList(), SessionActivationKind.Bootstrap))
                override fun submit(text: String, images: List<ImageRefDto>, turnId: Long,
                    visualizationContext: VisualizationRef?, promptMode: PromptModeDto?, sessionId: String,
                    admission: TurnSubmissionAdmission): Flow<ReplyEvent> = flow {
                    assertTrue(admission.begin(sessionId))
                    admission.finished(ClientException.Rejected("unknown admission"))
                    emit(ReplyEvent.Error("unknown admission"))
                    emit(ReplyEvent.End)
                }
                override suspend fun reconcileDurableTurnForUi(sessionId: String, turnId: Long, afterSequence: Long,
                    trustNotFound: Boolean, onAttach: (String, Long, Boolean, String) -> Unit) {
                    queried += sessionId to turnId
                    throw IllegalStateException("offline")
                }
                override fun close() { closed = true }
            }
            val replacement = SessionControlSource()
            val vm = ChatViewModel(original)
            runCurrent()
            var selection = "global"
            var rollbacks = 0
            val switching = async(start = CoroutineStart.UNDISPATCHED) {
                vm.switchWorkspaceSource(projectId = "project-b", allowInactiveWaitingRecovery = true,
                    createSource = {
                        if (!pauseDuringPersistence) { entered.complete(Unit); release.await() }
                        replacement
                    },
                    persistSelection = {
                        selection = "project-b"
                        if (pauseDuringPersistence) { entered.complete(Unit); release.await() }
                    },
                    rollbackSelection = { selection = "global"; rollbacks++ })
            }
            entered.await()
            vm.send("send while switching")
            runCurrent()
            assertTrue(vm.state.value.recoveryRetryAvailable)
            assertEquals("session-a", queried.single().first)
            release.complete(Unit)
            assertFalse(switching.await())
            assertSame(original, vm.engineSource.value)
            assertFalse(original.closed)
            assertTrue(replacement.closed)
            assertEquals("global", selection)
            assertEquals(if (pauseDuringPersistence) 1 else 0, rollbacks)
            vm.retryDurableRecovery()
            runCurrent()
            assertEquals(2, queried.size)
            assertEquals(queried.first(), queried.last())
        }
    }

    @Test
    fun recoveryStorageFailureDuringWorkspacePersistenceRollsBackSelection() = runTest(dispatcher) {
        val original = object : ConversationSource {
            var closed = false
            var healthy = true
            override suspend fun recoveryStorageAvailable() = healthy
            override fun close() { closed = true }
        }
        val replacement = SessionControlSource()
        val vm = ChatViewModel(original)
        runCurrent()
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        var selection = "global"
        val switching = async(start = CoroutineStart.UNDISPATCHED) {
            vm.switchWorkspaceSource(projectId = "project-b", createSource = { replacement },
                persistSelection = { selection = "project-b"; entered.complete(Unit); release.await() },
                rollbackSelection = { selection = "global" })
        }
        entered.await()
        original.healthy = false // The relay has not delivered its failure marker yet.
        release.complete(Unit)
        assertFalse(switching.await())
        assertEquals("global", selection)
        assertSame(original, vm.engineSource.value)
        assertFalse(original.closed)
        assertTrue(replacement.closed)
        assertTrue(vm.state.value.recoveryStorageFailed)
    }

    @Test
    fun failedPartialSelectionWriteInvokesRollbackWithoutClosingOriginal() = runTest(dispatcher) {
        val original = StubSource()
        val replacement = SessionControlSource()
        val vm = ChatViewModel(original)
        var selection = "global"

        val switched = vm.switchWorkspaceSource(
            projectId = "project-b",
            createSource = { replacement },
            persistSelection = { selection = "project-b"; error("injected scope write failure") },
            rollbackSelection = { selection = "global" },
        )

        assertFalse(switched)
        assertEquals("global", selection)
        assertFalse(original.closed)
        assertTrue(replacement.closed)
        assertSame(original, vm.engineSource.value)
    }

    @Test
    fun cancelledSelectionWriteRollsBackInNonCancellableContext() = runTest(dispatcher) {
        val original = StubSource()
        val replacement = SessionControlSource()
        val vm = ChatViewModel(original)
        val persisted = CompletableDeferred<Unit>()
        var selection = "global"
        var rolledBack = false
        val switch = async(start = CoroutineStart.UNDISPATCHED) {
            vm.switchWorkspaceSource(
                projectId = "project-b",
                createSource = { replacement },
                persistSelection = {
                    selection = "project-b"
                    persisted.complete(Unit)
                    CompletableDeferred<Unit>().await()
                },
                rollbackSelection = {
                    kotlinx.coroutines.yield()
                    selection = "global"
                    rolledBack = true
                },
            )
        }
        persisted.await()
        switch.cancel()
        switch.join()

        assertTrue(rolledBack)
        assertEquals("global", selection)
        assertFalse(original.closed)
        assertTrue(replacement.closed)
        assertSame(original, vm.engineSource.value)
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
    fun reconnectRemainsBlockedUntilCancellationIsAcknowledged() = runTest(dispatcher) {
        val gate = CompletableDeferred<Unit>()
        val source = SessionControlSource(cancelGate = gate)
        val vm = ChatViewModel(source)
        vm.send("work")
        vm.cancel()
        runCurrent()
        assertFalse(vm.state.value.streaming)
        assertTrue(vm.state.value.cancellationInFlight)
        assertTrue(vm.state.value.blocksEngineReconnect(hasPendingPermission = false))
        gate.complete(Unit)
        runCurrent()
        assertFalse(vm.state.value.cancellationInFlight)
        assertFalse(vm.state.value.blocksEngineReconnect(hasPendingPermission = false))
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
    fun enginePreparationReturnsBeforeNativeConstructionAndAdoptsOnMain() = runTest(dispatcher) {
        val original = CloseTrackingSource(); val prepared = CloseTrackingSource()
        val vm = ChatViewModel(source = original, sourceGeneration = -1)
        var calls = 0
        vm.ensureSourceInBackground(0, StandardTestDispatcher(testScheduler)) { calls++; prepared }
        assertEquals("UI caller must return before native construction", 0, calls)
        assertSame(original, vm.engineSource.value)
        runCurrent()
        assertEquals(1, calls)
        assertSame(prepared, vm.engineSource.value)
        assertEquals(1, original.closeCount)
    }

    @Test
    fun obsoleteBackgroundPreparationIsClosedWithoutReplacingTheNewGeneration() = runTest(dispatcher) {
        val original = CloseTrackingSource(); val obsolete = CloseTrackingSource(); val latest = CloseTrackingSource()
        val vm = ChatViewModel(source = original, sourceGeneration = -1)
        val io = StandardTestDispatcher(testScheduler)
        vm.ensureSourceInBackground(0, io) { obsolete }
        vm.ensureSourceInBackground(1, io) { latest }
        runCurrent()
        assertSame(latest, vm.engineSource.value)
        assertEquals(1, obsolete.closeCount)
        assertEquals(1, original.closeCount)
        assertEquals(0, latest.closeCount)
    }

    @Test
    fun nativeHandleReturnedAfterViewModelDestructionIsClosed() = runTest(dispatcher) {
        val original = CloseTrackingSource(); val abandoned = CloseTrackingSource()
        val vm = ChatViewModel(source = original, sourceGeneration = -1)
        val store = androidx.lifecycle.ViewModelStore().apply { put("chat", vm) }
        vm.ensureSourceInBackground(0, StandardTestDispatcher(testScheduler)) { abandoned }
        store.clear()
        runCurrent()
        assertEquals(1, original.closeCount)
        assertEquals(1, abandoned.closeCount)
    }

    @Test
    fun cancelledPreparationClosesTheReturnedOwnedHandle() = runTest(dispatcher) {
        val abandoned = CloseTrackingSource()
        lateinit var pending: kotlinx.coroutines.Deferred<ConversationSource>
        pending = async(start = CoroutineStart.LAZY) {
            prepareConversationSource(dispatcher = StandardTestDispatcher(testScheduler)) {
                pending.cancel()
                abandoned
            }
        }
        pending.start()
        runCurrent()
        assertTrue(pending.isCancelled)
        assertEquals(1, abandoned.closeCount)
    }

    @Test
    fun cancelledPreparationKeepsABorrowedProcessSourceOpen() = runTest(dispatcher) {
        val borrowed = CloseTrackingSource()
        lateinit var pending: kotlinx.coroutines.Deferred<ConversationSource>
        pending = async(start = CoroutineStart.LAZY) {
            prepareConversationSource(borrowed, StandardTestDispatcher(testScheduler)) {
                pending.cancel()
                borrowed
            }
        }
        pending.start()
        runCurrent()
        assertTrue(pending.isCancelled)
        assertEquals(0, borrowed.closeCount)
    }

    @Test
    fun reconnect_handedTheLiveSourceBack_keepsItOpen() = runTest(dispatcher) {
        val original = CloseTrackingSource()
        val vm = ChatViewModel(source = original, sourceGeneration = 1)

        vm.ensureSource(2) { original }
        runCurrent()

        assertEquals("the live source must not be destroyed as its own replacement", 0, original.closeCount)
        assertNull(vm.state.value.error)
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
            ClientEvent.TextDelta("好的".toClientTextDto()),
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
                message = MessageDto(transcriptUuid = null, role = "assistant", blocks = listOf(MessageBlockDto.Text("好的".toClientTextDto()))),
            ),
        )

        // Premise check: the wire message really does arrive with no tool block.
        val wireOnly = messageDtoToMessage(
            MessageDto(transcriptUuid = null, role = "assistant", blocks = listOf(MessageBlockDto.Text("好的".toClientTextDto()))),
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
                ClientEvent.TextDelta("查完了".toClientTextDto()),
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
            ClientEvent.TextDelta("改好了".toClientTextDto()),
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
            ClientEvent.TextDelta("跑一下".toClientTextDto()),
            ClientEvent.ToolUseStarted(
                id = "sh1",
                tool = "bash",
                inputJson = """{"command":"echo ok","cwd":"/home/lingxi/lingxi"}""",
                header = currentToolHeaderFixture(label = "Bash"),
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
    @Test fun appAuthoringWaitsForConfirmedSessionAndWorkspace() = runTest {
        val source = SessionControlSource()
        val vm = ChatViewModel(source = source)
        vm.startNewSession()
        runCurrent()
        val pending = async { vm.awaitAppAuthoringSession("/home/lingxi/source") }
        runCurrent()
        assertFalse(pending.isCompleted)
        source.active.value = ActivatedSession("restored-app-session", emptyList(), SessionActivationKind.Started)
        runCurrent()
        assertFalse(pending.isCompleted)
        source.workspace.value = com.lingxi.code.project.SessionWorkspaceState("restored-app-session", "/home/lingxi/source", "/home/lingxi/source", emptyList(), "/transcript")
        testScheduler.advanceTimeBy(50)
        runCurrent()
        assertEquals("restored-app-session", pending.await().sessionId)
        assertEquals("restored-app-session", vm.awaitAppAuthoringSession("/home/lingxi/source", "restored-app-session").sessionId)
    }

    @Test fun appAuthoringRejectsConfirmedSessionInAnotherDirectory() = runTest {
        val source = SessionControlSource()
        val vm = ChatViewModel(source = source)
        source.active.value = ActivatedSession("app-session", emptyList(), SessionActivationKind.Started)
        source.workspace.value = com.lingxi.code.project.SessionWorkspaceState("app-session", "/home/lingxi/other", "/home/lingxi/other", emptyList(), "/transcript")
        runCurrent()
        val failure = runCatching { vm.awaitAppAuthoringSession("/home/lingxi/source", "app-session") }.exceptionOrNull()
        assertTrue(failure is IllegalStateException)
        assertTrue(failure!!.message!!.contains("切换目录"))
    }

    @Test fun recoveredAuthoringSessionPassesExplicitDirectoryToOrdinaryEngine() = runTest {
        val previous = SessionControlSource()
        val reused = SessionControlSource()
        reused.workspace.value = com.lingxi.code.project.SessionWorkspaceState("old-session", "/home/lingxi/moved", "/home/lingxi/source", emptyList(), "/transcript")
        val vm = ChatViewModel(source = previous)
        assertTrue(vm.switchWorkspaceSource(projectId = "authoring-project", newSessionDirectory = "/home/lingxi/source", createSource = { reused }))
        runCurrent()
        assertTrue(reused.operations.contains("new:/home/lingxi/source"))
        assertFalse(vm.state.value.sessionReady)
    }

}
