package com.lingxi.code.voice.audio

import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioOwnerBindingRegistryTest {
    @Test
    fun computerUseKeepsItsEngineAcrossNavigationAndStaleCleanup() {
        val bindings = AudioOwnerBindingRegistry<Any>()
        val oldCode = Any()
        val newCode = Any()
        val visibleChat = Any()
        val codeOwner = AudioOwnerKey.computerUse("code-generation")
        val chatOwner = AudioOwnerKey.computerUse("chat-generation")

        bindings.bind(codeOwner, oldCode)
        bindings.bind(chatOwner, visibleChat)
        bindings.attach(visibleChat)
        assertSame(oldCode, bindings.engineFor(codeOwner))
        assertSame(visibleChat, bindings.engineFor(chatOwner))

        bindings.bind(codeOwner, newCode)
        bindings.releaseIfBound(codeOwner, oldCode)
        bindings.detach(oldCode)
        assertSame(newCode, bindings.engineFor(codeOwner))

        bindings.releaseIfBound(codeOwner, newCode)
        assertTrue(bindings.isReserved(codeOwner))
        assertNull(bindings.engineFor(codeOwner))
        assertSame(visibleChat, bindings.engineFor(chatOwner))
    }
}
