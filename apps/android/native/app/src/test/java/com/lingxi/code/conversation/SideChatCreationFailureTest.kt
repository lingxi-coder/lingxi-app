package com.lingxi.code.conversation

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class SideChatCreationFailureTest {
    @Test fun toolbarCreationFailureDoesNotPublishAParentDraft() = runTest {
        val manager = SideChatManager(backgroundScope)
        var failures = 0
        manager.create(input = "", draftOnFailure = null,
            newSource = { throw IllegalStateException("side chat host is closed") }) {
            failures++
        }
        assertTrue(manager.isCreating.value)
        runCurrent()

        assertNull(manager.failedDraft.value)
        assertEquals("side chat host is closed", manager.error.value)
        assertEquals(1, failures)
        assertFalse(manager.isCreating.value)
        assertFalse(manager.hasLiveChats)
        assertTrue(manager.chats.value.isEmpty())
        assertNull(manager.selected.value)
        assertFalse(manager.visible.value)
    }

    @Test fun commandCreationFailureRestoresTheExactSubmittedDraft() = runTest {
        for (draft in listOf("/side", " /side  explain\nthis\uD800 ")) {
            val manager = SideChatManager(backgroundScope)
            var restored: String? = null
            manager.create(input = sideChatInput(draft)!!, draftOnFailure = draft,
                newSource = { throw IllegalStateException("native creation failed") }) {
                restored = manager.failedDraft.value
            }
            runCurrent()

            assertEquals(draft, restored)
            assertEquals(draft, manager.failedDraft.value)
            assertEquals("native creation failed", manager.error.value)
            assertFalse(manager.isCreating.value)
            assertFalse(manager.hasLiveChats)
        }
    }

    @Test fun toolbarFailureClearsAnEarlierCommandRecoveryAndAllowsRetry() = runTest {
        val manager = SideChatManager(backgroundScope)
        manager.create(input = "question", draftOnFailure = "/side question",
            newSource = { throw IllegalStateException("first failure") }) {}
        runCurrent()
        assertEquals("/side question", manager.failedDraft.value)
        var vacant = 0
        manager.whenVacant = { vacant++ }
        manager.create(input = "", draftOnFailure = null,
            newSource = { throw IllegalStateException("retry failure") }) {}
        assertNull(manager.failedDraft.value)
        assertNull(manager.error.value)
        runCurrent()

        assertNull(manager.failedDraft.value)
        assertEquals("retry failure", manager.error.value)
        assertFalse(manager.isCreating.value)
        assertFalse(manager.hasLiveChats)
        assertEquals(1, vacant)
    }
}
