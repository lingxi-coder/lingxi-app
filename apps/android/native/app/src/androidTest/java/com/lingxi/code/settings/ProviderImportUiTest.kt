package com.lingxi.code.settings

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.flow.MutableSharedFlow
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ProviderImportUiTest {
    @get:Rule val rule=createComposeRule()
    private fun label(id:Int)=InstrumentationRegistry.getInstrumentation().targetContext.getString(id)
    private fun showImporter() {
        val bridge=SettingsEngineBridge()
        val source=object: ConversationSource {
            override val clientEvents=MutableSharedFlow<ClientEvent>()
            override suspend fun submitClientCommand(command:ClientCommand) {
                bridge.accept(ClientEvent.SettingsSnapshot("{}","{}","[]",null,emptyList(),"{\"user\":{}}",emptyList()))
            }
        }
        rule.setContent {LingXiTheme {
            LaunchedEffect(Unit) {bridge.bind(source)}
            Column(Modifier.verticalScroll(rememberScrollState())) {ProviderBulkImport(bridge,"user")}
        }}
        rule.onNodeWithText(label(R.string.settings_parity_import_providers)).performClick()
    }
    @Test fun reviewNeverRendersExtractedCredentials() {
        showImporter()
        val sourceJson = """{"providers":{"custom":{"type":"openai","baseUrl":"https://api.example.test","models":["model"],"apiKey":"fixture-hidden"}}}"""
        val parsed = parseProviderImport(sourceJson, org.json.JSONObject()).entries.single()
        org.junit.Assert.assertNotNull("Configuration import must reject embedded credentials", parsed.error)
        rule.onNodeWithText(label(R.string.settings_parity_import_json)).performTextInput(sourceJson)
        rule.onNodeWithText(label(R.string.settings_parity_import_preview)).performScrollTo().performClick()
        rule.onNodeWithText("custom").assertExists()
        rule.onAllNodesWithText("fixture-hidden",substring=true).assertCountEquals(0)
        rule.onNodeWithText(label(R.string.provider_account_import_without_credentials)).performScrollTo().assertIsDisplayed()
    }
    @Test fun environmentAuthenticationIsRejectedByConfigurationImport() {
        val json = """{"providers":{"custom":{"type":"openai","baseUrl":"https://api.example.test","models":["model"],"apiKey":"{env:PROVIDER_API_KEY}"}}}"""
        val parsed = parseProviderImport(json, org.json.JSONObject()).entries.single()
        org.junit.Assert.assertNotNull(parsed.error)
        org.junit.Assert.assertFalse(parsed.definition.toString().contains("PROVIDER_API_KEY"))
    }

    @Test fun invalidInputRemainsEditableAfterValidationFailure() {
        showImporter()
        rule.onNodeWithText(label(R.string.settings_parity_import_json)).performTextInput("{invalid-json")
        rule.onNodeWithText(label(R.string.settings_parity_import_preview)).performScrollTo().performClick()
        rule.onNodeWithText(label(R.string.settings_parity_import_invalid)).assertExists()
        rule.onNode(hasSetTextAction()).assertTextContains("{invalid-json")
    }
}
