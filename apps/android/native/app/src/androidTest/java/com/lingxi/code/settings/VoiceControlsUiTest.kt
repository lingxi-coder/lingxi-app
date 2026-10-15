package com.lingxi.code.settings

import android.graphics.Bitmap
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Surface
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.offline.GeneratedVoiceModelCatalog
import com.lingxi.code.voice.offline.ModelState
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class VoiceControlsUiTest {
    @get:Rule val rule = createComposeRule()

    @Test fun speedAndDownloadStagesRemainVisibleAndUpdateInBothThemes() {
        val dark = mutableStateOf(false)
        val rate = mutableFloatStateOf(1f)
        val download = mutableStateOf<ModelState>(ModelState.Downloading(25L * 1024 * 1024, 100L * 1024 * 1024))
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        rule.setContent {
            LingXiTheme(dark.value) {
                Surface(Modifier.fillMaxSize().safeDrawingPadding()) {
                    Column {
                        VoiceRateControl(rate.floatValue) { rate.floatValue = it }
                        OfflineModelRow(GeneratedVoiceModelCatalog.all.first(), download.value,
                            "Zipformer 中文 14M", "100 MB", true)
                    }
                }
            }
        }
        for (night in listOf(false, true)) {
            rule.runOnIdle { dark.value = night; download.value = ModelState.Downloading(25L * 1024 * 1024, 100L * 1024 * 1024) }
            rule.onNodeWithTag("voice.rate").performSemanticsAction(SemanticsActions.SetProgress) { it(1.5f) }
            rule.runOnIdle { assertEquals(1.5f, rate.floatValue, 0.001f) }
            rule.onNodeWithText("1.5×").assertIsDisplayed()
            rule.onNodeWithTag("voice.rate.decrease").performClick()
            rule.runOnIdle { assertEquals(1.4f, rate.floatValue, 0.001f) }
            rule.onNodeWithTag("voice.rate.increase").performClick()
            rule.runOnIdle { assertEquals(1.5f, rate.floatValue, 0.001f) }
            rule.runOnIdle { rate.floatValue = 0.5f }
            rule.onNodeWithTag("voice.rate.decrease").assertIsNotEnabled()
            rule.onNodeWithTag("voice.rate.increase").assertIsEnabled().performClick()
            rule.runOnIdle { assertEquals(0.6f, rate.floatValue, 0.001f); rate.floatValue = 2f }
            rule.onNodeWithTag("voice.rate.increase").assertIsNotEnabled()
            rule.onNodeWithTag("voice.rate.decrease").assertIsEnabled().performClick()
            rule.runOnIdle { assertEquals(1.9f, rate.floatValue, 0.001f); rate.floatValue = 1.5f }

            rule.onNodeWithText(context.getString(R.string.voice_model_state_downloading) + " 25%").assertIsDisplayed()
            rule.onNode(SemanticsMatcher.expectValue(SemanticsProperties.ProgressBarRangeInfo,
                ProgressBarRangeInfo(0.25f, 0f..1f))).assertIsDisplayed()
            capture("${if (night) "dark" else "light"}-download")
            rule.runOnIdle { download.value = ModelState.Downloading(75L * 1024 * 1024, 100L * 1024 * 1024) }
            rule.onNodeWithText(context.getString(R.string.voice_model_state_downloading) + " 75%").assertIsDisplayed()
            rule.onNode(SemanticsMatcher.expectValue(SemanticsProperties.ProgressBarRangeInfo,
                ProgressBarRangeInfo(0.75f, 0f..1f))).assertIsDisplayed()
            for ((state, label) in listOf(ModelState.Verifying to R.string.voice_model_state_verifying,
                ModelState.Extracting to R.string.voice_model_state_installing)) {
                rule.runOnIdle { download.value = state }
                rule.onNodeWithText(context.getString(label)).assertIsDisplayed()
                rule.onNode(SemanticsMatcher.expectValue(SemanticsProperties.ProgressBarRangeInfo,
                    ProgressBarRangeInfo.Indeterminate)).assertIsDisplayed()
            }
            rule.runOnIdle { download.value = ModelState.Downloading(1024, 0) }
            rule.onNode(SemanticsMatcher.expectValue(SemanticsProperties.ProgressBarRangeInfo,
                ProgressBarRangeInfo.Indeterminate)).assertIsDisplayed()
            rule.runOnIdle { download.value = ModelState.Ready }
            rule.onNodeWithText(context.getString(R.string.voice_model_state_ready)).assertIsDisplayed()
            rule.onNodeWithText(context.getString(R.string.common_delete)).assertIsDisplayed()
        }
    }
    private fun capture(name: String) {
        rule.waitForIdle()
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        File(instrumentation.targetContext.getExternalFilesDir("voice-controls-audit"), "$name.png").apply {
            parentFile?.mkdirs()
            outputStream().use { instrumentation.uiAutomation.takeScreenshot().compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }
}
