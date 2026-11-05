package com.lingxi.code.conversation

import com.lingxi.code.R
import com.lingxi.code.bindings.client.MessageBlockDto
import com.lingxi.code.bindings.client.MessageDto
import com.lingxi.code.bindings.client.PageRefDto
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class PrivatePageTranscriptTest {
    private val strings = ConversationStrings { id, fallback, _ ->
        if (id == R.string.private_page_unavailable) "Private page unavailable" else fallback
    }

    @Test fun restoredPrivatePageKeepsSurroundingProseAndHasALocalizedUnavailableNotice() {
        for (reference in listOf(PageRefDto("desktop-private-notes", 2u), null)) {
            val blocks = listOf(
                MessageBlockDto.Text("Before".toClientTextDto()),
                MessageBlockDto.Page(reference),
                MessageBlockDto.Text("After".toClientTextDto()),
            )
            val message = transcriptFromDtos(listOf(dto("assistant", blocks)), strings).single()

            assertEquals("Before\n\nPrivate page unavailable\n\nAfter", message.text)
            assertEquals(listOf("Before", "Private page unavailable", "After"),
                message.blocks.map { (it as MessageContent.Text).text })
        }
    }

    @Test fun anUnavailablePrivatePageIsNotDroppedAsAnEmptyAssistantTurn() {
        val blocks = listOf(MessageBlockDto.Page(null))

        assertEquals("Private page unavailable",
            transcriptFromDtos(listOf(dto("assistant", blocks)), strings).single().text)
    }

    @Test fun aPrivatePageReferenceCannotCreateAUserBubble() {
        val blocks = listOf(MessageBlockDto.Page(PageRefDto("desktop-private-notes", 2u)))

        assertTrue(transcriptFromDtos(listOf(dto("user", blocks)), strings).isEmpty())
    }

    private fun dto(role: String, blocks: List<MessageBlockDto>) =
        MessageDto(transcriptUuid = null, role = role, blocks = blocks)
}
