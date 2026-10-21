package com.lingxi.code.conversation

import android.graphics.Bitmap
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Surface
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.components.UiTags
import com.lingxi.code.model.*
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.AudioSetupGuidance
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class ChatChromeUiTest {
    @get:Rule val rule = createComposeRule()
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext
    private fun snapshot(name: String) {
        rule.waitForIdle()
        File(context.getExternalFilesDir("chat-chrome-audit"), "$name.png").apply {
            parentFile?.mkdirs()
            outputStream().use { InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot().compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }
    private fun state() = ChatState(SessionRef("new", "新对话"), emptyList(), isNew = true, model = EngineModelCatalog.pending)

    @Test fun phoneUsesOneActionRowAndAttachmentWorks() {
        var camera = 0; var permission = 0
        val draft = mutableStateOf("")
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                Surface { ChatScreen(state = state(), onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false, draft = draft.value, onDraftChange = { draft.value = it },
                    onCameraClick = { camera++ }, onOpenPermissionSettings = { permission++ }, modelSetupRequired = true) }
            }
        }
        val attach = rule.onNodeWithTag(UiTags.COMPOSER_CAMERA).fetchSemanticsNode().boundsInRoot
        val permissionBounds = rule.onNodeWithTag("composer.permission").fetchSemanticsNode().boundsInRoot
        val model = rule.onNodeWithTag(UiTags.MODEL_SETUP_CHIP).fetchSemanticsNode().boundsInRoot
        assertTrue(kotlin.math.abs(attach.center.y - permissionBounds.center.y) < 2f)
        assertTrue(kotlin.math.abs(attach.center.y - model.center.y) < 12f)
        assertTrue(attach.right <= permissionBounds.left + 1f)
        rule.onNodeWithTag(UiTags.COMPOSER_CAMERA).performClick()
        rule.onNodeWithTag("composer.permission").performClick()
        assertEquals(1, camera); assertEquals(1, permission)
        snapshot("phone-compact")
        rule.onNode(hasSetTextAction()).performClick().performTextInput("继续输入")
        rule.onNodeWithTag(UiTags.COMPOSER_SEND).assertIsDisplayed()
        snapshot("phone-keyboard")
    }

    @Test fun moreCategoriesOpenRealSessionDataAndEmptyStates() {
        val call = ToolCallUi("tool-ui", "Read", status = AgentToolStatus.Completed,
            display = ToolResultDisplayUi(headline = "README result", body = "Tool body from engine", bodyLines = 1))
        val data = state().copy(messages = listOf(Message(Role.Ai, "", blocks = listOf(MessageContent.Tool(call)))),
            planTasks = listOf(PlanTaskUi(subject = "Review the real project", state = PlanTaskStateUi.InProgress)))
        rule.setContent { LingXiTheme(false) { Surface { ChatScreen(state = data, onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false) } } }
        for (category in ConversationSummaryCategory.entries) {
            rule.onNodeWithTag("conversation.more").performClick()
            rule.onNodeWithTag("conversation.summary.${category.name}").performClick()
            rule.onNodeWithTag("conversation.summary-sheet.${category.name}").assertIsDisplayed()
            when (category) {
                ConversationSummaryCategory.Tools -> rule.onNodeWithText("README result", substring = true).assertIsDisplayed()
                ConversationSummaryCategory.Plan -> rule.onNode(hasText("Review the real project") and hasAnyAncestor(hasTestTag("conversation.summary-sheet.Plan"))).assertIsDisplayed()
                else -> rule.onNodeWithText(context.getString(category.empty)).assertIsDisplayed()
            }
            snapshot("more-${category.name}")
            rule.onNodeWithTag("conversation.summary.close").performClick()
            rule.waitForIdle()
        }
    }

    @Test fun guidanceRemainsReadableAndActionsWorkInBothThemes() {
        val dark = mutableStateOf(false); var settings = 0; var computer = 0; var dismiss = 0
        rule.setContent {
            LingXiTheme(dark.value) {
                Surface { ChatScreen(state = state(), onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = dark.value, modelSetupRequired = true,
                    computerUseSetup = ComputerUseSetupStatus(false, false, false), audioGuidance = AudioSetupGuidance.Configuration,
                    onOpenModelSettings = { settings++ }, onOpenComputerUseSettings = { computer++ },
                    onDismissComputerUseSetup = { dismiss++ }) }
            }
        }
        for (night in listOf(false, true)) {
            rule.runOnIdle { dark.value = night }
            rule.onNodeWithTag(UiTags.COMPUTER_USE_SETUP_ACTION).assertIsDisplayed().performClick()
            rule.onNodeWithTag(UiTags.MODEL_SETUP_ACTION).assertIsDisplayed().performClick()
            rule.onNodeWithTag(UiTags.COMPUTER_USE_SETUP_DISMISS).performClick()
            snapshot("guidance-${if (night) "dark" else "light"}")
        }
        assertEquals(2, settings); assertEquals(2, computer); assertEquals(2, dismiss)
    }

    @Test fun largeTextUsesAdaptiveRowsWithoutLosingControls() {
        rule.setContent {
            LingXiTheme(false) {
                val density = LocalDensity.current
                CompositionLocalProvider(LocalDensity provides Density(density.density, 2f)) {
                    Surface { Composer("", {}, EngineModelCatalog.pending, {}, {},
                        onCameraClick = {}, modelSetupRequired = true) }
                }
            }
        }
        rule.onNodeWithTag(UiTags.COMPOSER_CAMERA).assertIsDisplayed()
        rule.onNodeWithTag("composer.permission").assertIsDisplayed()
        rule.onNodeWithTag(UiTags.MODEL_SETUP_CHIP).assertIsDisplayed()
        snapshot("composer-large-text")
    }
}
