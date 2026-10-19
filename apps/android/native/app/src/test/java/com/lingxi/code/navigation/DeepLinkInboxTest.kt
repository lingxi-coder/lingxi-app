package com.lingxi.code.navigation

import androidx.lifecycle.SavedStateHandle
import org.junit.Assert.*
import org.junit.Test

class DeepLinkInboxTest {
    @Test fun `rotation never redelivers consumed intent but a new process restore intent and explicit warm repeat are handled`() {
        val saved = SavedStateHandle()
        val first = DeepLinkInbox(saved)
        first.deliver("lingxi://project/one")
        first.complete("lingxi://project/one")
        val rotated = DeepLinkInbox(saved)
        rotated.deliver("lingxi://project/one", restoringActivity = true)
        assertTrue(rotated.links.value.isEmpty())
        rotated.deliver("lingxi://project/two", restoringActivity = true)
        assertEquals(listOf("lingxi://project/two"), rotated.links.value)
        rotated.complete("lingxi://project/two")
        rotated.deliver("lingxi://project/two")
        assertEquals(listOf("lingxi://project/two"), rotated.links.value)
    }
    @Test fun `background startup onboarding execution and editor blockers retain requests until foreground readiness`() {
        val saved = SavedStateHandle()
        val inbox = DeepLinkInbox(saved)
        inbox.enqueue("lingxi://new#payload=%7B%22prompt%22%3A%22draft%22%7D")
        val ready = DeepLinkReadiness(true, true, true, false, false)
        for (blocked in listOf(ready.copy(foreground = false), ready.copy(onboardingComplete = false), ready.copy(sourceBound = false), ready.copy(engineBusy = true), ready.copy(blockingSurfaceOpen = true))) {
            assertNull(inbox.next(blocked))
            assertEquals(1, inbox.links.value.size)
        }
        val recreated = DeepLinkInbox(saved)
        assertEquals(inbox.links.value.first(), recreated.next(ready))
        recreated.complete(recreated.next(ready)!!)
        assertNull(recreated.next(ready))
    }
    @Test fun `cold and warm links remain FIFO across recreation and consume only their own head`() {
        val saved = SavedStateHandle()
        val inbox = DeepLinkInbox(saved)
        assertTrue(inbox.enqueue("lingxi://session/first"))
        assertTrue(inbox.enqueue("lingxi://session/second"))
        assertTrue(inbox.enqueue("lingxi://session/first"))
        val recreated = DeepLinkInbox(saved)
        recreated.complete("lingxi://session/second")
        assertEquals(listOf("lingxi://session/first", "lingxi://session/second"), recreated.links.value)
        recreated.complete("lingxi://session/first")
        assertEquals(listOf("lingxi://session/second"), recreated.links.value)
    }
    @Test fun `bounded queue rejects overflow without losing the oldest request`() {
        val inbox = DeepLinkInbox(SavedStateHandle())
        repeat(DeepLinkInbox.CAPACITY) { assertTrue(inbox.enqueue("lingxi://project/p$it")) }
        assertFalse(inbox.enqueue("lingxi://project/overflow"))
        assertFalse(inbox.enqueue("https://evil.test/open/new"))
        assertEquals("lingxi://project/p0", inbox.links.value.first())
    }
}
