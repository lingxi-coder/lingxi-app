package com.lingxi.code.conversation

import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.components.UiTags
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class AgentActivityUiTest {
    @get:Rule val rule = createComposeRule()

    @Test fun oneActivityHeaderKeepsItsBoundsAcrossThinkingAndToolTransitions() {
        val state = mutableStateOf(AgentRunState(42, reasoning = "Inspect spacing", reasoningActive = true))
        rule.setContent { LingXiTheme(false) { AgentRunTimeline(state.value) } }
        val initial = rule.onNodeWithTag(UiTags.AGENT_RUN_TIMELINE).fetchSemanticsNode().boundsInRoot
        val tools = listOf(AgentToolRunState("read", "Read"), AgentToolRunState("shell", "Bash"))
        for (phase in listOf(
            state.value.copy(reasoningActive = false, tools = tools.take(1)),
            state.value.copy(reasoningActive = false, tools = tools),
            state.value.copy(reasoningActive = true, tools = tools.map { it.copy(status = AgentToolStatus.Completed) }),
            state.value.copy(active = false, outcome = AgentRunOutcome.Completed, tools = tools.map { it.copy(status = AgentToolStatus.Completed) }),
        )) {
            rule.runOnIdle { state.value = phase }
            assertEquals(initial, rule.onNodeWithTag(UiTags.AGENT_RUN_TIMELINE).fetchSemanticsNode().boundsInRoot)
            rule.onAllNodesWithTag("conversation.activity.header").assertCountEquals(1)
        }
        rule.onNodeWithTag("conversation.activity.header").performClick()
        rule.onNodeWithText("Inspect spacing").assertIsDisplayed()
    }
}
