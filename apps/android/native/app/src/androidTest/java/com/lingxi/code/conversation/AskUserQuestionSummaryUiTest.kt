package com.lingxi.code.conversation

import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.theme.LingXiTheme
import android.graphics.Bitmap
import java.io.File
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class AskUserQuestionSummaryUiTest {
    @get:Rule val rule = createComposeRule()

    @Test fun selectionSubmitsOriginalQuestionKey() {
        val question = "同时纳入原生实时对话？"
        val answer = "同时纳入"
        var submitted: Map<String, String>? = null
        val request = com.lingxi.code.bindings.client.AskUserQuestionRequestDto(
            requestId = 42uL,
            questions = listOf(com.lingxi.code.bindings.client.AskQuestionDto(
                question, "范围", listOf(com.lingxi.code.bindings.client.AskOptionDto(answer, "双向音频与打断", null)), false)),
            timeoutSecs = null)
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                AskUserQuestionCard(request, { _, answers -> submitted = answers }, {})
            }
        }
        rule.onNodeWithText(answer).performClick()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        rule.onNodeWithText(context.getString(com.lingxi.code.R.string.chat_question_submit)).performClick()
        rule.runOnIdle { org.junit.Assert.assertEquals(mapOf(question to answer), submitted) }
    }

    @Test fun lightDarkDisclosurePreservesWrappedAnswer() {
        val dark = mutableStateOf(false)
        val question = "这次统一语音服务，要不要同时纳入 provider 原生实时语音对话（双向音频、打断、会话管理）？"
        val answer = "同时纳入原生实时对话"
        val state = mutableStateOf(ChatState(
            session = com.lingxi.code.model.SessionRef("ask-ui", "Question UI"),
            model = com.lingxi.code.model.EngineModelCatalog.pending,
            messages = listOf(com.lingxi.code.model.Message(com.lingxi.code.model.Role.Ai, "", id = "ask-message",
                blocks = listOf(MessageContent.Tool(ToolCallUi(id = "ask", tool = "AskUserQuestion", status = AgentToolStatus.Completed,
                    questionAnswers = listOf(AnsweredQuestion(question, answer))))))),
        ))
        rule.setContent {
            LingXiTheme(darkTheme = dark.value) {
                ChatScreen(state = state.value, onSend = {}, onNewChat = {}, onSelectModel = {},
                    isDark = dark.value, onToggleTheme = {}, onToggleToolCall = { id ->
                        val expanded = state.value.expandedToolCalls
                        state.value = state.value.copy(expandedToolCalls = if (id in expanded) expanded - id else expanded + id)
                    })
            }
        }
        for (mode in listOf(false, true)) {
            rule.runOnIdle { dark.value = mode }
            rule.onNodeWithText(question).assertIsDisplayed()
            rule.onNodeWithText(answer).assertIsDisplayed()
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val image = File(context.getExternalFilesDir(null), "ask-question-${if (mode) "dark" else "light"}.png")
            image.outputStream().use { rule.onRoot().captureToImage().asAndroidBitmap().compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val title = context.resources.getQuantityString(com.lingxi.code.R.plurals.chat_asked_questions, 1, 1)
        rule.onNodeWithText(title).performClick()
        rule.onNodeWithText(answer).assertDoesNotExist()
        rule.onNodeWithText(title).performClick()
        rule.onNodeWithText(answer).assertIsDisplayed()
    }
}
