package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.PromptModeDto
import org.junit.Assert.*
import org.junit.Test

class SideChatCommandTest {
    @Test fun backgroundFencesPendingCreateEvenAfterForegroundReturns() {
        val gate = SideChatForegroundGate()
        val pendingCreate = gate.capture()
        assertTrue(gate.maySubmit(pendingCreate))
        gate.background()
        assertFalse(gate.maySubmit(pendingCreate))
        gate.resume()
        assertFalse(gate.maySubmit(pendingCreate))
        assertTrue(gate.maySubmit(gate.capture()))
    }

    @Test fun failedSideImageSendRestoresRamComposer() {
        val restored = restoreSideComposerAfterFailure("", null, "describe this", "submitted image")
        assertEquals("describe this", restored.first)
        assertEquals("submitted image", restored.second)
        val edited = restoreSideComposerAfterFailure("new draft", "new image", "old draft", "old image")
        assertEquals("new draft", edited.first)
        assertEquals("new image", edited.second)
    }

    @Test fun rejectedSideCreationRetainsDraftAndAttachment() {
        assertComposerRetained("/side explain this", hasImages = true, isSideChat = false)
    }

    @Test fun rejectedNestedSideRetainsDraftAndAttachment() {
        assertComposerRetained("/side nested", hasImages = false, isSideChat = true)
        assertComposerRetained("/side nested", hasImages = true, isSideChat = true)
    }

    private fun assertComposerRetained(text: String, hasImages: Boolean, isSideChat: Boolean) {
        var draft = text
        var attachmentRetained = hasImages
        var submitted = false
        var followed = false
        val admitted = sideChatSendRejection(text, hasImages, isSideChat) == null
        assertFalse(admitted)
        sendComposerIfAdmitted(admitted, { submitted = true }, { draft = "" },
            { attachmentRetained = false }, { followed = true })
        assertEquals(text, draft)
        assertEquals(hasImages, attachmentRetained)
        assertFalse(submitted)
        assertFalse(followed)
    }

    @Test fun admittedSendClearsComposerAfterSubmission() {
        listOf("/side explain this" to false, "ordinary prompt" to true).forEach { (text, hasImages) ->
            var draft = text
            var attachmentRetained = hasImages
            val events = mutableListOf<String>()
            val admitted = sideChatSendRejection(text, hasImages, isSideChat = false) == null
            assertTrue(admitted)
            sendComposerIfAdmitted(admitted, { events += "send" }, { draft = ""; events += "draft" },
                { attachmentRetained = false; events += "attachment" }, { events += "follow" })
            assertEquals("", draft)
            assertFalse(attachmentRetained)
            assertEquals(listOf("send", "draft", "attachment", "follow"), events)
        }
    }

    @Test fun matchesOnlyCommandBoundary() {
        assertEquals("", sideChatInput(" /side "))
        assertEquals("explain this", sideChatInput("/side explain this"))
        assertEquals("explain", sideChatInput("/side\nexplain"))
        assertNull(sideChatInput("/sideways"))
        assertNull(sideChatInput("mention /side"))
        assertNull(sideChatInput("/Side"))
    }

    @Test fun sidePromptPreservesRawTextModeTurnAndVisualizationContext() {
        val rawText = "!echo \uD800"
        val command = sideChatSendPromptCommand(
            text = rawText,
            images = emptyList(),
            turnId = 42L,
            visualizationContext = VisualizationRef("widget-a", 3u),
            promptMode = PromptModeDto.BASH,
        )

        assertEquals(rawText.map { it.code.toUShort() }, command.text.codeUnits)
        assertEquals(PromptModeDto.BASH, command.promptMode)
        assertEquals(42uL, command.turnId)
        assertEquals("widget-a", command.visualizationContext?.id)
        assertEquals(3u, command.visualizationContext?.revision)
    }

    @Test fun sideCancelPreservesOptionalTurnIdentity() {
        assertEquals(ClientCommand.Cancel(42uL), sideChatCancelCommand(42L))
        assertEquals(ClientCommand.Cancel(null), sideChatCancelCommand(null))
    }
}
