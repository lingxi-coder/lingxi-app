package com.lingxi.code.conversation

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AssistantMessageCollapsePolicyTest {
    @Test fun ordinaryAssistantRepliesStayReadable() {
        assertFalse(AssistantMessageCollapsePolicy.shouldCollapse("长".repeat(700)))
        assertFalse(AssistantMessageCollapsePolicy.shouldCollapse(List(22) { "step" }.joinToString("\n")))
    }
    @Test fun characterBoundariesMatchDesktop() {
        assertFalse(AssistantMessageCollapsePolicy.shouldCollapse("x".repeat(640), isUser = true))
        assertTrue(AssistantMessageCollapsePolicy.shouldCollapse("x".repeat(641), isUser = true))
        assertFalse(AssistantMessageCollapsePolicy.shouldCollapse("x".repeat(8_000)))
        assertTrue(AssistantMessageCollapsePolicy.shouldCollapse("x".repeat(8_001)))
    }
    @Test fun hardLineBoundariesMatchDesktop() {
        assertFalse(AssistantMessageCollapsePolicy.shouldCollapse(List(8) { "x" }.joinToString("\r\n"), isUser = true))
        assertTrue(AssistantMessageCollapsePolicy.shouldCollapse(List(9) { "x" }.joinToString("\r"), isUser = true))
        assertFalse(AssistantMessageCollapsePolicy.shouldCollapse(List(80) { "x" }.joinToString("\n")))
        assertTrue(AssistantMessageCollapsePolicy.shouldCollapse(List(81) { "x" }.joinToString("\n")))
    }
    @Test fun unicodeAndWhitespaceMatchDesktop() {
        assertFalse(AssistantMessageCollapsePolicy.shouldCollapse("😀".repeat(640), isUser = true))
        assertTrue(AssistantMessageCollapsePolicy.shouldCollapse("😀".repeat(641), isUser = true))
        assertFalse(AssistantMessageCollapsePolicy.shouldCollapse(" \r\n".repeat(1_000), isUser = true))
    }
}
