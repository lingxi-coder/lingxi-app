package com.lingxi.code.conversation

import android.content.Context
import android.content.ContextWrapper
import android.content.SharedPreferences
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalContext
import java.util.UUID
import org.junit.After

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
import androidx.compose.ui.test.hasSetTextAction
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
import com.lingxi.code.settings.ProviderAccountModelAvailability
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ModelPickerUiTest {

    @get:Rule
    val rule = createComposeRule()

    private val preferencePrefix = "model-picker-test-${UUID.randomUUID()}"
    private val fixtureContext by lazy {
        val target = InstrumentationRegistry.getInstrumentation().targetContext
        object : ContextWrapper(target) {
            override fun getApplicationContext(): Context = this
            override fun getSharedPreferences(name: String, mode: Int): SharedPreferences =
                target.getSharedPreferences("$preferencePrefix-$name", mode)
        }
    }

    private fun setIsolatedContent(content: @Composable () -> Unit) {
        rule.setContent { CompositionLocalProvider(LocalContext provides fixtureContext, content = content) }
    }

    @After fun removeOnlyFixturePreferences() {
        InstrumentationRegistry.getInstrumentation().targetContext
            .deleteSharedPreferences("$preferencePrefix-lingxi_model_recents")
    }

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

    private val availability = listOf(
        ProviderAccountModelAvailability("deepseek", "deepseek", "deepseek-flash", emptyList(), emptyList(), emptyList(), emptyList()),
        ProviderAccountModelAvailability("anthropic", "anthropic", "claude-sonnet-5", listOf("fixture-account"),
            listOf("fixture-account"), emptyList(), emptyList()),
    )

    @Test
    fun pickerShowsConnectionStatus_andSearchFiltersRows() {
        setIsolatedContent {
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
                    accountModelAvailability = availability,
                )
            }
        }

        rule.onNodeWithTag(UiTags.MODEL_PICKER_CHIP).performClick()
        rule.onNodeWithTag(UiTags.MODEL_PICKER_SEARCH).assertIsDisplayed()
        rule.onNodeWithText("连接失败").assertIsDisplayed()
        rule.onNodeWithText(InstrumentationRegistry.getInstrumentation().targetContext.getString(R.string.settings_provider_status_configured)).assertIsDisplayed()
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
        setIsolatedContent {
            LingXiTheme(darkTheme = false) {
                var reasoning by remember { mutableStateOf(com.lingxi.code.model.ReasoningControl(
                    selection = "automatic", levels = listOf("low", "high"), editable = true, disabledReason = null,
                )) }
                Composer(
                    text = "", onTextChange = {}, model = models.last(), onModelChange = {}, onSend = {},
                    availableModels = models, modelProviderStatuses = statuses,
                    accountModelAvailability = availability,
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
        setIsolatedContent {
            LingXiTheme(darkTheme = true) {
                Composer(
                    text = "",
                    onTextChange = {},
                    model = models.last(),
                    onModelChange = { selectedModel.set(it.id) },
                    onSend = {},
                    availableModels = models,
                    modelProviderStatuses = statuses,
                    accountModelAvailability = availability,
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
    @Test fun subscriptionOnlyHostModelRemainsSelectableBesideApiCatalog() {
        val selected = AtomicReference<String?>(null)
        val apiModels = EngineModelCatalog.options(listOf("openai/api-only"))
        val apiStatus = ModelProviderStatus(profileId = "openai", settingsId = "openai", name = "OpenAI",
            status = ConnStatus.Configured, enabled = true, credentialConfigured = false,
            catalogModelIds = listOf("api-only"))
        val projection = listOf(ProviderAccountModelAvailability("openai", "openai-chatgpt", "subscription-only",
            listOf("oauth-a"), emptyList(), listOf("oauth-a"), emptyList()))
        setIsolatedContent {
            LingXiTheme(darkTheme = false) {
                Composer(text = "", onTextChange = {}, model = apiModels.single(), onSend = {},
                    onModelChange = { selected.set(it.id) }, availableModels = apiModels,
                    modelProviderStatuses = listOf(apiStatus), accountModelAvailability = projection)
            }
        }
        rule.onNodeWithTag(UiTags.MODEL_PICKER_CHIP).performClick()
        rule.onNodeWithTag(UiTags.MODEL_PICKER_SEARCH).performTextInput("subscription-only")
        rule.onNode(hasText("subscription-only") and !hasSetTextAction()).assertIsDisplayed().performClick()
        assertEquals("openai-chatgpt/subscription-only", selected.get())
        capture("subscription-model-selected")
    }

    @Test
    fun hostModelProjectionShowsPermissionsAndIncludesAccountOnlyModel() {
        val selectedModel = AtomicReference<String?>(null)
        val availability = listOf(
            ProviderAccountModelAvailability("anthropic", "anthropic", "claude-sonnet-5",
                listOf("account-a"), listOf("account-a"), emptyList(), emptyList()),
            ProviderAccountModelAvailability("anthropic", "anthropic", "account-exclusive-model",
                listOf("account-b"), emptyList(), listOf("account-b"), emptyList()),
            ProviderAccountModelAvailability("deepseek", "deepseek", "deepseek-flash",
                emptyList(), emptyList(), emptyList(), listOf("account-c")),
        )
        setIsolatedContent {
            LingXiTheme(darkTheme = false) {
                Composer(text = "", onTextChange = {}, model = models.last(),
                    onModelChange = { selectedModel.set(it.id) }, onSend = {},
                    availableModels = models,
                    modelProviderStatuses = statuses.map { it.copy(catalogModelIds = listOf("claude-sonnet-5", "deepseek-flash")) },
                    accountModelAvailability = availability)
            }
        }
        rule.onNodeWithTag(UiTags.MODEL_PICKER_CHIP).performClick()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        rule.onAllNodes(hasText(context.getString(R.string.provider_account_model_confirmed))).onFirst().assertIsDisplayed()
        rule.onAllNodes(hasText(context.getString(R.string.provider_account_model_forbidden))).onFirst().assertIsDisplayed()
        rule.onNodeWithTag(UiTags.MODEL_PICKER_SEARCH).performTextInput("account-exclusive")
        rule.onNodeWithText(context.getString(R.string.provider_account_model_unverified)).assertIsDisplayed()
        rule.onNodeWithText("Account Exclusive Model").performClick()
        assertEquals("anthropic/account-exclusive-model", selectedModel.get())
    }

    @Test fun cooldownNoticeKeepsModelPickerAvailableInsteadOfFirstSetupNavigation() {
        val openedSettings = java.util.concurrent.atomic.AtomicBoolean(false)
        setIsolatedContent {
            LingXiTheme(darkTheme = false) {
                Composer(text = "", onTextChange = {}, model = models.last(), onModelChange = {}, onSend = {},
                    availableModels = models, modelProviderStatuses = statuses,
                    accountModelAvailability = availability.map { it.copy(eligibleAccountIds = emptyList()) },
                    modelSetupRequired = true, modelAvailabilityNotice = "Fixture cooldown; availability refreshes automatically",
                    onOpenModelSettings = { openedSettings.set(true) })
            }
        }
        rule.onNodeWithTag(UiTags.MODEL_SETUP_CHIP).assertDoesNotExist()
        rule.onNodeWithTag(UiTags.MODEL_PICKER_CHIP).performClick()
        rule.onNodeWithTag(UiTags.MODEL_PICKER_SEARCH).assertIsDisplayed()
        assertEquals(false, openedSettings.get())
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
