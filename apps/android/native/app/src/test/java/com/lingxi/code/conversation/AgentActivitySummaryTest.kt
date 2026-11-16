package com.lingxi.code.conversation

import org.junit.Assert.*
import org.junit.Test

class AgentActivitySummaryTest {
    @Test fun thinkingAndToolsUseTheSameRunAndKeepAllDetails() {
        val thinking = AgentRunState(turnId = 7, reasoning = "Inspect the layout", reasoningActive = true)
        assertEquals("Inspect the layout", thinking.activitySummaryText())
        val tool = AgentToolRunState(id = "read", tool = "Read")
        val running = thinking.copy(reasoningActive = false, tools = listOf(tool))
        assertEquals("Read", running.activitySummaryText())
        assertEquals(thinking.turnId, running.turnId)
        assertEquals(thinking.reasoning, running.reasoning)
        val finished = running.copy(active = false, outcome = AgentRunOutcome.Completed,
            tools = listOf(tool.copy(status = AgentToolStatus.Completed)))
        assertEquals("Read", finished.activitySummaryText())
        assertEquals(thinking.reasoning, finished.reasoning)
    }

    @Test fun concurrentToolsOwnTheHeaderUntilTheyAllFinish() {
        val state = AgentRunState(9, reasoning = "Next step", reasoningActive = true, tools = listOf(
            AgentToolRunState("a", "Read", status = AgentToolStatus.Completed),
            AgentToolRunState("b", "Bash")))
        assertEquals("Bash", state.activitySummaryText())
        assertEquals("Next step", state.copy(tools = state.tools.map { it.copy(status = AgentToolStatus.Completed) }).activitySummaryText())
    }
}
