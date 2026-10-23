package com.lingxi.code.conversation

import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.test.*
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
import com.lingxi.code.testing.createComponentComposeRule
import com.lingxi.code.theme.LingXiTheme
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class CompactTranscriptUiTest {
    @get:Rule val rule = createComponentComposeRule()

    @Test fun lightAppearanceKeepsCompactSpacingAndIconOnlyCopy() = verify(dark = false)
    @Test fun darkAppearanceKeepsCompactSpacingAndIconOnlyCopy() = verify(dark = true)

    private fun verify(dark: Boolean) {
        val interrupted = "[Request interrupted by user]"
        val state = ChatState(
            session = SessionRef("compact-transcript", "Conversation spacing"),
            model = EngineModelCatalog.pending,
            messages = listOf(
                Message(Role.User, "# Inline visualization\n\n" +
                    "A compact animation with a penguin riding a bicycle. ".repeat(16),
                    id = "long-prompt", sentAtMs = 1_792_706_220_000),
                Message(Role.User, interrupted, id = "interrupted", sentAtMs = 1_792_706_340_000),
            ),
        )
        rule.setContent {
            LingXiTheme(darkTheme = dark) {
                ChatScreen(state = state, onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = dark)
            }
        }
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val copyLabel = context.getString(R.string.chat_message_copy)
        val copiedLabel = context.getString(R.string.chat_message_copied)
        val buttons = rule.onAllNodesWithTag("conversation.message.user.copy").assertCountEquals(2)
        val copy = buttons[1].assertIsDisplayed().assertContentDescriptionEquals(copyLabel)
        rule.onAllNodesWithText(copyLabel).assertCountEquals(0)
        val previousFooter = buttons[0].getUnclippedBoundsInRoot()
        val nextText = rule.onNodeWithText(interrupted).getUnclippedBoundsInRoot()
        assertEquals(16f, nextText.top.value - 12f - previousFooter.bottom.value, 0.5f)
        val copyBounds = copy.getUnclippedBoundsInRoot()
        assertEquals(28f, copyBounds.bottom.value - copyBounds.top.value, 0.5f)
        val appearance = if (dark) "dark" else "light"
        capture("compact-transcript-$appearance")

        // Exercise the expanded touch target above the compact visual bounds.
        copy.performTouchInput { click(Offset(center.x, -height * 0.2f)) }
        copy.assertContentDescriptionEquals(copiedLabel)
        rule.onAllNodesWithText(copiedLabel).assertCountEquals(0)
        rule.runOnIdle {
            val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            assertEquals(interrupted, clipboard.primaryClip?.getItemAt(0)?.text?.toString())
        }
        capture("compact-transcript-copied-$appearance")
    }

    private fun capture(name: String) {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val directory = requireNotNull(context.getExternalFilesDir("compact-transcript-screenshots"))
        directory.mkdirs()
        File(directory, "$name.png").outputStream().use {
            rule.onRoot().captureToImage().asAndroidBitmap().compress(Bitmap.CompressFormat.PNG, 100, it)
        }
    }
}
