package com.lingxi.code.conversation

import org.junit.Assert.assertEquals
import org.junit.Test

class ComposerRestoreGateTest {
    private class Composer(var text: String, var attachment: String?) {
        var textRevision = 0L
        var attachmentRevision = 0L
        fun editText(value: String) { text = value; textRevision++ }
        fun editAttachment(value: String?) { attachment = value; attachmentRevision++ }
        fun revision() = textRevision to attachmentRevision
    }

    @Test fun rejectionBeforeClearRestoresBothFieldsAfterClear() {
        val owner = Composer("original", "photo")
        val gate = ComposerRestoreGate(owner::revision) { owner.editText("original"); owner.editAttachment("photo") }
        gate.rejected()
        assertEquals("original", owner.text)
        owner.editText(""); owner.editAttachment(null)
        gate.cleared()
        assertEquals("original", owner.text)
        assertEquals("photo", owner.attachment)
    }

    @Test fun changingEitherFieldAndChangingItBackDoesNotRestoreStaleSubmission() {
        for (editText in listOf(true, false)) {
            val owner = Composer("original", "photo")
            val preserved = mutableListOf<Pair<String, String?>>()
            val gate = ComposerRestoreGate(owner::revision, preserve = { preserved += "original" to "photo" }) {
                owner.editText("original"); owner.editAttachment("photo")
            }
            owner.editText(""); owner.editAttachment(null); gate.cleared()
            if (editText) { owner.editText("new"); owner.editText("") }
            else { owner.editAttachment("new photo"); owner.editAttachment(null) }
            gate.rejected()
            assertEquals("", owner.text)
            assertEquals(null, owner.attachment)
            assertEquals(listOf("original" to "photo"), preserved)
        }
    }

    @Test fun failureAfterNewTextOrPhotoPreservesSubmittedPairWithoutMixing() {
        for (newText in listOf(true, false)) {
            val owner = Composer("original", "photo")
            val preserved = mutableListOf<Pair<String, String?>>()
            val gate = ComposerRestoreGate(owner::revision, preserve = { preserved += "original" to "photo" }) {
                owner.editText("original"); owner.editAttachment("photo")
            }
            owner.editText(""); owner.editAttachment(null); gate.cleared()
            if (newText) owner.editText("new") else owner.editAttachment("new photo")
            gate.rejected(); gate.rejected()
            assertEquals(if (newText) "new" else "", owner.text)
            assertEquals(if (newText) null else "new photo", owner.attachment)
            assertEquals(listOf("original" to "photo"), preserved)
        }
    }

    @Test fun rejectionRestoresItsOwnHiddenOwnerOnly() {
        val original = Composer("original", "photo")
        val selected = Composer("other", "other photo")
        val gate = ComposerRestoreGate(original::revision) { original.editText("original"); original.editAttachment("photo") }
        original.editText(""); original.editAttachment(null); gate.cleared()
        selected.editText("other edited")
        gate.rejected()
        assertEquals("original", original.text)
        assertEquals("photo", original.attachment)
        assertEquals("other edited", selected.text)
        assertEquals("other photo", selected.attachment)
    }
}
