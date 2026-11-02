package com.lingxi.code.settings

import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.ProviderKind
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ProviderSettingsSemanticsTest {

    @Test
    fun modelSetup_isRequiredWithoutAnEnabledProviderModel() {
        assertTrue(SettingsUiState().needsLlmSetup)
        assertTrue(
            SettingsUiState(
                llmProviders = listOf(provider().copy(enabled = false)),
            ).needsLlmSetup,
        )
        assertTrue(
            SettingsUiState(
                llmProviders = listOf(provider().copy(model = "")),
            ).needsLlmSetup,
        )
    }

    @Test
    fun selectedProviderModel_withoutCredential_stillRequiresSetup() {
        val state = SettingsUiState(
            llmProviders = listOf(
                provider().copy(
                    credentialConfigured = false,
                    status = ConnStatus.Idle,
                ),
            ),
        )

        assertTrue(state.needsLlmSetup)
    }

    @Test
    fun providerCredentialSummaryWithoutHostProjection_stillRequiresSetup() {
        val state = SettingsUiState(
            llmProviders = listOf(
                provider().copy(
                    credentialConfigured = true,
                    status = ConnStatus.Configured,
                ),
            ),
        )

        assertTrue(state.needsLlmSetup)
    }

    @Test fun hostOAuthProjectionSatisfiesSetupWithoutMatchingApiProviderCredentialSummary() {
        val row = ProviderAccountModelAvailability("openai", "openai-chatgpt", "oauth-model", listOf("oauth-account"),
            emptyList(), listOf("oauth-account"), emptyList())
        val state = SettingsUiState(llmProviders = listOf(provider().copy(credentialConfigured = false)), accountModelAvailability = listOf(row))
        assertFalse(state.needsLlmSetup)
        assertTrue(state.copy(accountModelAvailability = listOf(row.copy(eligibleAccountIds = emptyList()))).needsLlmSetup)
    }

    @Test
    fun acceptedReconnect_appliesAllSavedLaunchChangesTogether() {
        val store = SettingsStore()
        val first = store.addProvider(ProviderKind.Llm, "deepseek")
        val second = store.addProvider(ProviderKind.Llm, "openai")
        store.updateProvider(ProviderKind.Llm, first) { it.copy(enabled = false) }
        store.updateProvider(ProviderKind.Llm, second) { it.copy(model = "gpt-4.1") }
        assertTrue(store.state.value.pendingLlmProviderChanges.containsAll(listOf(first, second)))

        var reconnects = 0
        assertTrue(store.applyLlmConfiguration { reconnects += 1; true })

        assertEquals(1, reconnects)
        assertTrue(store.state.value.pendingLlmProviderChanges.isEmpty())
    }

    @Test
    fun onlyLaunchAffectingFieldsRequireEngineReconnect() {
        val original = provider()

        assertFalse(
            providerLaunchConfigurationChanged(
                original,
                original.copy(name = "Work OpenAI"),
            ),
        )
        assertTrue(
            providerLaunchConfigurationChanged(
                original,
                original.copy(url = "https://gateway.example/v1"),
            ),
        )
        assertTrue(
            providerLaunchConfigurationChanged(
                original,
                original.copy(model = "gpt-4.1"),
            ),
        )
        assertTrue(
            providerLaunchConfigurationChanged(
                original,
                original.copy(enabled = false),
            ),
        )
        assertFalse(
            providerLaunchConfigurationChanged(
                original,
                original.copy(showInModelPicker = false),
            ),
        )
        assertFalse(
            providerLaunchConfigurationChanged(
                original,
                original.copy(visibleModelIds = listOf("gpt-4o-mini")),
            ),
        )
    }

    @Test
    fun credentialPresenceChangesDoNotRebuildProviderRouting() {
        val unconfigured = provider().copy(credentialConfigured = false)
        val configured = unconfigured.copy(credentialConfigured = true)

        assertFalse(ProviderSettingsRepository.buildEngineLaunchConfig(listOf(unconfigured)).defaultModel.isEmpty())
        assertFalse(ProviderSettingsRepository.buildEngineLaunchConfig(listOf(configured)).defaultModel.isEmpty())
        assertEquals(
            ProviderSettingsRepository.buildEngineLaunchConfig(listOf(unconfigured)),
            ProviderSettingsRepository.buildEngineLaunchConfig(listOf(configured)),
        )
        assertFalse(providerLaunchConfigurationChanged(unconfigured, configured))
        assertFalse(providerLaunchConfigurationChanged(configured, unconfigured))
    }

    @Test
    fun removingProviderConfigurationDoesNotRequireOrDeleteHostCredentials() {
        val store = SettingsStore()
        val id = store.addProvider(ProviderKind.Llm, "openai")
        store.updateProvider(ProviderKind.Llm, id) { it.copy(credentialConfigured = true) }
        var result: String? = "not completed"

        store.removeProvider(ProviderKind.Llm, id) { result = it }

        assertEquals(null, result)
        assertTrue(store.state.value.llmProviders.none { it.id == id })
        assertTrue(id in store.state.value.pendingLlmProviderChanges)
    }

    @Test
    fun removingOneProviderConfigurationPreservesAnotherConfigurationOfTheSameProvider() {
        val store = SettingsStore()
        val first = store.addProvider(ProviderKind.Llm, "openai")
        val second = store.addProvider(ProviderKind.Llm, "openai")
        store.updateProvider(ProviderKind.Llm, second) { it.copy(credentialConfigured = true) }

        store.removeProvider(ProviderKind.Llm, first)

        assertTrue(store.state.value.llmProviders.single { it.id == second }.credentialConfigured)
    }

    private fun provider() = GenericProvider(
        id = "l_openai",
        preset = "openai",
        name = "OpenAI",
        url = "https://api.openai.com/v1",
        key = "",
        model = "gpt-4o",
        status = ConnStatus.Idle,
        enabled = true,
    )
}
