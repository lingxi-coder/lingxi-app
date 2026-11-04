package com.lingxi.code.conversation

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModelStore
import com.lingxi.code.bindings.android.AndroidNativeMediaHost
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.CostDto
import com.lingxi.code.bindings.client.MessageBlockDto
import com.lingxi.code.bindings.client.MessageDto
import com.lingxi.code.bindings.client.TaskStatusDto
import com.lingxi.code.bindings.client.TurnOutcomeDto
import com.lingxi.code.bindings.client.TurnRecoverySnapshotDto
import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import com.lingxi.code.bindings.runtime.MobileEngineHandle
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRef
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class RetainedConversationTakeoverTest {
    private val dispatcher = UnconfinedTestDispatcher()

    @Before fun setMain() = Dispatchers.setMain(dispatcher)
    @After fun resetMain() = Dispatchers.resetMain()

    @Test
    fun realSourceRetainsEarlierTurnsAndReplaysTheRunningTurnWithoutSwitching() = runTest(dispatcher) {
        exerciseTakeover(terminal = false)
    }

    @Test
    fun realSourceReplaysATerminalTurnAfterItsClientRecordWasCleared() = runTest(dispatcher) {
        exerciseTakeover(terminal = true)
    }

    @Test
    fun runningTakeoverUsesTheAcknowledgedCursorAfterMoreThan8192Events() = runTest(dispatcher) {
        exerciseTakeover(terminal = false, longHistory = true)
    }

    @Test
    fun terminalTakeoverUsesTheAcknowledgedCursorAfterMoreThan8192Events() = runTest(dispatcher) {
        exerciseTakeover(terminal = true, longHistory = true)
    }

    @Test
    fun slowAttachKeepsTheComposerBlockedUntilSubmissionAndSnapshotConfirm() = runTest(dispatcher) {
        exerciseTakeover(terminal = false, slowAttach = true)
    }

    @Test
    fun runningReplayGapPreservesProjectionAndReportsMissingOutput() = runTest(dispatcher) {
        exerciseTakeover(terminal = false, longHistory = true, gap = true)
    }

    @Test
    fun terminalReplayGapPreservesProjectionAndEndsTheVisibleTurn() = runTest(dispatcher) {
        exerciseTakeover(terminal = true, longHistory = true, gap = true)
    }

    @Test
    fun completedAnswerSurvivesWhenAnUnrelatedBackgroundTaskKeepsTheSource() = runTest(dispatcher) {
        exerciseTakeover(terminal = false, completedWithBackgroundTask = true)
    }

    @Test
    fun retainedSessionDoesNotAdoptAnotherSessionsCollidingCheckpoint() = runTest(dispatcher) {
        exerciseTakeover(terminal = false, otherSessionCheckpoint = true)
    }

    @Test
    fun replayKeepsMessageBoundariesAndAppliesRetractionsAfterTheSavedCursor() = runTest(dispatcher) {
        exerciseTakeover(terminal = false, identityReplay = true)
    }

    @Test
    fun liveEventsAfterRetainedReplayAreNotRenderedTwice() = runTest(dispatcher) {
        exerciseTakeover(terminal = false, longHistory = true, subsequentLive = true)
    }

    @Test
    fun retractionAfterTakeoverUsesTheSavedMessageIdentity() = runTest(dispatcher) {
        exerciseTakeover(terminal = false, identityBeforeDetach = true)
    }

    @Test
    fun overlappingRawCallbacksDoNotAdvancePastAnUnconfirmedProjection() = runTest(dispatcher) {
        exerciseTakeover(terminal = false, interleavedAcks = true, fullyConfirmed = false)
        exerciseTakeover(terminal = false, interleavedAcks = true, fullyConfirmed = true)
    }

    @Test
    fun outOfOrderEnvelopesWaitForTheContinuousFrontier() = runTest(dispatcher) {
        exerciseTakeover(terminal = false, interleavedAcks = true, reverseAcks = true, fullyConfirmed = false)
        exerciseTakeover(terminal = false, interleavedAcks = true, reverseAcks = true, fullyConfirmed = true)
    }

    @Test
    fun reattachedLiveStreamAlsoWaitsForAllOverlappingEnvelopes() = runTest(dispatcher) {
        for (reverse in listOf(false, true)) {
            for (confirmed in listOf(false, true)) {
                exerciseTakeover(terminal = false, reattachInterleaving = true,
                    reverseAcks = reverse, fullyConfirmed = confirmed)
            }
        }
    }

    @Test
    fun liveRawCrossingTheReplayBoundaryIsRenderedExactlyOnce() = runTest(dispatcher) {
        // Raw 2 precedes replay 1, with envelope 2 either side of replay 1;
        // the third case delivers raw 2 after replay 1.
        for (order in 1..3) exerciseTakeover(terminal = false, boundaryInterleaving = order)
    }

    @Test
    fun newUiOwnerReceivesAnAttachSnapshotAtTheUnchangedCursor() = runTest(dispatcher) {
        exerciseTakeover(terminal = false, unchangedCursorTakeover = true)
    }

    @Test
    fun pendingUiReceivesResumeDispositionBeforeSourcePublication() = runTest(dispatcher) {
        val sourceScope = CoroutineScope(SupervisorJob() + dispatcher)
        val relay = LosslessEventRelay<ClientEvent>(sourceScope)
        val workflows = LosslessEventRelay<WorkflowProgressUpdate>(sourceScope)
        val commands = mutableListOf<ClientCommand>()
        val handle = object : MobileEngineHandle(com.lingxi.code.bindings.runtime.NoHandle) {
            override suspend fun submit(command: ClientCommand) { commands += command }
        }
        val media = AndroidNativeMediaHost(com.lingxi.code.bindings.android.NoHandle)
        val permissions = MutableStateFlow<PermissionPromptState?>(null)
        val durable = DurableConversationTurnClientStore(FakeSharedPreferences(), "global#code", initialSessionId = "session-a")
        durable.begin("session-a", 42L)
        val spec = ConversationRecoverySpec(null, null, SessionMode.Code, LinuxRuntimeMode.MobileLinux)
        val source = EngineConversationSource(
            handle = handle, mediaHost = media, mediaContextBindings = MediaAudioSessionContextBindings<MobileEngineHandle> { _, _ -> },
            events = relay.events, eventRelay = relay, workflowEvents = workflows.events, workflowRelay = workflows,
            eventScope = sourceScope, permissions = permissions, permissionIngress = PermissionIngress(permissions),
            models = MutableStateFlow(EngineModelState()), sessions = MutableStateFlow(EngineSessionState.loading()),
            workspace = MutableStateFlow(com.lingxi.code.project.SessionWorkspaceState("session-a", "/home", "/home", emptyList(), "/home/session-a.jsonl")),
            activeSession = MutableStateFlow(ActivatedSession("session-a", emptyList(), SessionActivationKind.Resumed, SessionMode.Code)),
            mcp = MutableStateFlow(emptyList()), strings = DefaultConversationStrings,
            durableTurns = durable, durableReplayGate = DurableTurnReplayGate(), autoAttachDurableTurns = false,
            resumeOnUiAttach = false, initialRecoverySpec = spec, userHomePath = "/home", userHomeGuestPath = "/home",
        )
        val owner = ConversationHeadlessRecovery.RecoveryOwner(spec)
        owner.claimForUi()
        val consumer = async(start = CoroutineStart.UNDISPATCHED) {
            checkNotNull(owner.pending.await()).attachDurableTurnForUi(0L)
        }
        try {
            owner.complete(source)
            consumer.await()
            assertEquals(listOf(ClientCommand.AttachTurn(42uL, 0uL), ClientCommand.ResumeTurn(42uL)), commands)
        } finally {
            source.close()
        }
    }

    @Test
    fun releasingPendingUiWakesAHeadlessAttemptThatWasSuppressed() = runTest(dispatcher) {
        val sourceScope = CoroutineScope(SupervisorJob() + dispatcher)
        val commands = mutableListOf<ClientCommand>()
        val handle = object : MobileEngineHandle(com.lingxi.code.bindings.runtime.NoHandle) {
            override suspend fun submit(command: ClientCommand) { commands += command }
        }
        val source = headlessSource(sourceScope, handle)
        try {
            source.claimDurableTurnForUi()
            source.startHeadlessRecovery()
            runCurrent()
            assertTrue(commands.isEmpty())
            source.releaseDurableTurnToHeadless()
            source.startHeadlessRecovery()
            runCurrent()
            assertEquals(listOf(ClientCommand.AttachTurn(42uL, 0uL), ClientCommand.ResumeTurn(42uL)), commands)
        } finally {
            source.close()
        }
    }

    @Test
    fun failedHeadlessAttachIsRetriedOnTheNextWake() = runTest(dispatcher) {
        val sourceScope = CoroutineScope(SupervisorJob() + dispatcher)
        val commands = mutableListOf<ClientCommand>()
        var attempts = 0
        val handle = object : MobileEngineHandle(com.lingxi.code.bindings.runtime.NoHandle) {
            override suspend fun submit(command: ClientCommand) {
                commands += command
                if (command is ClientCommand.AttachTurn && attempts++ == 0) error("transient attach failure")
            }
        }
        val source = headlessSource(sourceScope, handle)
        try {
            source.startHeadlessRecovery()
            runCurrent()
            source.startHeadlessRecovery()
            runCurrent()
            assertEquals(listOf(ClientCommand.AttachTurn(42uL, 0uL), ClientCommand.AttachTurn(42uL, 0uL),
                ClientCommand.ResumeTurn(42uL)), commands)
        } finally {
            source.close()
        }
    }

    private fun headlessSource(sourceScope: CoroutineScope, handle: MobileEngineHandle): EngineConversationSource {
        val relay = LosslessEventRelay<ClientEvent>(sourceScope)
        val workflows = LosslessEventRelay<WorkflowProgressUpdate>(sourceScope)
        val media = AndroidNativeMediaHost(com.lingxi.code.bindings.android.NoHandle)
        val permissions = MutableStateFlow<PermissionPromptState?>(null)
        val durable = DurableConversationTurnClientStore(FakeSharedPreferences(), "global#code", initialSessionId = "session-a")
        durable.begin("session-a", 42L)
        return EngineConversationSource(
            handle = handle, mediaHost = media, mediaContextBindings = MediaAudioSessionContextBindings<MobileEngineHandle> { _, _ -> },
            events = relay.events, eventRelay = relay, workflowEvents = workflows.events, workflowRelay = workflows,
            eventScope = sourceScope, permissions = permissions, permissionIngress = PermissionIngress(permissions),
            models = MutableStateFlow(EngineModelState()), sessions = MutableStateFlow(EngineSessionState.loading()),
            workspace = MutableStateFlow(com.lingxi.code.project.SessionWorkspaceState("session-a", "/home", "/home", emptyList(), "/home/session-a.jsonl")),
            activeSession = MutableStateFlow(ActivatedSession("session-a", emptyList(), SessionActivationKind.Resumed, SessionMode.Code)),
            mcp = MutableStateFlow(emptyList()), strings = DefaultConversationStrings,
            durableTurns = durable, durableReplayGate = DurableTurnReplayGate(), autoAttachDurableTurns = false,
            resumeOnUiAttach = false,
            initialRecoverySpec = ConversationRecoverySpec(null, null, SessionMode.Code, LinuxRuntimeMode.MobileLinux),
            userHomePath = "/home", userHomeGuestPath = "/home",
        )
    }

    private suspend fun TestScope.exerciseTakeover(
        terminal: Boolean,
        longHistory: Boolean = false,
        slowAttach: Boolean = false,
        gap: Boolean = false,
        completedWithBackgroundTask: Boolean = false,
        otherSessionCheckpoint: Boolean = false,
        identityReplay: Boolean = false,
        subsequentLive: Boolean = false,
        identityBeforeDetach: Boolean = false,
        interleavedAcks: Boolean = false,
        reverseAcks: Boolean = false,
        fullyConfirmed: Boolean = false,
        reattachInterleaving: Boolean = false,
        boundaryInterleaving: Int = 0,
        unchangedCursorTakeover: Boolean = false,
    ) {
        val sourceScope = CoroutineScope(SupervisorJob() + dispatcher)
        val relay = LosslessEventRelay<ClientEvent>(sourceScope)
        val workflows = LosslessEventRelay<WorkflowProgressUpdate>(sourceScope)
        val gate = DurableTurnReplayGate()
        val commands = mutableListOf<ClientCommand>()
        val releaseAttach = CompletableDeferred<Unit>()
        val handle = object : MobileEngineHandle(com.lingxi.code.bindings.runtime.NoHandle) {
            override suspend fun submit(command: ClientCommand) {
                commands += command
                if (command is ClientCommand.AttachTurn) {
                    val secondTakeover = reattachInterleaving && commands.count { it is ClientCommand.AttachTurn } > 1
                    val cursor = command.afterSequence ?: 0u
                    val first = if (gap) cursor + 2u else if (longHistory) 2u else 0u
                    val state = ClientEvent.TurnRecoveryState(TurnRecoverySnapshotDto(
                        sessionId = "session-a", turnId = command.turnId,
                        state = if (terminal) TurnRecoveryStateDto.COMPLETED else TurnRecoveryStateDto.RUNNING,
                        firstSequence = first, lastSequence = if (unchangedCursorTakeover) 1u else if (secondTakeover) 3u else if (interleavedAcks) 2u else cursor + when {
                            identityReplay -> 4u
                            identityBeforeDetach -> 2u
                            terminal || gap -> 2u
                            else -> 1u
                        }, safeToResume = false,
                        reason = if (gap) "replay_history_truncated" else null,
                    ))
                    if (gate.shouldForward(state)) relay.offer(state)
                    if (boundaryInterleaving != 0) {
                        val replay = ClientEvent.TurnEventReplay("session-a", command.turnId, 1u,
                            """{"type":"text_delta","text":"third partial"}""")
                        val live = ClientEvent.TurnEventReplay("session-a", command.turnId, 2u,
                            """{"type":"text_delta","text":" live"}""")
                        if (boundaryInterleaving != 3) relay.offer(ClientEvent.TextDelta(" live".toClientTextDto()))
                        if (boundaryInterleaving == 2 && gate.shouldForward(live)) relay.offer(live)
                        if (gate.shouldForward(replay)) relay.offer(replay)
                        if (boundaryInterleaving == 3) relay.offer(ClientEvent.TextDelta(" live".toClientTextDto()))
                        if (boundaryInterleaving != 2 && gate.shouldForward(live)) relay.offer(live)
                    } else if (unchangedCursorTakeover) {
                        // Both owners are already at the snapshot frontier.
                    } else if (secondTakeover) {
                        listOf("X", "Y").forEachIndexed { index, value ->
                            val sequence = index.toULong() + 2u
                            if (sequence > cursor) {
                                val replay = ClientEvent.TurnEventReplay("session-a", command.turnId, sequence,
                                    """{"type":"text_delta","text":"$value"}""")
                                if (gate.shouldForward(replay)) relay.offer(replay)
                            }
                        }
                    } else if (interleavedAcks) {
                        listOf("A", "B").forEachIndexed { index, value ->
                            val sequence = index.toULong() + 1u
                            if (sequence > cursor) {
                                val replay = ClientEvent.TurnEventReplay("session-a", command.turnId, sequence,
                                    """{"type":"text_delta","text":"$value"}""")
                                if (gate.shouldForward(replay)) relay.offer(replay)
                            }
                        }
                    } else if (identityBeforeDetach) {
                        listOf(
                            """{"type":"message_retracted","message_id":"attempt-a"}""",
                            """{"type":"text_delta","text":"b"}""",
                        ).forEachIndexed { index, json ->
                            val replay = ClientEvent.TurnEventReplay("session-a", command.turnId, cursor + index.toULong() + 1u, json)
                            if (gate.shouldForward(replay)) relay.offer(replay)
                        }
                    } else if (identityReplay) {
                        listOf(
                            3 to """{"type":"text_delta","text":"b"}""",
                            1 to """{"type":"message_identity","message_id":"attempt-a"}""",
                            2 to """{"type":"message_complete","message":{"role":"assistant","blocks":[{"type":"text","text":"a"}]}}""",
                            4 to """{"type":"message_retracted","message_id":"attempt-a"}""",
                        ).forEach { (offset, json) ->
                            val replay = ClientEvent.TurnEventReplay("session-a", command.turnId, cursor + offset.toULong(), json)
                            if (gate.shouldForward(replay)) relay.offer(replay)
                        }
                    } else if (!gap) {
                        val replay = ClientEvent.TurnEventReplay("session-a", command.turnId, cursor + 1u,
                            if (longHistory) """{"type":"text_delta","text":"z"}""" else """{"type":"text_delta","text":"third partial"}""")
                        if (gate.shouldForward(replay)) relay.offer(replay)
                    }
                    if (terminal && !gap) {
                        val end = ClientEvent.TurnEventReplay("session-a", command.turnId, cursor + 2u,
                            """{"type":"turn_ended","outcome":{"type":"end_turn"}}""")
                        if (gate.shouldForward(end)) relay.offer(end)
                    }
                    if (slowAttach) releaseAttach.await()
                }
            }
        }
        val media = AndroidNativeMediaHost(com.lingxi.code.bindings.android.NoHandle)
        val permissions = MutableStateFlow<PermissionPromptState?>(null)
        val durable = DurableConversationTurnClientStore(FakeSharedPreferences(), "global#code")
        val source = EngineConversationSource(
            handle = handle,
            mediaHost = media,
            mediaContextBindings = MediaAudioSessionContextBindings<MobileEngineHandle> { _, _ -> },
            events = relay.events,
            eventRelay = relay,
            workflowEvents = workflows.events,
            workflowRelay = workflows,
            eventScope = sourceScope,
            permissions = permissions,
            permissionIngress = PermissionIngress(permissions),
            models = MutableStateFlow(EngineModelState()),
            sessions = MutableStateFlow(EngineSessionState.loading()),
            workspace = MutableStateFlow(com.lingxi.code.project.SessionWorkspaceState("session-a", "/home", "/home", emptyList(), "/home/session-a.jsonl")),
            activeSession = MutableStateFlow(ActivatedSession("session-a", emptyList(), SessionActivationKind.Started, SessionMode.Code)),
            mcp = MutableStateFlow(emptyList()),
            strings = DefaultConversationStrings,
            durableTurns = durable,
            durableReplayGate = gate,
            autoAttachDurableTurns = false,
            resumeOnUiAttach = false,
            initialRecoverySpec = ConversationRecoverySpec(null, null, SessionMode.Code, LinuxRuntimeMode.MobileLinux),
            userHomePath = "/home",
            userHomeGuestPath = "/home",
        )
        val retention = object : ConversationBackgroundExecution {
            override fun setTurnActive(active: Boolean) = Unit
            override fun retainAfterUiDestroyed(source: ConversationSource, snapshot: ConversationBackgroundSnapshot): Boolean {
                (source as? EngineConversationSource)?.releaseDurableTurnToHeadless()
                return true
            }
        }
        val old = ChatViewModel(source, backgroundExecution = retention)
        val store = ViewModelStore().apply { put("chat", old) }
        val cost = CostDto(0.0, 0u, 0u, 0u, 0u, "")
        fun finish(answer: String) {
            relay.offer(ClientEvent.TextDelta(answer.toClientTextDto()))
            relay.offer(ClientEvent.TurnEnded(TurnOutcomeDto.END_TURN, null, cost))
        }

        old.send("first question")
        finish("first answer")
        runCurrent()
        old.send("second question")
        finish("second answer")
        runCurrent()
        old.send("third question")
        val originalTurnId = durable.load()?.turnId ?: error("expected active checkpoint")
        if (completedWithBackgroundTask) {
            finish("third answer")
            runCurrent()
            old.reduceClientEvent(ClientEvent.TurnRecoveryState(TurnRecoverySnapshotDto(
                "session-a", originalTurnId.toULong(), TurnRecoveryStateDto.COMPLETED, 0u, 0u, false, null,
            )))
            old.reduceClientEvent(ClientEvent.TaskStatusChanged("background", TaskStatusDto.RUNNING, "session-a", null))
            durable.clear()
        } else if (unchangedCursorTakeover) {
            relay.offer(ClientEvent.TextDelta("third partial".toClientTextDto()))
            relay.offer(ClientEvent.TurnEventReplay("session-a", originalTurnId.toULong(), 1u,
                """{"type":"text_delta","text":"third partial"}"""))
        } else if (interleavedAcks) {
            relay.offer(ClientEvent.TextDelta("A".toClientTextDto()))
            relay.offer(ClientEvent.TextDelta("B".toClientTextDto()))
            val acknowledgments = listOf(
                ClientEvent.TurnEventReplay("session-a", originalTurnId.toULong(), 1u, """{"type":"text_delta","text":"A"}"""),
                ClientEvent.TurnEventReplay("session-a", originalTurnId.toULong(), 2u, """{"type":"text_delta","text":"B"}"""),
            ).let { if (reverseAcks) it.reversed() else it }
            relay.offer(acknowledgments.first())
            runCurrent()
            assertEquals(0L, old.durableReplayCursorForTesting())
            if (fullyConfirmed) {
                relay.offer(acknowledgments.last())
                runCurrent()
                assertEquals(2L, old.durableReplayCursorForTesting())
            }
        } else if (longHistory) {
            for (sequence in 1..8193) {
                relay.offer(ClientEvent.TextDelta("a".toClientTextDto()))
                relay.offer(ClientEvent.TurnEventReplay("session-a", originalTurnId.toULong(), sequence.toULong(),
                    """{"type":"text_delta","text":"a"}"""))
            }
        } else if (identityReplay) {
            relay.offer(ClientEvent.TextDelta("a".toClientTextDto()))
            relay.offer(ClientEvent.TurnEventReplay("session-a", originalTurnId.toULong(), 1u,
                """{"type":"text_delta","text":"a"}"""))
        } else if (identityBeforeDetach) {
            relay.offer(ClientEvent.TextDelta("a".toClientTextDto()))
            relay.offer(ClientEvent.TurnEventReplay("session-a", originalTurnId.toULong(), 1u, """{"type":"text_delta","text":"a"}"""))
            relay.offer(ClientEvent.MessageIdentity("attempt-a"))
            relay.offer(ClientEvent.TurnEventReplay("session-a", originalTurnId.toULong(), 2u, """{"type":"message_identity","message_id":"attempt-a"}"""))
            relay.offer(ClientEvent.MessageComplete(null, MessageDto(null, "assistant", listOf(MessageBlockDto.Text("a".toClientTextDto())))))
            relay.offer(ClientEvent.TurnEventReplay("session-a", originalTurnId.toULong(), 3u,
                """{"type":"message_complete","message":{"role":"assistant","blocks":[{"type":"text","text":"a"}]}}"""))
        } else {
            relay.offer(ClientEvent.TextDelta("third partial".toClientTextDto()))
        }
        runCurrent()
        assertEquals(!completedWithBackgroundTask, old.state.value.streaming)
        store.clear()
        if (terminal) durable.clear()
        if (otherSessionCheckpoint) durable.begin("session-b", originalTurnId)

        val recreated = ChatViewModel(ConnectingConversationSource(DefaultConversationStrings), SavedStateHandle(),
            sourceGeneration = -1, backgroundExecution = retention)
        recreated.prepareInitialSessionRestore(SessionRef("session-a", ""), ConversationScope.Global)
        recreated.ensureSource(0) { source }
        runCurrent()
        if (slowAttach) {
            assertFalse(recreated.state.value.sessionReady)
            recreated.send("must remain editable")
            assertEquals(3, commands.filterIsInstance<ClientCommand.SendPrompt>().size)
            assertEquals(originalTurnId, durable.load()?.turnId)
            releaseAttach.complete(Unit)
            runCurrent()
        }
        val expected = listOf("first question", "first answer", "second question", "second answer", "third question")
        val output = when {
            completedWithBackgroundTask -> "third answer"
            interleavedAcks -> "AB"
            boundaryInterleaving != 0 -> "third partial live"
            identityReplay || identityBeforeDetach -> "b"
            longHistory -> "a".repeat(8193) + if (gap) "" else "z"
            else -> "third partial"
        }
        assertEquals(if (terminal || completedWithBackgroundTask) expected + output else expected, recreated.state.value.messages.map { it.text })
        assertEquals(if (terminal || completedWithBackgroundTask) null else output, recreated.state.value.streamingMessage?.text)
        assertEquals(!terminal && !completedWithBackgroundTask, recreated.state.value.streaming)
        assertEquals(gap, recreated.state.value.error != null)
        if (completedWithBackgroundTask) {
            assertTrue(commands.none { it is ClientCommand.AttachTurn })
        } else {
            val attach = commands.filterIsInstance<ClientCommand.AttachTurn>().single()
            assertEquals(originalTurnId.toULong(), attach.turnId)
            assertEquals(when { unchangedCursorTakeover -> 1uL; interleavedAcks && fullyConfirmed -> 2uL; longHistory -> 8193uL; identityReplay -> 1uL; identityBeforeDetach -> 3uL; else -> 0uL }, attach.afterSequence)
        }
        if (subsequentLive) {
            relay.offer(ClientEvent.TextDelta(" live".toClientTextDto()))
            relay.offer(ClientEvent.TurnEventReplay("session-a", originalTurnId.toULong(), 8195u,
                """{"type":"text_delta","text":" live"}"""))
            runCurrent()
            assertEquals(output + " live", recreated.state.value.streamingMessage?.text)
        }
        if (reattachInterleaving) {
            val recreatedStore = ViewModelStore().apply { put("chat", recreated) }
            relay.offer(ClientEvent.TextDelta("X".toClientTextDto()))
            relay.offer(ClientEvent.TextDelta("Y".toClientTextDto()))
            val acknowledgments = listOf(
                ClientEvent.TurnEventReplay("session-a", originalTurnId.toULong(), 2u, """{"type":"text_delta","text":"X"}"""),
                ClientEvent.TurnEventReplay("session-a", originalTurnId.toULong(), 3u, """{"type":"text_delta","text":"Y"}"""),
            ).let { if (reverseAcks) it.reversed() else it }
            relay.offer(acknowledgments.first())
            runCurrent()
            assertEquals(1L, recreated.durableReplayCursorForTesting())
            if (fullyConfirmed) {
                relay.offer(acknowledgments.last())
                runCurrent()
                assertEquals(3L, recreated.durableReplayCursorForTesting())
            }
            recreatedStore.clear()
            val again = ChatViewModel(ConnectingConversationSource(DefaultConversationStrings), SavedStateHandle(), sourceGeneration = -1)
            again.prepareInitialSessionRestore(SessionRef("session-a", ""), ConversationScope.Global)
            again.ensureSource(0) { source }
            runCurrent()
            val secondAttach = commands.filterIsInstance<ClientCommand.AttachTurn>().last()
            assertEquals(if (fullyConfirmed) 3uL else 1uL, secondAttach.afterSequence)
            assertEquals("third partialXY", again.state.value.streamingMessage?.text)
        }
        if (unchangedCursorTakeover) {
            ViewModelStore().apply { put("chat", recreated) }.clear()
            val again = ChatViewModel(ConnectingConversationSource(DefaultConversationStrings), SavedStateHandle(), sourceGeneration = -1)
            again.prepareInitialSessionRestore(SessionRef("session-a", ""), ConversationScope.Global)
            again.ensureSource(0) { source }
            runCurrent()
            val attaches = commands.filterIsInstance<ClientCommand.AttachTurn>()
            assertEquals(listOf(1uL, 1uL), attaches.map { it.afterSequence })
            assertTrue(again.state.value.sessionReady)
            assertEquals("third partial", again.state.value.streamingMessage?.text)
        }
        assertFalse(commands.any { it is ClientCommand.NewSession || it is ClientCommand.ResumeSession ||
            it is ClientCommand.Cancel || it is ClientCommand.ResumeTurn })
        if (otherSessionCheckpoint) assertEquals("session-b", durable.load()?.sessionId)
        if (terminal) {
            val turnId = commands.filterIsInstance<ClientCommand.AttachTurn>().single().turnId
            relay.offer(ClientEvent.TurnRecoveryState(TurnRecoverySnapshotDto(
                "session-a", turnId, TurnRecoveryStateDto.COMPLETED, 0u, 2u, false, null,
            )))
            runCurrent()
            assertEquals(expected + output, recreated.state.value.messages.map { it.text })
            assertFalse(recreated.state.value.streaming)
        }
        source.close()
    }
}
