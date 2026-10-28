package com.lingxi.code.acceptance

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class AcceptanceArgsTest {
    @Test fun optInsAndRequiredArgumentsFailClosed() {
        assertFalse(acceptanceEnabled("flag", null))
        assertFalse(acceptanceEnabled("flag", "false"))
        assertTrue(acceptanceEnabled("flag", "true"))
        for (value in listOf("", "True", "yes", " true")) {
            assertThrows(IllegalArgumentException::class.java) { acceptanceEnabled("flag", value) }
        }
        assertEquals("value", requiredAcceptanceArg("fixture", "value"))
        for (value in listOf(null, "", "  ")) {
            assertThrows(IllegalArgumentException::class.java) { requiredAcceptanceArg("fixture", value) }
        }
    }

    @Test fun explicitClassMethodAndLegacyArgumentsCannotLookLikeAnOrdinarySkip() {
        val target = "com.example.Acceptance#specialCase"
        assertFalse(acceptanceRequested(target, null, emptyList()))
        assertTrue(acceptanceRequested(target, null, listOf("legacy-fixture")))
        assertTrue(acceptanceRequested(target, "com.example.Acceptance", emptyList()))
        assertTrue(acceptanceRequested(target, "com.example.Other,com.example.Acceptance#specialCase", emptyList()))
        assertFalse(acceptanceRequested(target, "com.example.Acceptance#otherCase", listOf("legacy-fixture")))
        assertFalse(acceptanceRequested(target, "com.example.Unrelated", listOf("legacy-fixture")))
    }
}
