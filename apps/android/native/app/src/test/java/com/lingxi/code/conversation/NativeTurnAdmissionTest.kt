package com.lingxi.code.conversation

import org.junit.Assert.*
import org.junit.Test

class NativeTurnAdmissionTest {
    @Test fun followUpStaysEditableUntilTheMatchingFirstTurnStarts() {
        val admission = NativeTurnAdmission()
        admission.start(7L)
        var draft = "second prompt"
        var attachment = true
        val sent = mutableListOf<String>()
        fun attempt() = sendComposerIfAdmitted(
            admitted = !admission.blocksFollowUp(streaming = true),
            send = { sent += draft }, clearDraft = { draft = "" },
            removeAttachment = { attachment = false }, followTail = {},
        )
        attempt()
        assertTrue(sent.isEmpty())
        assertEquals("second prompt", draft)
        assertTrue(attachment)
        admission.confirm(8L)
        assertTrue(admission.blocksFollowUp(streaming = true))
        admission.confirm(7L)
        attempt()
        assertEquals(listOf("second prompt"), sent)
        assertEquals("", draft)
        assertFalse(attachment)
    }

    @Test fun failedOrCancelledFirstTurnReleasesAdmissionWithoutClearingDraft() {
        val admission = NativeTurnAdmission()
        admission.start(7L)
        assertTrue(admission.blocksFollowUp(streaming = true))
        admission.clear()
        assertFalse(admission.blocksFollowUp(streaming = true))
    }
}
