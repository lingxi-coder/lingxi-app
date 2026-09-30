package com.lingxi.code.conversation

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AssistantMessageCollapsePolicyTest {
    @Test
    fun shortReplyStaysExpanded() {
        assertFalse(AssistantMessageCollapsePolicy.shouldCollapse("A concise reply."))
    }

    @Test
    fun longUnbrokenReplyCanCollapse() {
        assertTrue(AssistantMessageCollapsePolicy.shouldCollapse("长".repeat(700)))
    }

    @Test
    fun manyShortLinesCanCollapse() {
        assertTrue(AssistantMessageCollapsePolicy.shouldCollapse(List(22) { "step" }.joinToString("\n")))
    }
}
