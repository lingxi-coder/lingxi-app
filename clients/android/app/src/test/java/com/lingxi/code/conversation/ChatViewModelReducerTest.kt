package com.lingxi.code.conversation

import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.emptyFlow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Reducer-level tests for [ChatViewModel] — the second half of the engine-path
 * coverage (the first being [ClientEventMapperTest]). These drive the `internal`
 * [ChatViewModel.reduce] directly with a stub source so no coroutine / engine is
 * involved: we assert the exact [ChatState] transition each [ReplyEvent] causes,
 * including the streaming-message accumulation that mirrors the iOS
 * `EngineConversationSource.appendDelta`.
 */
class ChatViewModelReducerTest {

    /** A source that supplies an empty transcript and never streams (reduce is driven directly). */
    private class StubSource : ConversationSource {
        override fun initialMessages(): List<Message> = emptyList()
        override fun submit(text: String): Flow<ReplyEvent> = emptyFlow()
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
    fun error_setsStatusLine_clearsStreaming() {
        val vm = newVm()
        vm.reduce(ReplyEvent.Delta("partial"))
        vm.reduce(ReplyEvent.Error("kaboom"))

        val s = vm.state.value
        assertFalse(s.streaming)
        assertTrue(s.statusLine!!.contains("kaboom"))
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
}
