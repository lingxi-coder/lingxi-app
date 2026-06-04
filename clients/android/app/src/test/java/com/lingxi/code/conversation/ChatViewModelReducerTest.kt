package com.lingxi.code.conversation

import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
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
        override fun initialMessages(): List<Message> = emptyList()
        override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()
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
    }

    // --- status line ------------------------------------------------------

    @Test
    fun toolActivity_setsStatusLine() {
        val vm = newVm()
        vm.reduce(ReplyEvent.ToolActivity("调用工具 bash…"))
        assertEquals("调用工具 bash…", vm.state.value.statusLine)
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
    }

    @Test
    fun cancel_whenIdle_isNoOp() = runTest(dispatcher) {
        val src = RecordingSource()
        val vm = ChatViewModel(src)
        vm.cancel()
        assertEquals("no cancel sent when no turn in flight", 0, src.cancelCount)
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
}
