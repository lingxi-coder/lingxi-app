package com.lingxi.code.conversation

import com.lingxi.code.bindings.android.MediaAudioSessionBindingFfi
import org.junit.Assert.*
import org.junit.Test

class MediaAudioSessionContextBindingsTest {
    private fun token(host: String, attachment: ULong = 1u, session: String = "same-session", generation: ULong = 7u) =
        MediaAudioSessionBindingFfi(host, attachment, session, generation)

    @Test fun teardownReturnsTheExactAttachmentTokenAndIssuingEngine() {
        val engine = Any()
        val removed = mutableListOf<Pair<Any, MediaAudioSessionBindingFfi>>()
        val routes = MediaAudioSessionContextBindings<Any> { owner, binding -> removed += owner to binding }
        val original = token("host-a", attachment = 42u)
        routes.register(engine, original)
        routes.remove(engine, original.sessionId)
        assertSame(engine, removed.single().first)
        assertSame(original, removed.single().second)
    }

    @Test fun replacedSessionKeepsTheLatestFullBindingForThatEngine() {
        val engine = Any()
        val removed = mutableListOf<MediaAudioSessionBindingFfi>()
        val routes = MediaAudioSessionContextBindings<Any> { _, binding -> removed += binding }
        routes.register(engine, token("host-a", attachment = 1u))
        val replacement = token("host-b", attachment = 2u)
        routes.register(engine, replacement)
        routes.close()
        assertEquals(listOf(replacement), removed)
    }

    @Test fun lateOldEngineRemovalCannotEraseNewEngineWithSameSessionAndGeneration() {
        val removed = mutableListOf<Pair<Any, MediaAudioSessionBindingFfi>>()
        val routes = MediaAudioSessionContextBindings<Any> { owner, binding -> removed += owner to binding }
        val oldEngine = Any()
        val newEngine = Any()
        val oldToken = token("old-host", attachment = 11u)
        val newToken = token("new-host", attachment = 12u)
        routes.register(oldEngine, oldToken)
        routes.register(newEngine, newToken)
        routes.remove(oldEngine, oldToken.sessionId)
        routes.remove(oldEngine, oldToken.sessionId)
        assertEquals(listOf(oldEngine to oldToken), removed)
        routes.close()
        assertEquals(listOf(oldEngine to oldToken, newEngine to newToken), removed)
    }

    @Test fun registrationAfterCloseReturnsItsOwnTokenAndRepeatedCloseIsHarmless() {
        val engine = Any()
        val removed = mutableListOf<MediaAudioSessionBindingFfi>()
        val routes = MediaAudioSessionContextBindings<Any> { _, binding -> removed += binding }
        val first = token("engine", session = "first")
        val second = token("engine", session = "second")
        routes.register(engine, first)
        routes.close()
        routes.register(engine, second)
        routes.close()
        assertEquals(listOf(first, second), removed)
    }

    @Test fun unavailableOldEngineClearsOnlyItsOwnExactRoutes() {
        val oldEngine = Any()
        val currentEngine = Any()
        val removed = mutableListOf<Pair<Any, MediaAudioSessionBindingFfi>>()
        val routes = MediaAudioSessionContextBindings<Any> { owner, binding -> removed += owner to binding }
        val oldToken = token("old-host")
        val currentToken = token("current-host")
        routes.register(oldEngine, oldToken)
        routes.register(currentEngine, currentToken)
        routes.removeEngine(oldEngine)
        routes.removeEngine(oldEngine)
        assertEquals(listOf(oldEngine to oldToken), removed)
        routes.close()
        assertEquals(listOf(oldEngine to oldToken, currentEngine to currentToken), removed)
    }
}
