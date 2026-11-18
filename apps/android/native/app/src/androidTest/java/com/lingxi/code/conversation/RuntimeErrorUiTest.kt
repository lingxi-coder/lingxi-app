package com.lingxi.code.conversation

import android.content.ClipboardManager
import android.graphics.Bitmap
import android.graphics.Color
import android.util.Base64
import android.view.WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.unit.Density
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.components.UiTags
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.SessionRef
import com.lingxi.code.testing.createComponentComposeRule
import com.lingxi.code.theme.LingXiTheme
import java.io.ByteArrayOutputStream
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class RuntimeErrorUiTest {
    @get:Rule val rule = createComponentComposeRule()

    @Test fun rejectedSendNotificationCanBeDismissedWithoutMovingOrClearingComposer() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val text = "Keep this draft and its image"
        val draft = mutableStateOf(text)
        val bitmap = Bitmap.createBitmap(72, 48, Bitmap.Config.ARGB_8888).apply { eraseColor(Color.CYAN) }
        val imageBytes = ByteArrayOutputStream().apply { bitmap.compress(Bitmap.CompressFormat.PNG, 100, this) }
        val image = ComposerAttachment(
            thumb = bitmap.asImageBitmap(),
            width = bitmap.width,
            height = bitmap.height,
            mediaType = "image/png",
            base64 = Base64.encodeToString(imageBytes.toByteArray(), Base64.NO_WRAP),
        )
        val attachment = mutableStateOf<ComposerAttachment?>(image)
        rule.runOnUiThread {
            rule.activity.enableEdgeToEdge()
            rule.activity.window.setSoftInputMode(SOFT_INPUT_ADJUST_RESIZE)
        }
        rule.setContent {
            val chat = viewModel {
                ChatViewModel(source = object : ConversationSource {
                    override val turnAdmissionReady = false
                })
            }
            val state by chat.state.collectAsState()
            LingXiTheme(darkTheme = false) {
                ChatScreen(
                    state = state,
                    modifier = Modifier.systemBarsPadding().imePadding(),
                    onSend = { error("Rejected prompt was sent") },
                    onSendPreflight = { prompt, photo -> chat.admitComposerSend(prompt, photo != null) },
                    draft = draft.value,
                    onDraftChange = { draft.value = it },
                    attachment = attachment.value,
                    onRemoveAttachment = { attachment.value = null },
                    onNewChat = {},
                    onSelectModel = {},
                    isDark = false,
                    onDismissError = chat::dismissError,
                )
            }
        }

        val input = rule.onNode(hasSetTextAction())
        val thumbnail = rule.onNodeWithContentDescription(context.getString(R.string.chat_preview_attached_image))
        val notification = rule.onNodeWithTag(UiTags.CHAT_ERROR)
        input.assertTextContains(text)
        thumbnail.assertIsDisplayed()
        notification.assertDoesNotExist()
        val editorBounds = input.fetchSemanticsNode().boundsInWindow
        val thumbnailBounds = thumbnail.fetchSemanticsNode().boundsInWindow
        snapshot("rejected-send-before")

        rule.onNodeWithTag(UiTags.COMPOSER_SEND).performClick()
        notification.assertIsDisplayed()
        assertTrue(notification.fetchSemanticsNode().boundsInWindow.bottom < editorBounds.top)
        assertEquals(editorBounds, input.fetchSemanticsNode().boundsInWindow)
        assertEquals(thumbnailBounds, thumbnail.fetchSemanticsNode().boundsInWindow)
        input.assertTextContains(text)
        thumbnail.assertIsDisplayed()
        snapshot("rejected-send-notification")

        rule.onNodeWithTag(UiTags.CHAT_ERROR_DISMISS).performClick()
        notification.assertDoesNotExist()
        assertEquals(editorBounds, input.fetchSemanticsNode().boundsInWindow)
        assertEquals(thumbnailBounds, thumbnail.fetchSemanticsNode().boundsInWindow)
        input.assertTextContains(text)
        thumbnail.assertIsDisplayed()
        rule.runOnIdle { assertSame(image, attachment.value) }
        snapshot("rejected-send-notification-dismissed")
    }

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
            rule.runOnIdle { dark.value = night; error.value = null }
            val inputBeforeError = rule.onNode(hasSetTextAction()).fetchSemanticsNode().boundsInRoot
            rule.runOnIdle { error.value = ChatError(diagnostic) }
            rule.onNodeWithText(diagnostic).assertDoesNotExist()
            rule.onNodeWithTag(UiTags.CHAT_ERROR).assertIsDisplayed()
            val banner = rule.onNodeWithTag(UiTags.CHAT_ERROR).fetchSemanticsNode().boundsInRoot
            val input = rule.onNode(hasSetTextAction()).fetchSemanticsNode().boundsInRoot
            assertTrue("Error summary must stay above the editor", banner.bottom <= input.top)
            assertEquals("A notification must not shift the editor", inputBeforeError, input)
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
