package com.lingxi.code.conversation

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.util.Base64
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.theme.LingXiTheme
import java.io.ByteArrayOutputStream
import java.io.File
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ImagePreviewUiTest {
    @get:Rule val rule = createComposeRule()
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext

    private fun openImage() {
        val bitmap = Bitmap.createBitmap(780, 1688, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        canvas.drawColor(Color.rgb(242, 242, 247))
        val paint = Paint(Paint.ANTI_ALIAS_FLAG)
        for (index in 0..7) {
            paint.color = Color.WHITE
            val top = 60f + index * 195
            canvas.drawRoundRect(30f, top, 750f, top + 165f, 26f, 26f, paint)
            paint.color = Color.BLACK; paint.textSize = 32f
            canvas.drawText("Image preview · ${index + 1}", 64f, top + 96f, paint)
        }
        val bytes = ByteArrayOutputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it); it.toByteArray() }
        val image = ImageRefDto("image/png", Base64.encodeToString(bytes, Base64.NO_WRAP))
        rule.setContent { LingXiTheme { MessageBubble(Message(role = Role.User, text = "Image preview fixture", images = listOf(image))) } }
        rule.onNodeWithContentDescription(context.getString(R.string.chat_attached_image_fmt, 1)).performClick()
        rule.waitUntil(5_000) { rule.onAllNodesWithTag("image-preview.fit").fetchSemanticsNodes().any { node ->
            node.config.getOrNull(SemanticsProperties.Text)?.any { it.text.contains('%') } == true
        } }
    }

    private fun percent(): Int = rule.onNodeWithTag("image-preview.fit").fetchSemanticsNode().config[SemanticsProperties.Text].joinToString { it.text }.removeSuffix("%").toInt()

    @Test fun messageImageOpensZoomsPansAndCloses() {
        openImage()
        val initial = percent()
        rule.onNodeWithTag("image-preview.zoom-in").performClick()
        assertTrue(percent() > initial)
        rule.onNodeWithTag("image-preview.zoom-out").performClick()
        rule.onNodeWithTag("image-preview.fit").performClick()
        rule.onNodeWithTag("image-preview.image").performTouchInput { doubleClick(center) }
        assertTrue(percent() > initial)
        rule.onNodeWithTag("image-preview.image").performTouchInput { swipeUp() }
        rule.onNodeWithTag("image-preview.close").assertIsDisplayed()
        rule.onNodeWithTag("image-preview.fit").performClick()
        rule.onAllNodes(isDialog()).onLast().captureToImage().asAndroidBitmap().let { screenshot ->
            File(context.getExternalFilesDir(null), "image-preview-android.png").outputStream().use { screenshot.compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
        rule.onNodeWithTag("image-preview.close").performClick()
        rule.onNodeWithTag("image-preview").assertDoesNotExist()
    }

    @Test fun oversizedWorkspaceImageRendersAndFitsAccessibly() {
        val file = File(context.cacheDir, "large-preview.png")
        InstrumentationRegistry.getInstrumentation().context.assets.open("image-preview-large.png").use { input -> file.outputStream().use { input.copyTo(it) } }
        try {
            rule.setContent { LingXiTheme { ImagePreviewDialog(ImagePreviewSource.LocalFile(file, "image/png"), {}) } }
            rule.waitUntil(10_000) { rule.onAllNodesWithTag("image-preview.image").fetchSemanticsNodes().isNotEmpty() }
            rule.onNodeWithTag("image-preview.image").captureToImage()
            rule.onNodeWithTag("image-preview.fit").assertContentDescriptionEquals(context.getString(R.string.image_preview_fit))
            assertTrue(percent() in 1..99)
            rule.onNodeWithTag("image-preview.zoom-in").performClick()
            rule.onNodeWithTag("image-preview.image").captureToImage()
        } finally { file.delete() }
    }

    @Test fun systemBackDismissesOnlyThePreview() {
        openImage()
        pressBack()
        rule.onNodeWithTag("image-preview").assertDoesNotExist()
        rule.onNodeWithContentDescription(context.getString(R.string.chat_attached_image_fmt, 1)).assertIsDisplayed()
    }

    @Test fun saveUsesTheSystemDocumentPickerAndCanBeCancelled() {
        openImage()
        rule.onNodeWithTag("image-preview.save").performClick()
        rule.waitUntil(5_000) {
            InstrumentationRegistry.getInstrumentation().uiAutomation.rootInActiveWindow?.packageName?.toString()?.contains("documentsui") == true
        }
        pressBack()
        rule.onNodeWithTag("image-preview.close").assertIsDisplayed()
        rule.onNodeWithTag("image-preview.close").performClick()
    }
    private fun pressBack() {
        android.os.ParcelFileDescriptor.AutoCloseInputStream(
            InstrumentationRegistry.getInstrumentation().uiAutomation.executeShellCommand("input keyevent 4")
        ).use { it.readBytes() }
        rule.waitForIdle()
    }
}
