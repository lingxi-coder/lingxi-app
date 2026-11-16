package com.lingxi.code.conversation

import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.SavedStateHandle
import com.lingxi.code.bindings.android.AndroidNativeMediaHost
import com.lingxi.code.bindings.client.*
import com.lingxi.code.bindings.runtime.MobileEngineHandle
import com.lingxi.code.bindings.runtime.MobileTurnAdmissionDto
import com.lingxi.code.model.*
import com.lingxi.code.project.SessionWorkspaceState
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.test.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import java.util.concurrent.atomic.AtomicReference

/** Real source/relay/ViewModel wiring with a typed native-handle test double. */
@OptIn(ExperimentalCoroutinesApi::class)
class QueuedConversationOwnershipTest {
    private val dispatcher = UnconfinedTestDispatcher()
    @Before fun setMain() = Dispatchers.setMain(dispatcher)
    @After fun resetMain() = Dispatchers.resetMain()

    private val cost = CostDto(0.0, 0u, 0u, 0u, 0u, "", emptyList())
    private fun dto(role: String, text: String, uuid: String? = null) =
        MessageDto(uuid, role, listOf(MessageBlockDto.Text(text.toClientTextDto())))
    private fun recovery(id: Long, state: TurnRecoveryStateDto = TurnRecoveryStateDto.RUNNING, last: ULong = 0u) =
        TurnRecoverySnapshotDto("session-a", id.toULong(), state, 0u, last, false, null)
    private fun fact(id: Long, active: Boolean = false, queued: Boolean = false, settled: Boolean = false,
                     state: TurnRecoveryStateDto = TurnRecoveryStateDto.RUNNING, owner: Long? = id) =
        MobileTurnAdmissionDto(id.toULong(), true, queued, settled, active, owner?.let { recovery(it, state) })
    private fun queuedPrompt(id: Long) = ClientCommand.SendPrompt("B".toClientTextDto(), null, emptyList(), id.toULong(), null)

    private inner class Harness(
        val prefs: FakeSharedPreferences = FakeSharedPreferences(),
        initial: List<Message> = emptyList(),
        resumed: Boolean = false,
    ) {
        val scope = CoroutineScope(SupervisorJob() + dispatcher)
        val store = DurableConversationTurnClientStore(prefs, "global#code", initialSessionId = "session-a")
        val facts = linkedMapOf<Long, MobileTurnAdmissionDto>()
        val activation = MutableStateFlow<ActivatedSession?>(ActivatedSession("session-a", initial,
            if (resumed) SessionActivationKind.Resumed else SessionActivationKind.Started, SessionMode.Code))
        val commands = mutableListOf<ClientCommand>()
        val capture = AtomicReference<HeadlessSnapshotCapture?>(null)
        var live: Long? = null
        var autoStart = true
        var sendFailure: Exception? = null
        var queryFailure: Exception? = null
        var load: suspend () -> Unit = { emitHistory() }
        var history = emptyList<MessageDto>()
        val retained = linkedMapOf<Long, List<String>>()
        var bootstrap = emptyList<ClientEvent>()
        var queryCount = 0
        var beforeQuery: suspend (Int) -> Unit = {}
        val handle = object : MobileEngineHandle(com.lingxi.code.bindings.runtime.NoHandle) {
            override suspend fun queryTurnAdmissions(expectedSessionId: String, turnIds: List<ULong>): List<MobileTurnAdmissionDto> {
                assertEquals(activation.value?.sessionId, expectedSessionId)
                beforeQuery(++queryCount)
                queryFailure?.let { throw it }
                return turnIds.map { facts[it.toLong()] ?: MobileTurnAdmissionDto(it, false, false, false, false, null) }
            }
            override suspend fun submit(command: ClientCommand) {
                commands += command
                when (command) {
                    is ClientCommand.SendPrompt -> {
                        sendFailure?.let { throw it }
                        val id = checkNotNull(command.turnId).toLong()
                        if (live == null && autoStart) start(id)
                        else facts[id] = fact(id, queued = true, owner = null)
                    }
                    is ClientCommand.AttachTurn -> {
                        val id = command.turnId.toLong()
                        val rows = retained[id].orEmpty()
                        val snapshot = facts[id]?.recovery ?: recovery(id)
                        emit(ClientEvent.TurnRecoveryState(snapshot.copy(lastSequence = rows.size.toULong())))
                        rows.forEachIndexed { index, json ->
                            if ((index + 1).toULong() > (command.afterSequence ?: 0u)) {
                                emit(ClientEvent.TurnEventReplay("session-a", command.turnId, (index + 1).toULong(), json))
                            }
                        }
                    }
                    is ClientCommand.ResumeTurn -> {
                        val id = command.turnId.toLong()
                        if (facts[id]?.settled != true) start(id)
                    }
                    is ClientCommand.Cancel -> command.turnId?.toLong()?.takeIf { facts[it]?.settled != true }?.let { end(it, TurnRecoveryStateDto.CANCELLED) }
                    is ClientCommand.LoadSessionAgentTranscript -> load()
                    is ClientCommand.NewSession -> {
                        store.selectSession("session-b")
                        activation.value = ActivatedSession("session-b", emptyList(), SessionActivationKind.Started, SessionMode.Code)
                    }
                    else -> Unit
                }
            }
        }
        val turns = QueuedConversationTurns(store, { activation.value?.sessionId }) { session, ids -> queryQueuedAdmissions(handle, session, ids) }
        val relay = LosslessEventRelay<ClientEvent>(scope, beforeEmit = { event ->
            try { turns.beforeEvent(event) } catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { false }
        })
        private val workflow = LosslessEventRelay<WorkflowProgressUpdate>(scope)
        private val permissions = MutableStateFlow<PermissionPromptState?>(null)
        val spec = ConversationRecoverySpec(null, null, SessionMode.Code, LinuxRuntimeMode.MobileLinux,
            workspaceKey = "global", transcriptPath = "/home/session-a.jsonl")
        val source = EngineConversationSource(
            handle = handle, mediaHost = AndroidNativeMediaHost(com.lingxi.code.bindings.android.NoHandle),
            mediaContextBindings = MediaAudioSessionContextBindings<MobileEngineHandle> { _, _ -> },
            events = relay.events.onSubscription {
                val initial = bootstrap; bootstrap = emptyList()
                initial.forEach { this@Harness.emit(it) }
            }, eventRelay = relay, workflowEvents = workflow.events, workflowRelay = workflow,
            eventScope = scope, permissions = permissions, permissionIngress = PermissionIngress(permissions),
            models = MutableStateFlow(EngineModelState()), sessions = MutableStateFlow(EngineSessionState.loading()),
            workspace = MutableStateFlow(SessionWorkspaceState("session-a", "/home", "/home", emptyList(), "/home/session-a.jsonl")),
            activeSession = activation,
            mcp = MutableStateFlow(emptyList()), strings = DefaultConversationStrings,
            durableTurns = store, durableReplayGate = DurableTurnReplayGate(), autoAttachDurableTurns = false,
            resumeOnUiAttach = false, initialRecoverySpec = spec, userHomePath = "/home", userHomeGuestPath = "/home",
            queuedTurns = turns, snapshotCapture = capture,
        )
        private val viewModels = ViewModelStore()
        fun vm(): ChatViewModel = ChatViewModel(source).also { viewModels.put("chat", it) }
        fun emit(event: ClientEvent) {
            capture.get()?.accept(event)
            turns.noteIncoming(event)
            relay.offer(event)
        }
        fun start(id: Long) {
            live = id; facts[id] = fact(id, active = true)
            emit(ClientEvent.TurnRecoveryState(recovery(id)))
            emit(ClientEvent.TurnStarted(id.toULong()))
        }
        fun end(id: Long, state: TurnRecoveryStateDto = TurnRecoveryStateDto.COMPLETED) {
            facts[id] = fact(id, settled = true, state = state)
            live = null
            emit(ClientEvent.TurnEnded(TurnOutcomeDto.END_TURN, null, cost))
            emit(ClientEvent.TurnRecoveryState(recovery(id, state)))
        }
        fun emitHistory() {
            emit(ClientEvent.SessionAgentTranscript("session-a", "main", history.mapIndexed { index, message ->
                SessionAgentMessageRowDto(index.toULong(), checkNotNull(message.transcriptUuid), message, null)
            }, history.size.toULong(), 1uL))
        }
        fun close() { viewModels.clear(); source.close() }
    }

    @Test fun busyIndependentBHasCorrectUiOrderDurableStopGuardAndTargetAfterATerminal() = runTest(dispatcher) {
        for (cancelA in listOf(false, true)) {
            val h = Harness()
            try {
                val vm = h.vm(); runCurrent()
                vm.send("A"); runCurrent()
                val a = h.commands.filterIsInstance<ClientCommand.SendPrompt>().single().turnId!!.toLong()
                vm.send("B"); runCurrent()
                val b = h.commands.filterIsInstance<ClientCommand.SendPrompt>().last().turnId!!.toLong()
                assertEquals(a, h.store.load()?.turnId)
                assertEquals(listOf(b), h.store.state("session-a").queuedTurnIds)
                h.emit(ClientEvent.TextDelta("A answer".toClientTextDto()))
                h.end(a, if (cancelA) TurnRecoveryStateDto.CANCELLED else TurnRecoveryStateDto.COMPLETED)
                h.start(b); h.emit(ClientEvent.TextDelta("B answer".toClientTextDto())); runCurrent()
                assertEquals(b, h.store.load()?.turnId)
                assertTrue(h.source.ownsDurableTurn("session-a", b, h.spec.runtimeGeneration))
                assertFalse(h.source.ownsDurableTurn("session-a", a, h.spec.runtimeGeneration))
                assertEquals(listOf("A", "B", "A answer"), vm.state.value.messages.map { it.text })
                assertEquals("B answer", vm.state.value.streamingMessage?.text)
                vm.cancel(); runCurrent()
                assertEquals(b.toULong(), h.commands.filterIsInstance<ClientCommand.Cancel>().last().turnId)
                assertFalse(vm.state.value.streaming)
                vm.send("C"); runCurrent()
                assertEquals(3, h.commands.filterIsInstance<ClientCommand.SendPrompt>().size)
            } finally { h.close() }
        }
    }

    @Test fun nativeMidTurnConsumptionLeavesAAsOwnerAndNeverExecutesBAgain() = runTest(dispatcher) {
        val h = Harness()
        try {
            val vm = h.vm(); runCurrent(); vm.send("A"); runCurrent(); vm.send("B"); runCurrent()
            val sends = h.commands.filterIsInstance<ClientCommand.SendPrompt>()
            val a = sends.first().turnId!!.toLong(); val b = sends.last().turnId!!.toLong()
            h.facts[b] = fact(b, settled = true, owner = null)
            h.emit(ClientEvent.UserTranscriptRowIdentity("client-turn:$b", "b-user")); runCurrent()
            assertEquals(a, h.store.load()?.turnId)
            assertTrue(h.store.state("session-a").queuedTurnIds.isEmpty())
            h.emit(ClientEvent.TextDelta("A handles both".toClientTextDto())); h.end(a); runCurrent()
            assertEquals(2, h.commands.filterIsInstance<ClientCommand.SendPrompt>().size)
            assertEquals(listOf("A", "B", "A handles both"), vm.state.value.messages.map { it.text })
        } finally { h.close() }
    }

    @Test fun initialPendingAdmissionCannotAppendBUntilAMatchingStarted() = runTest(dispatcher) {
        val h = Harness()
        try {
            h.autoStart = false
            val vm = h.vm(); runCurrent(); vm.send("A"); runCurrent()
            vm.send("B"); runCurrent()
            assertEquals(1, h.commands.filterIsInstance<ClientCommand.SendPrompt>().size)
            val a = h.commands.filterIsInstance<ClientCommand.SendPrompt>().single().turnId!!.toLong()
            h.start(a); runCurrent(); vm.send("B"); runCurrent()
            assertEquals(2, h.commands.filterIsInstance<ClientCommand.SendPrompt>().size)
            assertEquals(a, h.store.load()?.turnId)
        } finally { h.close() }
    }

    @Test fun definiteAndUncertainBusyFailureRestoreOnlyTheNativeProvenMissingInput() = runTest(dispatcher) {
        for (definite in listOf(false, true)) {
            val h = Harness()
            try {
                val vm = h.vm(); runCurrent(); vm.send("A"); runCurrent()
                val a = h.store.load()!!.turnId
                var restored = 0
                h.sendFailure = if (definite) ClientException.Rejected("turn_cancelled_before_admission") else IllegalStateException("receipt lost")
                vm.send("B", onNotAdmitted = { restored++ }); runCurrent()
                assertEquals(1, restored)
                assertEquals(listOf("A"), vm.state.value.messages.map { it.text })
                assertEquals(a, h.store.load()?.turnId)
                assertTrue(h.store.state("session-a").queuedTurnIds.isEmpty())
                assertEquals(2, h.commands.filterIsInstance<ClientCommand.SendPrompt>().size)
            } finally { h.close() }
        }
    }

    @Test fun queuedSuccessorRetargetsOnlyItsExactBackgroundLeasesAndRejectsAStaleStop() {
        val spec = ConversationRecoverySpec(null, null, SessionMode.Code, LinuxRuntimeMode.MobileLinux,
            workspaceKey = "global", transcriptPath = "/guest/a.jsonl")
        val a = ConversationBackgroundSnapshot("session-a", 41, "Working", spec, setOf("child-a"))
        val another = a.copy(recoverySpec = spec.copy(runtimeGeneration = "another-source"))
        val existing = mapOf("ui" to a, spec.ownerKey to a.copy(activeTaskIds = setOf("child-b")), "another" to another)
        val updates = queuedExecutorOwnerSnapshots(spec, "session-a", 42, true, existing)
        assertEquals(setOf("ui", spec.ownerKey), updates.keys)
        assertTrue(updates.values.all { it.turnId == 42L && it.executorActive })
        assertEquals(setOf("child-a"), updates.getValue("ui").activeTaskIds)
        assertEquals(setOf("child-b"), updates.getValue(spec.ownerKey).activeTaskIds)
        assertTrue(runCatching { reconcileConversationAction(a, updates) }.isFailure)
    }

    @Test fun coldAndWarmClaimedRecoveryAttachAndResumeOnlyTheNativeProvenB() = runTest(dispatcher) {
        for (cold in listOf(false, true)) {
            val h = Harness(); h.store.begin("session-a", 41)
            h.facts[41] = fact(41, active = true); h.live = 41
            h.source.submitQueuedPrompt(queuedPrompt(42), 41, { true }, {})
            h.facts[41] = fact(41, settled = true)
            h.facts[42] = fact(42); h.live = null
            if (!cold) {
                h.source.attachDurableTurnForUi(0); runCurrent()
                assertEquals(42L, h.store.load()?.turnId)
                assertEquals(42uL, h.commands.filterIsInstance<ClientCommand.ResumeTurn>().single().turnId)
                assertEquals(1, h.commands.filterIsInstance<ClientCommand.SendPrompt>().size)
                h.close()
            } else {
                val restored = Harness(h.prefs, resumed = true)
                restored.facts.putAll(h.facts)
                try {
                    restored.source.attachDurableTurnForUi(0); runCurrent()
                    assertEquals(42L, restored.store.load()?.turnId)
                    assertEquals(42uL, restored.commands.filterIsInstance<ClientCommand.ResumeTurn>().single().turnId)
                    assertTrue(restored.commands.none { it is ClientCommand.SendPrompt })
                } finally { restored.close(); h.close() }
            }
        }
    }

    @Test fun transientAdmissionFailureAtRunningAndStartedRepairsTerminalHistoryAndAllowsContinue() = runTest(dispatcher) {
        val h = Harness()
        try {
            val vm = h.vm(); runCurrent(); vm.send("A"); runCurrent(); vm.send("B"); runCurrent()
            val sends = h.commands.filterIsInstance<ClientCommand.SendPrompt>()
            val a = sends.first().turnId!!.toLong(); val b = sends.last().turnId!!.toLong()
            h.end(a); runCurrent()
            h.queryFailure = IllegalStateException("temporary admission read")
            h.start(b); h.emit(ClientEvent.TextDelta("B answer".toClientTextDto())); runCurrent()
            h.queryFailure = null
            h.history = listOf(dto("user", "A", "a"), dto("user", "B", "b"), dto("assistant", "B answer", "b-answer"))
            h.end(b); runCurrent()
            assertFalse(h.source.hasQueuedTurnRecovery())
            assertEquals(1, vm.state.value.messages.count { it.transcriptUuid == "b-answer" })
            assertFalse(vm.state.value.streaming)
            vm.send("C"); runCurrent()
            assertEquals(3, h.commands.filterIsInstance<ClientCommand.SendPrompt>().size)
        } finally { h.close() }
    }

    @Test fun headlessFirstUiRefreshesAFinalAndMergesBusyBReplayWithoutBackpressureOrDuplicates() = runTest(dispatcher) {
        val h = Harness(initial = listOf(Message(Role.User, "A", transcriptUuid = "a-user")), resumed = true)
        try {
            h.store.begin("session-a", 41); h.store.enqueue("session-a", 41, 42)
            h.facts[41] = fact(41, settled = true); h.start(42); runCurrent()
            h.history = listOf(dto("user", "A", "a-user"), dto("assistant", "A final", "a-final"),
                dto("user", "B", "b-user"), dto("assistant", "B prefix", "b-prefix"))
            val entered = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
            val tail = "x".repeat(130)
            h.retained[42] = listOf("""{"type":"message_complete","message":{"role":"assistant","transcript_uuid":"b-prefix","blocks":[{"type":"text","text":"B prefix"}]}}""") +
                List(130) { """{"type":"text_delta","text":"x"}""" } +
                """{"type":"message_complete","message":{"role":"assistant","transcript_uuid":"b-final","blocks":[{"type":"text","text":"$tail"}]}}"""
            h.load = { entered.complete(Unit); release.await(); h.emitHistory() }
            h.source.requestHeadlessUiSnapshot()
            val vm = h.vm(); runCurrent(); entered.await()
            repeat(130) { h.emit(ClientEvent.TextDelta("x".toClientTextDto())) }
            h.emit(ClientEvent.MessageComplete(null, dto("assistant", tail, "b-final")))
            release.complete(Unit); runCurrent()
            assertEquals(listOf("A", "A final", "B", "B prefix", tail), vm.state.value.messages.map { it.text })
            assertEquals(1, vm.state.value.messages.count { it.transcriptUuid == "b-prefix" })
            assertEquals(1, vm.state.value.messages.count { it.transcriptUuid == "b-final" })
            assertTrue(vm.state.value.sessionReady)
            assertTrue(vm.state.value.streaming)
            assertTrue(h.commands.none { it is ClientCommand.SendPrompt || it is ClientCommand.ResumeSession })
            assertEquals(1, h.commands.filterIsInstance<ClientCommand.LoadSessionAgentTranscript>().size)
        } finally { h.close() }
    }

    @Test fun cancelledSnapshotCallerCannotConsumeTheNextReadsReplyAndNeedsUiAck() = runTest(dispatcher) {
        val h = Harness(initial = listOf(Message(Role.User, "old", transcriptUuid = "old")), resumed = true)
        try {
            val firstEntered = CompletableDeferred<Unit>(); val firstRelease = CompletableDeferred<Unit>()
            h.load = { firstEntered.complete(Unit); firstRelease.await(); h.history = listOf(dto("user", "first", "first")); h.emitHistory() }
            val first = launch { h.source.snapshotForQueuedHandoff() }
            firstEntered.await(); first.cancelAndJoin()
            h.load = { h.history = listOf(dto("user", "second", "second")); h.emitHistory() }
            val second = async { h.source.snapshotForQueuedHandoff() }
            runCurrent()
            assertEquals(1, h.commands.filterIsInstance<ClientCommand.LoadSessionAgentTranscript>().size)
            firstRelease.complete(Unit); runCurrent()
            // The second source-owned read starts after the first reply even if
            // its SharedFlow publication is still queued behind UI callbacks.
            assertEquals(2, h.commands.filterIsInstance<ClientCommand.LoadSessionAgentTranscript>().size)
            assertEquals(listOf("second"), second.await()!!.map { it.text })
            h.source.requestHeadlessUiSnapshot()
            h.load = { h.history = listOf(dto("user", "fresh", "fresh")); h.emitHistory() }
            val activation = checkNotNull(h.source.activeSessionState.value)
            h.source.prepareHeadlessUiActivation(activation) { _, _ -> }
            // No UI acknowledgement: the next UI must still read a fresh base.
            val fresh = h.source.prepareHeadlessUiActivation(activation) { _, _ -> }
            h.source.confirmHeadlessUiActivation(fresh)
            val count = h.commands.filterIsInstance<ClientCommand.LoadSessionAgentTranscript>().size
            assertSame(activation, h.source.prepareHeadlessUiActivation(activation) { _, _ -> })
            assertEquals(count, h.commands.filterIsInstance<ClientCommand.LoadSessionAgentTranscript>().size)
        } finally { h.close() }
    }

    @Test fun retainedAPartialCannotSuppressHeadlessAFinalOrBecomeSuccessorStreamingText() = runTest(dispatcher) {
        val h = Harness(resumed = true)
        val restoredStore = ViewModelStore()
        try {
            h.store.begin("session-a", 41); h.store.enqueue("session-a", 41, 42)
            h.facts[41] = fact(41, settled = true); h.start(42); runCurrent()
            val partial = ChatState(SessionRef("session-a", ""), listOf(Message(Role.User, "A", transcriptUuid = "a-user")),
                model = EngineModelCatalog.pending, sessionMode = SessionMode.Code,
                streaming = true, streamingMessage = Message(Role.Ai, "A partial"))
            h.source.retainUiTranscript(RetainedConversationTranscript("session-a", 41, partial, 1, 1,
                ConversationTurnOrigin.Ordinary, null, emptyMap(), emptyMap(), emptySet(), emptyMap(), null, emptyMap(), 1))
            h.history = listOf(dto("user", "A", "a-user"), dto("assistant", "A final", "a-final"), dto("user", "B", "b-user"))
            h.retained[42] = listOf("""{"type":"text_delta","text":"B ongoing"}""")
            h.source.requestHeadlessUiSnapshot()
            val vm = ChatViewModel(ConnectingConversationSource(DefaultConversationStrings), SavedStateHandle(), sourceGeneration = -1)
            restoredStore.put("restored", vm)
            vm.prepareInitialSessionRestore(SessionRef("session-a", ""), ConversationScope.Global)
            vm.ensureSource(0) { h.source }; runCurrent()
            assertEquals(listOf("A", "A final", "B"), vm.state.value.messages.map { it.text })
            assertEquals("B ongoing", vm.state.value.streamingMessage?.text)
            assertTrue(vm.state.value.sessionReady)
            assertEquals(1, h.commands.filterIsInstance<ClientCommand.LoadSessionAgentTranscript>().size)
        } finally { restoredStore.clear(); h.close() }
    }

    @Test fun retainedSameOwnerPreservesAcknowledgedCursorAndReplaysOnlyItsNewSuffix() = runTest(dispatcher) {
        val h = Harness(resumed = true); val restoredStore = ViewModelStore()
        try {
            h.store.begin("session-a", 41); h.start(41); runCurrent()
            val partial = ChatState(SessionRef("session-a", ""), listOf(Message(Role.User, "A", transcriptUuid = "a-user")),
                model = EngineModelCatalog.pending, sessionMode = SessionMode.Code,
                streaming = true, streamingMessage = Message(Role.Ai, "A partial"))
            h.source.retainUiTranscript(RetainedConversationTranscript("session-a", 41, partial, 1, 1,
                ConversationTurnOrigin.Ordinary, null, emptyMap(), emptyMap(), emptySet(), emptyMap(), null, emptyMap(), 1))
            h.history = listOf(dto("user", "A", "a-user"))
            h.retained[41] = listOf("""{"type":"text_delta","text":"A partial"}""", """{"type":"text_delta","text":" suffix"}""")
            h.source.requestHeadlessUiSnapshot()
            val vm = ChatViewModel(ConnectingConversationSource(DefaultConversationStrings), SavedStateHandle(), sourceGeneration = -1)
            restoredStore.put("restored", vm)
            vm.prepareInitialSessionRestore(SessionRef("session-a", ""), ConversationScope.Global)
            vm.ensureSource(0) { h.source }; runCurrent()
            assertEquals(listOf("A"), vm.state.value.messages.map { it.text })
            assertEquals("A partial suffix", vm.state.value.streamingMessage?.text)
            assertEquals(1uL, h.commands.filterIsInstance<ClientCommand.AttachTurn>().single().afterSequence)
            assertTrue(vm.state.value.sessionReady)
        } finally { restoredStore.clear(); h.close() }
    }

    @Test fun terminalRepairAwaitCannotApplyOldRowsOrErrorsAfterSameSourceSessionChange() = runTest(dispatcher) {
        for (failRead in listOf(false, true)) {
            val h = Harness()
            try {
                val vm = h.vm(); runCurrent(); vm.send("A"); runCurrent(); vm.send("B"); runCurrent()
                val sends = h.commands.filterIsInstance<ClientCommand.SendPrompt>()
                val a = sends.first().turnId!!.toLong(); val b = sends.last().turnId!!.toLong()
                h.end(a); runCurrent()
                h.queryFailure = IllegalStateException("transient receipt read")
                h.start(b); runCurrent(); h.queryFailure = null
                h.history = listOf(dto("user", "A", "a"), dto("assistant", "old B", "b-answer"))
                val entered = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
                h.load = { entered.complete(Unit); release.await(); if (failRead) error("old session read failure") else h.emitHistory() }
                h.end(b); runCurrent(); entered.await()
                vm.newChat(); runCurrent()
                assertEquals("session-b", h.activation.value?.sessionId)
                release.complete(Unit); runCurrent()
                assertEquals("session-b", vm.state.value.session.id)
                assertTrue(vm.state.value.messages.isEmpty())
                assertFalse(vm.state.value.streaming)
                assertNull(vm.state.value.error)
            } finally { h.close() }
        }
    }

    @Test fun queuedHandoffAwaitCannotAdoptOldExecutorAfterSameSourceSessionChange() = runTest(dispatcher) {
        val h = Harness(initial = listOf(Message(Role.User, "A", transcriptUuid = "a-user")), resumed = true)
        try {
            h.store.begin("session-a", 41); h.store.enqueue("session-a", 41, 42)
            h.start(41); h.facts[42] = fact(42, queued = true, owner = null); runCurrent()
            val entered = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
            var loads = 0
            h.load = {
                if (loads++ == 0) { entered.complete(Unit); release.await(); h.emitHistory() }
                else h.emit(ClientEvent.SessionAgentTranscript("session-b", "main", emptyList(), 0u, 1u))
            }
            h.source.requestHeadlessUiSnapshot()
            val vm = h.vm(); runCurrent(); entered.await()
            h.end(41); h.start(42); h.emit(ClientEvent.TextDelta("old B".toClientTextDto())); runCurrent()
            vm.newChat(); runCurrent(); release.complete(Unit); runCurrent()
            assertEquals("session-b", vm.state.value.session.id)
            assertTrue(vm.state.value.messages.isEmpty())
            assertFalse(vm.state.value.streaming)
            assertNull(vm.state.value.error)
            assertNull(h.store.load())
        } finally { h.close() }
    }

    @Test fun finishingTheLastBackgroundTaskCannotReleaseQueuedOrRunningBUntilItsTerminal() = runTest(dispatcher) {
        val h = Harness()
        try {
            h.store.begin("session-a", 41); h.store.enqueue("session-a", 41, 42)
            h.facts[42] = fact(42, queued = true, owner = null)
            h.end(41); runCurrent()
            assertFalse(canSettleConversationAfterTaskUpdate(emptySet(), 41, 2, h.source))
            h.start(42); runCurrent()
            assertFalse(canSettleConversationAfterTaskUpdate(emptySet(), 42, 1, h.source))
            h.end(42); runCurrent()
            assertTrue(canSettleConversationAfterTaskUpdate(emptySet(), 42, 2, h.source))
            assertFalse(canSettleConversationAfterTaskUpdate(setOf("another-child"), 42, 2, h.source))
        } finally { h.close() }
    }

    @Test fun bufferedRawAndItsLaterEnvelopeStayBehindTheInitialHeadlessOwnerDecision() = runTest(dispatcher) {
        val h = Harness(resumed = true)
        val queryEntered = CompletableDeferred<Unit>(); val queryRelease = CompletableDeferred<Unit>()
        val loadEntered = CompletableDeferred<Unit>(); val loadRelease = CompletableDeferred<Unit>()
        try {
            h.store.begin("session-a", 41); h.store.enqueue("session-a", 41, 42)
            h.facts[41] = fact(41, settled = true); h.facts[42] = fact(42, active = true); h.live = 42
            h.retained[42] = listOf("""{"type":"turn_started","turn_id":42}""", """{"type":"text_delta","text":"x"}""")
            h.bootstrap = listOf(ClientEvent.TurnRecoveryState(recovery(42)), ClientEvent.TurnStarted(42uL),
                ClientEvent.TurnEventReplay("session-a", 42uL, 1uL, h.retained.getValue(42)[0]), ClientEvent.TextDelta("x".toClientTextDto()))
            h.beforeQuery = { count -> if (count == 2) { queryEntered.complete(Unit); queryRelease.await() } }
            h.load = { loadEntered.complete(Unit); loadRelease.await(); h.emitHistory() }
            h.source.requestHeadlessUiSnapshot()
            val vm = h.vm(); runCurrent(); queryEntered.await()
            assertNull(vm.state.value.streamingMessage)
            queryRelease.complete(Unit); runCurrent(); loadEntered.await()
            h.emit(ClientEvent.TurnEventReplay("session-a", 42uL, 2uL, h.retained.getValue(42)[1])); runCurrent()
            assertNull(vm.state.value.streamingMessage)
            loadRelease.complete(Unit); runCurrent()
            assertEquals("x", vm.state.value.streamingMessage?.text)
            assertTrue(vm.state.value.sessionReady)
            assertEquals(2L, vm.durableReplayCursorForTesting())
        } finally { queryRelease.complete(Unit); loadRelease.complete(Unit); runCurrent(); h.close() }
    }
}
