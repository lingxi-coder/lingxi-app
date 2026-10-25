package com.lingxi.code.settings

import android.graphics.Bitmap
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Surface
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.unit.dp
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.isDialog
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.onLast
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsOff
import androidx.compose.ui.test.assertIsOn
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTextReplacement
import androidx.compose.ui.unit.Density
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.model.CatalogModelDetails
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.ModelMetadata
import com.lingxi.code.theme.LingXiTheme
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ProviderModelUiTest {
    @get:Rule val rule = createComposeRule()
    private fun text(id: Int) = InstrumentationRegistry.getInstrumentation().targetContext.getString(id)
    private val initial = GenericProvider(
        id = "preview", preset = "custom", name = "Preview", url = "https://example.com", key = "",
        model = "model-0", status = ConnStatus.Configured, enabled = true,
    )

    @Test fun defaultSelectionSearchesLongCatalogWithoutChangingVisibility() {
        var provider = initial
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                var current by remember { mutableStateOf(initial) }
                ProviderModelSheet(current, (0..79).map { details("model-$it") }, true, false,
                    onUpdate = { current = it; provider = it },
                    onSelect = { current = current.copy(model = it); provider = current }, onDismiss = {})
            }
        }
        rule.onNodeWithTag("provider.models.search").performTextInput("model-79")
        rule.onNodeWithTag("provider.models.row.model-79").assertIsDisplayed().performClick()
        rule.runOnIdle { assertEquals("model-79", provider.model); assertNull(provider.visibleModelIds) }
        rule.onNodeWithTag("provider.models.search").performTextReplacement("does-not-exist")
        rule.onNodeWithText(text(R.string.composer_no_matching_models)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.settings_provider_custom_model_id)).performClick()
        rule.onNodeWithTag("provider.models.custom").performTextReplacement("custom-model")
        rule.runOnIdle { assertEquals("custom-model", provider.model) }
    }

    @Test fun visibilityCheckboxesAndProviderSwitchPreserveTheDefaultModel() {
        var provider = initial
        rule.setContent {
            LingXiTheme(darkTheme = true) {
                var current by remember { mutableStateOf(initial) }
                ProviderModelSheet(current, listOf(details("model-0"), details("model-1")), true, true,
                    onUpdate = { current = it; provider = it }, onSelect = {}, onDismiss = {})
            }
        }
        val row = rule.onNodeWithTag("provider.models.row.model-1")
        row.assertIsOn().performClick().assertIsOff()
        rule.runOnIdle { assertEquals(listOf("model-0"), provider.visibleModelIds); assertEquals("model-0", provider.model) }
        rule.onNodeWithTag("provider.models.visible").performClick().assertIsOff()
        row.assertDoesNotExist()
        rule.onNodeWithTag("provider.models.visible").performClick().assertIsOn()
        row.assertIsOff()
        capture("visible-models-dark")
    }

    @Test fun modelSheetSupportsLightDarkAndLargeType() {
        val dark = mutableStateOf(false)
        val large = mutableStateOf(false)
        rule.setContent {
            val density = LocalDensity.current
            CompositionLocalProvider(LocalDensity provides Density(density.density, if (large.value) 2f else 1f)) {
                LingXiTheme(darkTheme = dark.value) {
                    ProviderModelSheet(initial, listOf(details("model-0"), details("model-1")), true, false,
                        onUpdate = {}, onSelect = {}, onDismiss = {})
                }
            }
        }
        for ((isDark, isLarge) in listOf(false to false, true to false, false to true)) {
            rule.runOnIdle { dark.value = isDark; large.value = isLarge }
            rule.onNodeWithTag("provider.models.search").assertIsDisplayed()
            rule.onNodeWithTag("provider.models.row.model-0").assertIsDisplayed()
        }
    }

    @Test fun lightModelSheetUsesNativeRows() { captureModelRows(false, "models-light-standard") }
    @Test fun darkModelSheetUsesNativeRows() { captureModelRows(true, "models-dark-standard") }
    @Test fun largeTypeModelSheetUsesNativeRows() { captureModelRows(false, "models-light-large", 2f) }

    private fun captureModelRows(dark: Boolean, name: String, fontScale: Float = 1f) {
        rule.setContent {
            val density = LocalDensity.current
            CompositionLocalProvider(LocalDensity provides Density(density.density, fontScale)) {
                LingXiTheme(darkTheme = dark) {
                    ProviderModelSheet(initial, listOf(details("model-0"), details("model-1")), true, false,
                        onUpdate = {}, onSelect = {}, onDismiss = {})
                }
            }
        }
        rule.onNodeWithTag("provider.models.row.model-0").assertIsDisplayed()
        capture(name)
    }

    @Test fun connectionEditorKeepsCredentialDraftWhenInspectingModels() {
        val store = SettingsStore()
        val id = store.addProvider(com.lingxi.code.model.ProviderKind.Llm, "custom")
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                val state by store.state.collectAsState()
                Surface {
                    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp)) {
                        ProviderEditPage(com.lingxi.code.model.ProviderKind.Llm, id, state, store, onPop = {})
                    }
                }
            }
        }
        android.util.Log.i("ModelUiProgress", "capture connection")
        capture("connection-light")
        android.util.Log.i("ModelUiProgress", "enter draft")
        rule.onNodeWithTag("provider.api-key").performScrollTo().performTextInput("sk-ui-placeholder")
        android.util.Log.i("ModelUiProgress", "open models")
        rule.onNodeWithTag("provider.models.open").performScrollTo().performClick()
        android.util.Log.i("ModelUiProgress", "verify models")
        rule.onNodeWithTag("provider.models.search").assertIsDisplayed()
        android.util.Log.i("ModelUiProgress", "close sheet")
        rule.onNodeWithText(text(R.string.common_close)).performClick()
        rule.onNodeWithTag("provider.api-key").performScrollTo()
        android.util.Log.i("ModelUiProgress", "show draft")
        rule.onNodeWithText(text(R.string.settings_provider_show_key)).performClick()
        rule.onNodeWithTag("provider.api-key").assertTextContains("sk-ui-placeholder")
        android.util.Log.i("ModelUiProgress", "open advanced")
        rule.onNodeWithTag("provider.advanced").performScrollTo().performClick()
        android.util.Log.i("ModelUiProgress", "verify advanced")
        rule.onNodeWithText(text(R.string.settings_provider_connection_add)).assertIsDisplayed()
        android.util.Log.i("ModelUiProgress", "close sheet")
        rule.onNodeWithText(text(R.string.common_close)).performClick()
        rule.onNodeWithTag("provider.api-key").performScrollTo().assertTextContains("sk-ui-placeholder")
    }

    @Test fun missingCatalogStillOffersCustomModelEntry() {
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                ProviderModelSheet(initial, emptyList(), false, false, onUpdate = {}, onSelect = {}, onDismiss = {})
            }
        }
        rule.onNodeWithTag("provider.models.custom").assertIsDisplayed()
        rule.onNodeWithText(text(R.string.settings_provider_catalog_loading)).assertIsDisplayed()
    }

    private fun capture(name: String) {
        rule.mainClock.advanceTimeBy(500)
        rule.waitForIdle()
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val file = File(instrumentation.targetContext.getExternalFilesDir("model-ui"), "$name.png")
        val screenshotNode = if (rule.onAllNodes(isDialog()).fetchSemanticsNodes().isNotEmpty()) {
            rule.onNode(isDialog())
        } else rule.onAllNodes(isRoot()).onLast()
        screenshotNode.captureToImage().asAndroidBitmap().let { bitmap ->
            file.outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
            bitmap.recycle()
        }
    }

    private fun details(id: String) = CatalogModelDetails(
        reference = "custom/$id", providerId = "custom", providerLabel = "Preview",
        displayName = if (id == "model-0") "Default model" else "Model ${id.substringAfter('-')}", modelId = id,
        description = null, family = null, status = null, releaseDate = null, lastUpdated = null,
        knowledgeCutoff = null, inputModalities = emptyList(), outputModalities = emptyList(),
        contextWindowTokens = null, maxInputTokens = null, maxOutputTokens = null, openWeights = null,
        attachments = null, temperatureControl = null, pricing = null, capabilities = emptyList(),
        reasoningOptions = emptyList(), reasoningEditable = false, reasoningForced = false,
        reasoningDefault = null, metadata = ModelMetadata(modelId = id),
    )
}
