package com.lingxi.code.conversation

import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import org.junit.Assert.assertEquals
import org.junit.Test

class TranscriptSpacingTest {
    private val user = ChatRenderItem.Message(Message(Role.User, "Question", id = "user"))
    private val reply = ChatRenderItem.Message(Message(Role.Ai, "Reply", id = "reply"))

    @Test fun messageAndTurnBoundariesMatchDesktop() {
        assertEquals(0, transcriptSpacingBefore(null, user))
        assertEquals(24, transcriptSpacingBefore(reply, reply))
        assertEquals(32, transcriptSpacingBefore(user, reply))
        assertEquals(32, transcriptSpacingBefore(reply, user))
        assertEquals(32, transcriptSpacingBefore(user, ChatRenderItem.StreamingIndicator))
        assertEquals(12, transcriptSpacingBefore(reply, ChatRenderItem.StreamingIndicator))
    }

    @Test fun toolsKeepCompactSpacingAcrossExecutionStates() {
        val tools = ChatRenderItem.Tools(listOf(ToolCallUi(id = "read", tool = "Read", status = AgentToolStatus.Running)))
        val completed = ChatRenderItem.Tools(tools.calls.map { it.copy(status = AgentToolStatus.Completed) })
        assertEquals(32, transcriptSpacingBefore(user, tools))
        assertEquals(32, transcriptSpacingBefore(tools, user))
        assertEquals(12, transcriptSpacingBefore(reply, tools))
        assertEquals(12, transcriptSpacingBefore(tools, reply))
        assertEquals(transcriptSpacingBefore(reply, tools), transcriptSpacingBefore(reply, completed))
        assertEquals(transcriptSpacingBefore(tools, ChatRenderItem.StreamingIndicator),
            transcriptSpacingBefore(completed, ChatRenderItem.StreamingIndicator))
    }

    @Test fun streamingSettlementKeepsIdentityAndSpacing() {
        val streaming = ChatRenderItem.Streaming(reply.message)
        assertEquals(reply.key, streaming.key)
        assertEquals(reply.contentType, streaming.contentType)
        for (previous in listOf(user, reply, ChatRenderItem.StreamingIndicator)) {
            assertEquals(transcriptSpacingBefore(previous, streaming), transcriptSpacingBefore(previous, reply))
        }
        assertEquals(transcriptSpacingBefore(streaming, ChatRenderItem.StreamingIndicator),
            transcriptSpacingBefore(reply, ChatRenderItem.StreamingIndicator))
    }
}
