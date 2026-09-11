package com.lingxi.code.conversation

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.test.ext.junit.runners.AndroidJUnit4
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
    )

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
    fun pickerShowsConnectionMetadata_andSearchFiltersRows() {
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
        rule.onNodeWithText("Thinking · 1M 上下文 · 284B / 13B 激活").assertIsDisplayed()

        rule.onNodeWithTag(UiTags.MODEL_PICKER_SEARCH).performTextInput("sonnet")
        rule.onNode(
            hasText("Claude Sonnet 5", substring = true) and
                hasContentDescription("当前模型"),
        ).assertIsDisplayed()
        rule.onNodeWithText("DeepSeek Flash").assertDoesNotExist()
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
        rule.onNode(
            hasText("Claude Sonnet 5", substring = true) and
                hasContentDescription("当前模型"),
        ).performClick()
        assertEquals("anthropic/claude-sonnet-5", selectedModel.get())
    }
}
