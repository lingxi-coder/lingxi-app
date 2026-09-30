package com.lingxi.code.conversation

import org.junit.Assert.*
import org.junit.Test

class PlanDocumentTest {
    @Test fun explicitDocumentPreservesSurroundingProse() {
        val parts = planTextParts("Before\n<proposed_plan>\n# Title\n\n- Step\n</proposed_plan>\nAfter")
        assertEquals(listOf(false, true, false), parts.map { it.plan })
        assertEquals("# Title\n\n- Step", parts[1].text)
        assertFalse(parts[1].writing)
    }
    @Test fun toolInputUsesOnlyExplicitPlanAndDecodesUnicode() {
        assertEquals("# 中文\nStep", toolPlanMarkdown("ExitPlanMode", "{\"plan\":\"# 中文\\nStep\"}"))
        assertNull(toolPlanMarkdown("Read", "{\"plan\":\"ignored\"}"))
    }
    @Test fun jsonUnicodeEscapesPreserveChinese() {
        assertEquals("中文", toolPlanMarkdown("ExitPlanMode", """{"plan":"\u4e2d\u6587"}"""))
    }
    @Test fun jsonUnicodeSurrogatePairPreservesEmoji() {
        assertEquals("🚀", toolPlanMarkdown("ExitPlanMode", """{"plan":"\ud83d\ude80"}"""))
    }
    @Test fun malformedUnicodeEscapeDoesNotReturnCorruptedPlan() {
        assertNull(toolPlanMarkdown("ExitPlanMode", """{"plan":"\u12"}"""))
        assertNull(toolPlanMarkdown("ExitPlanMode", """{"plan":"\uZZZZ"}"""))
    }
    @Test fun incompleteDocumentIsWriting() {
        assertTrue(planTextParts("<proposed_plan>\n# Draft").single().writing)
    }
    @Test fun codeExamplesAndOrdinaryProseAreNotPlans() {
        assertFalse(planTextParts("```xml\n<proposed_plan>\nexample\n</proposed_plan>\n```").single().plan)
        assertFalse(planTextParts("# A normal response").single().plan)
    }
    @Test fun planToolBreaksToolGroup() {
        val blocks = transcriptBlocks(listOf(
            MessageContent.Tool(ToolCallUi("a", "Read")),
            MessageContent.Tool(ToolCallUi("plan", "ExitPlanMode", planMarkdown = "# Plan")),
            MessageContent.Tool(ToolCallUi("b", "Read")),
        ))
        assertEquals(3, blocks.size)
        assertEquals("# Plan", (blocks[1] as TranscriptBlock.Plan).markdown)
    }
    @Test fun heartbeatPreservesPlanDocument() {
        val run = AgentToolRunState("plan", "ExitPlanMode", planMarkdown = "# Plan")
        assertEquals("# Plan", run.toToolCall().planMarkdown)
    }
}
