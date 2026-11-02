package com.lingxi.code.conversation

import com.lingxi.code.model.SessionMode
import org.junit.Assert.*
import org.junit.Test

class SessionOwnerRegistryTest {
    @Test fun ownersKeepTheirStateAndModeAcrossNavigation() {
        data class Owner(var draft: String, var running: Boolean)
        val registry = SessionOwnerRegistry<Owner>()
        val first = Owner("first draft", true)
        val second = Owner("second draft", false)
        assertTrue(registry.register("global", "a", SessionMode.Chat, first))
        assertTrue(registry.register("global", "b", SessionMode.Code, second))
        assertSame(second, registry.find("global", "b", SessionMode.Code))
        assertSame(first, registry.find("global", "a", SessionMode.Chat))
        assertTrue(first.running)
        assertEquals("first draft", first.draft)
        assertNull(registry.find("global", "a", SessionMode.Code))
        assertNull(registry.find("project.one", "a", SessionMode.Chat))
        assertFalse(registry.register("global", "a", SessionMode.Chat, second))
        assertFalse(registry.register("project.one", "a", SessionMode.Chat, second))
        assertTrue(registry.contains("a"))
        assertSame(first, registry.find("global", "a", SessionMode.Chat))
    }

    @Test fun unavailableStorageBlocksNewOwnersWithoutDisplacingAnAdmittedTurn() {
        data class Owner(val draft: String, var running: Boolean)
        val registry = SessionOwnerRegistry<Owner>()
        val active = Owner("unsent draft", true)
        val other = Owner("other", false)
        assertTrue(registry.register("global", "a", SessionMode.Chat, active))
        assertTrue(registry.register("global", "a", SessionMode.Chat, active, allowNew = false))
        assertFalse(registry.register("global", "b", SessionMode.Chat, other, allowNew = false))
        assertFalse(registry.register("project.p", "a", SessionMode.Chat, other, allowNew = false))
        assertFalse(shouldBlockForUnavailableStorage(true, hasRegisteredOwner = true))
        assertTrue(shouldBlockForUnavailableStorage(true, hasRegisteredOwner = false))
        assertSame(active, registry.find("global", "a", SessionMode.Chat))
        assertTrue(active.running)
        assertEquals("unsent draft", active.draft)
    }

    @Test fun ownerStateRevalidationKeepsAnAdmittedConversationVisible() {
        for (validation in listOf(null, false)) {
            assertFalse(shouldBlockForOwnerStateValidation(validation, hasRegisteredOwner = true))
            assertTrue(shouldBlockForOwnerStateValidation(validation, hasRegisteredOwner = false))
        }
        assertFalse(shouldBlockForOwnerStateValidation(true, hasRegisteredOwner = false))
    }
}
