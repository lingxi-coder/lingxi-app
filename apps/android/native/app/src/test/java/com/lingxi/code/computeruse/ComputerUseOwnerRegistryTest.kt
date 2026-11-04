package com.lingxi.code.computeruse

import org.junit.Assert.assertEquals
import org.junit.Test

class ComputerUseOwnerRegistryTest {
    @Test fun concurrentAndNestedCallsKeepTheirEngineOwnersUntilTheyReturn() {
        val registry = ComputerUseOwnerRegistry()
        registry.enter("code")
        registry.enter("code")
        registry.enter("chat")
        assertEquals(setOf("code", "chat"), registry.owners())
        registry.leave("code")
        assertEquals(setOf("code", "chat"), registry.owners())
        registry.leave("chat")
        registry.leave("code")
        assertEquals(emptySet<String>(), registry.owners())
    }
}
