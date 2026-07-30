package com.lingxi.code.conversation

import androidx.lifecycle.SavedStateHandle
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.SessionRow
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
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
    private class StubSource : ConversationSource {
        var closed = false
        override fun initialMessages(): List<Message> = emptyList()
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
        var cancelCount = 0
        private val never = MutableSharedFlow<ReplyEvent>()
        override fun initialMessages(): List<Message> = emptyList()
        override fun submit(text: String): Flow<ReplyEvent> {
            submitted += text
            return never.asSharedFlow() // a turn that streams forever until cancelled
        }
        override suspend fun cancel() { cancelCount++ }
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

    // --- delta accumulation ----------------------------------------------

    @Test
    fun firstDelta_opensAssistantMessage_andSetsStreaming() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Delta("Hel"))

        val s = vm.state.value
        assertTrue(s.streaming)
        assertEquals(1, s.messages.size)
        assertEquals(Role.Ai, s.messages[0].role)
        assertEquals("Hel", s.messages[0].text)
    }

    @Test
    fun subsequentDeltas_appendIntoSameMessage() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Delta("Hel"))
        vm.reduce(ReplyEvent.Delta("lo "))
        vm.reduce(ReplyEvent.Delta("world"))

        val s = vm.state.value
        assertEquals(1, s.messages.size)
        assertEquals("Hello world", s.messages[0].text)
        assertTrue(s.streaming)
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
        assertEquals(2, s.messages.size)
        assertEquals("turn1", s.messages[0].text)
        assertEquals("turn2", s.messages[1].text)
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
    fun newChat_resetsStreamingIndex_soNextDeltaOpensFresh() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Delta("old turn"))
        vm.newChat()
        vm.reduce(ReplyEvent.Delta("brand new"))

        val s = vm.state.value
        assertTrue(s.isNew)
        assertEquals(1, s.messages.size)
        assertEquals("brand new", s.messages[0].text)
        assertNull("statusLine cleared on newChat", ChatViewModel(StubSource()).state.value.statusLine)
    }

    // --- streaming gate / overlapping-submit guard ------------------------

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
    fun send_whileStreaming_isIgnored_noSecondSubmit() = runTest(dispatcher) {
        val src = RecordingSource()
        val vm = ChatViewModel(src)
        vm.send("first")
        // Overlapping submit while the first turn is still streaming: ignored.
        vm.send("second")
        assertEquals("only the first turn submitted", listOf("first"), src.submitted)
        // The user message for the ignored turn must NOT be appended either.
        assertEquals(1, vm.state.value.messages.count { it.role == Role.User })
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
        assertEquals("partial A", vm.state.value.messages.last { it.role == Role.Ai }.text)

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
}
