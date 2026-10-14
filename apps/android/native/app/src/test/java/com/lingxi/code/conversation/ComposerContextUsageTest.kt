package com.lingxi.code.conversation

import org.junit.Assert.*
import org.junit.Test

class ComposerContextUsageTest {
    @Test fun cachesCountButOutputDoesNot() {
        val snapshot = ComposerContextUsage.from(AgentRunUsage(100, 9_000, 200, 50), 1_000uL)!!
        assertEquals(350L, snapshot.used)
        assertEquals(35, snapshot.percent)
    }

    @Test fun missingOrInvalidDataHidesIndicator() {
        val zero = AgentRunUsage(0, 0, 0, 0)
        assertNull(ComposerContextUsage.from(null, 1_000uL))
        assertNull(ComposerContextUsage.from(zero, null))
        assertNull(ComposerContextUsage.from(zero, 0uL))
        assertEquals(0, ComposerContextUsage.from(zero, 1_000uL)!!.percent)
        assertNull(ComposerContextUsage.from(AgentRunUsage(-1, 0, 0, 0), 1_000uL))
        assertNull(ComposerContextUsage.from(AgentRunUsage(Long.MAX_VALUE, 0, 1, 0), 1_000uL))
    }

    @Test fun percentClampsWithoutTruncatingTokenCount() {
        val snapshot = ComposerContextUsage.from(AgentRunUsage(1_005, 0, 0, 0), 1_000uL)!!
        assertEquals(100, snapshot.percent)
        assertEquals(1_005L, snapshot.used)
    }
}
