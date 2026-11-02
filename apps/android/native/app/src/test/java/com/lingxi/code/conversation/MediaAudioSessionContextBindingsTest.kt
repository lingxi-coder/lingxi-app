package com.lingxi.code.conversation

import com.lingxi.code.bindings.android.MediaAudioSessionBindingFfi
import org.junit.Assert.assertEquals
import org.junit.Test

class MediaAudioSessionContextBindingsTest {
    @Test fun cleanupUsesTheExactAttachmentBindingAndIgnoresOlderGeneration() {
        val removed = mutableListOf<MediaAudioSessionBindingFfi>()
        val bindings = MediaAudioSessionContextBindings { removed += it }
        val old = MediaAudioSessionBindingFfi("host", 1uL, "session", 1uL)
        val current = MediaAudioSessionBindingFfi("host", 2uL, "session", 2uL)
        bindings.register(old)
        bindings.register(current)
        bindings.remove("session")
        bindings.close()
        assertEquals(listOf(current), removed)
        bindings.register(old)
        assertEquals(listOf(current, old), removed)
    }
}
