package com.lingxi.code.conversation

import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import org.junit.Assert.*
import org.junit.Test

class MessageLinkNavigationTest {
    private val uuid = "f4e89fba-1c3d-4a5c-9a6d-1b75d92ca85f"
    @Test fun `linked persisted record is revealed even when a loop summary normally folds it`() {
        val target = Message(Role.User, "earlier", transcriptUuid = uuid)
        val summary = Message(Role.Ai, "summary", loopWakeupStreak = 1u, loopFoldedItemIds = setOf(target.id))
        val state = ChatState(com.lingxi.code.model.SessionRef("session", ""), listOf(target, summary), model = com.lingxi.code.model.EngineModelCatalog.pending)
        assertEquals(-1, messageLinkRenderIndex(buildChatRenderItems(state), state.messages, uuid))
        assertEquals(0, messageLinkRenderIndex(buildChatRenderItems(state, uuid), state.messages, uuid))
    }
    @Test fun `independent history replays retain durable UUID while UI IDs are recreated`() {
        val dto = com.lingxi.code.bindings.client.MessageDto(
            transcriptUuid = uuid,
            role = "assistant",
            blocks = listOf(com.lingxi.code.bindings.client.MessageBlockDto.Text("persisted".toClientTextDto())),
        )
        val first = transcriptFromDtos(listOf(dto)).single()
        val replay = transcriptFromDtos(listOf(dto)).single()
        assertNotEquals(first.id, replay.id)
        assertEquals(uuid, replay.transcriptUuid)
        assertEquals(0, messageLinkRenderIndex(listOf(ChatRenderItem.Message(replay)), listOf(replay), uuid))
    }
    @Test fun `persisted UUID finds projected prose after UI row IDs change`() {
        val envelope = Message(Role.Ai, "one two", id = "recreated-row", transcriptUuid = uuid)
        val projected = envelope.copy(id = "recreated-row:prose:2", text = "two")
        assertEquals(1, messageLinkRenderIndex(listOf(ChatRenderItem.Empty, ChatRenderItem.Message(projected)), listOf(envelope), uuid.uppercase()))
        assertEquals(-1, messageLinkRenderIndex(listOf(ChatRenderItem.Message(projected)), listOf(envelope), "recreated-row"))
    }
    @Test fun `missing persisted message does not fall back to session or first row`() {
        val other = Message(Role.User, "unrelated")
        assertEquals(-1, messageLinkRenderIndex(listOf(ChatRenderItem.Message(other)), listOf(other), uuid))
    }
}
