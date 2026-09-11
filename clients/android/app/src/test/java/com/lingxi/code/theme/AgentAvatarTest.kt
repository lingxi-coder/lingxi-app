package com.lingxi.code.theme

import org.junit.Assert.assertEquals
import org.junit.Test

class AgentAvatarTest {
    @Test fun desktopUtf16IdentityVectors() {
        listOf("" to 0, "main" to 13, "agent-review-42" to 13, "搜索-agent" to 20, "🧠-review" to 23, "a".repeat(128) to 9)
            .forEach { (id, expected) -> assertEquals(id, expected, agentAvatarIndex(id)) }
        assertEquals(28, (0..299).map { agentAvatarIndex("agent-$it") }.toSet().size)
    }
}
