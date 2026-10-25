package com.lingxi.code.conversation

import android.content.ClipboardManager
import android.graphics.Bitmap
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.unit.Density
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.components.UiTags
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.SessionRef
import com.lingxi.code.testing.createComponentComposeRule
import com.lingxi.code.theme.LingXiTheme
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class RuntimeErrorUiTest {
    @get:Rule val rule = createComponentComposeRule()

    @Test fun detailsCopyAndDismissPreserveTheDiagnosticInBothAppearances() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val diagnostic = context.getString(R.string.chat_resume_session_failed) +
            ": Rejected(message: \"resume: malformed session id\")\n" +
            "The original diagnostic remains available in the details sheet."
        val error = mutableStateOf<ChatError?>(ChatError(diagnostic))
        val dark = mutableStateOf(false)
        rule.setContent {
            val density = LocalDensity.current
            CompositionLocalProvider(LocalDensity provides Density(density.density, if (dark.value) 2f else 1f)) {
                LingXiTheme(darkTheme = dark.value) {
                    ChatScreen(
                        state = ChatState(SessionRef("error-ui", "Error preview"), emptyList(),
                            isNew = true, model = EngineModelCatalog.pending, error = error.value),
                        onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = dark.value,
                        onDismissError = { error.value = null },
                    )
                }
            }
        }
        for (night in listOf(false, true)) {
            rule.runOnIdle { dark.value = night; error.value = ChatError(diagnostic) }
            rule.onNodeWithText(diagnostic).assertDoesNotExist()
            rule.onNodeWithTag(UiTags.CHAT_ERROR).assertIsDisplayed()
            val banner = rule.onNodeWithTag(UiTags.CHAT_ERROR).fetchSemanticsNode().boundsInRoot
            val input = rule.onNode(hasSetTextAction()).fetchSemanticsNode().boundsInRoot
            assertTrue("Error summary must stay above the editor", banner.bottom <= input.top)
            val model = rule.onNodeWithTag(UiTags.MODEL_PICKER_CHIP).fetchSemanticsNode().boundsInRoot
            val mic = rule.onNodeWithContentDescription(context.getString(R.string.composer_record))
                .fetchSemanticsNode().boundsInRoot
            assertEquals("Model belongs beside the microphone", model.right, mic.left, 1f)
            snapshot("error-${if (night) "dark-large-text" else "light"}")
            rule.onNodeWithTag("conversation.runtime-error.details").performClick()
            rule.onNodeWithTag("conversation.runtime-error.message").assertIsDisplayed().assertTextEquals(diagnostic)
            snapshot("details-${if (night) "dark-large-text" else "light"}")
            rule.onNodeWithTag("conversation.runtime-error.copy").performClick()
            rule.runOnIdle {
                val clipboard = context.getSystemService(ClipboardManager::class.java)
                assertEquals(diagnostic, clipboard.primaryClip?.getItemAt(0)?.text?.toString())
            }
            rule.onNodeWithTag("conversation.runtime-error.close-details").performClick()
            rule.onNodeWithTag("conversation.runtime-error.message").assertDoesNotExist()
            rule.onNodeWithTag(UiTags.CHAT_ERROR).assertIsDisplayed()
            rule.onNodeWithTag(UiTags.CHAT_ERROR_DISMISS).performClick()
            rule.onNodeWithTag(UiTags.CHAT_ERROR).assertDoesNotExist()
        }
    }

    private fun snapshot(name: String) {
        rule.waitForIdle()
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val directory = requireNotNull(instrumentation.targetContext.getExternalFilesDir("composer-style"))
        directory.mkdirs()
        File(directory, "$name.png").outputStream().use {
            requireNotNull(instrumentation.uiAutomation.takeScreenshot()).compress(Bitmap.CompressFormat.PNG, 100, it)
        }
    }
}
