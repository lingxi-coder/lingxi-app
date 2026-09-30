package com.lingxi.code.conversation

import org.junit.Assert.*
import org.junit.Test

class DisclosurePersistenceTest {
    @Test fun corruptOrTemporarySessionPreferencesAreIgnored() {
        assertTrue(decodeDisclosures("broken").isEmpty())
        assertTrue(decodeDisclosures(encodeDisclosures(mapOf("new" to setOf("id")))).isEmpty())
        assertTrue(decodeDisclosures("x".repeat(MAX_DISCLOSURE_CHARS + 1)).isEmpty())
    }

    @Test fun preferenceEncodingStaysBoundedAndRetainsMostRecentSession() {
        val sessions = linkedMapOf<String, Set<String>>()
        repeat(400) { session -> sessions["session-$session"] = (0 until 100).map { "tool-$it-${"x".repeat(64)}" }.toSet() }
        val encoded = encodeDisclosures(sessions)
        assertTrue(encoded.length <= MAX_DISCLOSURE_CHARS)
        assertEquals(sessions["session-399"], decodeDisclosures(encoded)["session-399"])
    }
}
