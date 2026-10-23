package com.lingxi.code.conversation

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.compose.ui.test.*
import com.lingxi.code.testing.createComponentComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.bindings.client.TaskStatusDto
import com.lingxi.code.components.UiTags
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.SessionRef
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class IosChatParityUiTest {
    @get:Rule val rule = createComponentComposeRule()

    @Test fun contextUsageDoesNotSplitTheInputToolbar() {
        rule.setContent {
            LingXiTheme(false) {
                Box(Modifier.width(360.dp)) {
                    Composer(text = "Send this", onTextChange = {}, model = EngineModelCatalog.pending,
                        onModelChange = {}, onSend = {}, contextUsage = AgentRunUsage(100, 0, 0, 0))
                }
            }
        }
        assertSingleToolbarRow()
    }

    @Test fun largeTextUsesIconsWithoutScrollingTheToolbar() {
        rule.setContent {
            LingXiTheme(false) {
                CompositionLocalProvider(LocalDensity provides Density(LocalDensity.current.density, 2f)) {
                    Box(Modifier.width(320.dp)) {
                        Composer(text = "Send this", onTextChange = {}, model = EngineModelCatalog.pending,
                            onModelChange = {}, onSend = {})
                    }
                }
            }
        }
        assertSingleToolbarRow()
        rule.onNodeWithTag(UiTags.MODEL_PICKER_CHIP).assertIsDisplayed()
        rule.onNodeWithTag(UiTags.COMPOSER_SEND).assertIsDisplayed()
    }

    private fun assertSingleToolbarRow() {
        val send = rule.onNodeWithTag(UiTags.COMPOSER_SEND).assertIsDisplayed().fetchSemanticsNode().boundsInRoot
        for (tag in listOf(UiTags.COMPOSER_CAMERA, "composer.permission", UiTags.MODEL_PICKER_CHIP)) {
            val bounds = rule.onNodeWithTag(tag).fetchSemanticsNode().boundsInRoot
            assertEquals("$tag must stay on the send button row", send.center.y, bounds.center.y, 1f)
        }
    }

    @Test fun executionMovesToSummaryWithoutLosingResumeOrPlan() {
        var resumed: String? = null
        val task = BackgroundTaskUi("paused-task", "workflow", "Resume this workflow",
            TaskStatusDto.PAUSED, true, null)
        val state = ChatState(
            session = SessionRef("parity", "Chat parity"), messages = emptyList(),
            model = EngineModelCatalog.pending,
            backgroundTasks = mapOf(task.taskId to task),
            planTasks = listOf(PlanTaskUi(subject = "Keep the plan accessible", state = PlanTaskStateUi.InProgress)),
        )
        rule.setContent {
            LingXiTheme(false) {
                ChatScreen(state = state, onSend = {}, onNewChat = {}, onSelectModel = {},
                    isDark = false, onResumeWorkflow = { resumed = it })
            }
        }
        rule.onNodeWithTag(UiTags.EXECUTION_STATUS_PANEL).assertDoesNotExist()
        rule.onNodeWithTag("conversation.more").performClick()
        rule.onNodeWithTag("conversation.summary.Agents").performClick()
        rule.onNodeWithText(task.description).assertIsDisplayed()
        val resume = InstrumentationRegistry.getInstrumentation().targetContext.getString(R.string.chat_workflow_resume)
        rule.onNodeWithContentDescription(resume).performClick()
        rule.runOnIdle { assertEquals(task.taskId, resumed) }
        rule.onNodeWithTag("conversation.summary.close").performClick()
        rule.onNodeWithTag("conversation.more").performClick()
        rule.onNodeWithTag("conversation.summary.Plan").performClick()
        rule.onNodeWithText("Keep the plan accessible").assertIsDisplayed()
    }
}
