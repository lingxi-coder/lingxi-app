package com.lingxi.code.conversation

import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.isDialog
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.onLast
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.model.ModelMetadata
import com.lingxi.code.components.UiTags
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.ModelProviderStatus
import com.lingxi.code.theme.LingXiTheme
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ModelPickerUiTest {

    @get:Rule
    val rule = createComposeRule()

    private val models = EngineModelCatalog.options(
        listOf(
            "deepseek/deepseek-flash",
            "anthropic/claude-sonnet-5",
        ),
    ).map { model ->
        if (model.providerId == "deepseek") model.copy(metadata = ModelMetadata(
            thinking = "Thinking", capabilities = "Tools", contextWindow = "1M context",
        )) else model
    }

    private val statuses = listOf(
        ModelProviderStatus(
            profileId = "deepseek",
            settingsId = "p_dsk",
            name = "DeepSeek",
            status = ConnStatus.Error,
            enabled = true,
            credentialConfigured = true,
        ),
        ModelProviderStatus(
            profileId = "anthropic",
            settingsId = "p_ant",
            name = "Anthropic",
            status = ConnStatus.Connected,
            enabled = true,
            credentialConfigured = true,
        ),
    )

    @Test
    fun pickerShowsConnectionStatus_andSearchFiltersRows() {
        rule.setContent {
            LingXiTheme(darkTheme = true) {
                var selected by remember { mutableStateOf(models.last()) }
                Composer(
                    text = "",
                    onTextChange = {},
                    model = selected,
                    onModelChange = { selected = it },
                    onSend = {},
                    availableModels = models,
                    modelProviderStatuses = statuses,
                )
            }
        }

        rule.onNodeWithTag(UiTags.MODEL_PICKER_CHIP).performClick()
        rule.onNodeWithTag(UiTags.MODEL_PICKER_SEARCH).assertIsDisplayed()
        rule.onNodeWithText("连接失败").assertIsDisplayed()
        rule.onNodeWithText("已连接").assertIsDisplayed()
        rule.onNodeWithText("Thinking · Tools · 1M context").assertDoesNotExist()

        rule.onNodeWithTag(UiTags.MODEL_PICKER_SEARCH).performTextInput("sonnet")
        rule.onAllNodes(
            hasText("Claude Sonnet 5", substring = true) and
                hasContentDescription(InstrumentationRegistry.getInstrumentation().targetContext.getString(R.string.composer_current_model)),
        ).onFirst().assertIsDisplayed()
        rule.onNodeWithText("DeepSeek Flash").assertDoesNotExist()
        capture("picker-dark")
    }

    @Test
    fun reasoningChoicesAreDisclosedOnDemand() {
        val selected = AtomicReference<String?>(null)
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                var reasoning by remember { mutableStateOf(com.lingxi.code.model.ReasoningControl(
                    selection = "automatic", levels = listOf("low", "high"), editable = true, disabledReason = null,
                )) }
                Composer(
                    text = "", onTextChange = {}, model = models.last(), onModelChange = {}, onSend = {},
                    availableModels = models, modelProviderStatuses = statuses,
                    reasoning = reasoning,
                    onReasoningChange = { selected.set(it); reasoning = reasoning.copy(selection = it) },
                )
            }
        }
        rule.onNodeWithTag(UiTags.MODEL_PICKER_CHIP).performClick()
        rule.onNodeWithTag(UiTags.COMPOSER_EFFORT).assertDoesNotExist()
        rule.onNodeWithTag("composer.model.options").performClick()
        rule.onNodeWithTag(UiTags.COMPOSER_EFFORT).assertIsDisplayed()
        rule.onNodeWithText(InstrumentationRegistry.getInstrumentation().targetContext.getString(R.string.composer_effort_high)).performClick()
        rule.runOnIdle { assertEquals("high", selected.get()) }
        capture("reasoning-light")
    }

    @Test
    fun failedProviderOpensItsEditor_whileConnectedProviderSelects() {
        val openedProvider = AtomicReference<String?>(null)
        val selectedModel = AtomicReference<String?>(null)
        rule.setContent {
            LingXiTheme(darkTheme = true) {
                Composer(
                    text = "",
                    onTextChange = {},
                    model = models.last(),
                    onModelChange = { selectedModel.set(it.id) },
                    onSend = {},
                    availableModels = models,
                    modelProviderStatuses = statuses,
                    onOpenProviderSettings = { openedProvider.set(it) },
                )
            }
        }

        rule.onNodeWithTag(UiTags.MODEL_PICKER_CHIP).performClick()
        rule.onNodeWithText("DeepSeek Flash").performClick()
        assertEquals("p_dsk", openedProvider.get())
        assertEquals(null, selectedModel.get())

        rule.onNodeWithTag(UiTags.MODEL_PICKER_CHIP).performClick()
        rule.onAllNodes(
            hasText("Claude Sonnet 5", substring = true) and
                hasContentDescription(InstrumentationRegistry.getInstrumentation().targetContext.getString(R.string.composer_current_model)),
        ).onFirst().performClick()
        assertEquals("anthropic/claude-sonnet-5", selectedModel.get())
    }
    private fun capture(name: String) {
        rule.mainClock.advanceTimeBy(500)
        rule.waitForIdle()
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val file = java.io.File(instrumentation.targetContext.getExternalFilesDir("model-ui"), "$name.png")
        val screenshotNode = if (rule.onAllNodes(isDialog()).fetchSemanticsNodes().isNotEmpty()) {
            rule.onNode(isDialog())
        } else rule.onAllNodes(isRoot()).onLast()
        screenshotNode.captureToImage().asAndroidBitmap().let { bitmap ->
            file.outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
            bitmap.recycle()
        }
    }

}
