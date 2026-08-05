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
    fun enabledProviderModel_withStoredCredential_satisfiesSetup() {
        val state = SettingsUiState(
            llmProviders = listOf(
                provider().copy(
                    credentialConfigured = true,
                    status = ConnStatus.Configured,
                ),
            ),
        )

        assertFalse(state.needsLlmSetup)
    }

    @Test
    fun encryptedCredential_isConfigured_notNetworkConnected() {
        val status = ProviderSettingsRepository.statusFor(
            provider = provider(),
            credentialConfigured = true,
            encrypted = true,
            unavailable = false,
        )

        assertEquals(ConnStatus.Configured, status)
        assertEquals("已配置", status.label)
    }

    @Test
    fun disabledProvider_keepsConfiguredCredentialStatus() {
        val status = ProviderSettingsRepository.statusFor(
            provider = provider().copy(enabled = false),
            credentialConfigured = true,
            encrypted = true,
            unavailable = false,
        )

        assertEquals(ConnStatus.Configured, status)
    }

    @Test
    fun refreshSnapshot_restoresConfiguredCredentialStatus() {
        val refreshed = ProviderSettingsRepository.applyCredentialSnapshot(
            providers = listOf(provider()),
            snapshot = ProviderCredentialSnapshot(
                configuredProviderIds = setOf("openai"),
                unavailableProviderIds = emptySet(),
                storageEncrypted = true,
            ),
        )

        assertTrue(refreshed.single().credentialConfigured)
        assertEquals(ConnStatus.Configured, refreshed.single().status)
    }

    @Test
    fun refreshFailure_isVisibleInsteadOfFallingBackToUnverified() {
        val refreshed = ProviderSettingsRepository.applyCredentialSnapshot(
            providers = listOf(provider()),
            snapshot = ProviderCredentialSnapshot(
                configuredProviderIds = emptySet(),
                unavailableProviderIds = emptySet(),
                storageEncrypted = false,
                error = "credential status timed out",
            ),
        )

        assertFalse(refreshed.single().credentialConfigured)
        assertEquals(ConnStatus.Error, refreshed.single().status)
    }

    @Test
    fun providerApplyState_savesCredentialDraftAlongsideConfiguration() {
        val state = providerApplyUiState(
            hasPendingConfiguration = false,
            hasCredentialDraft = true,
            busy = false,
        )

        assertTrue(state.enabled)
        assertTrue(state.saveCredential)
        assertEquals(com.lingxi.code.R.string.settings_provider_save_and_apply, state.labelRes)
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
    }

    @Test
    fun editingTestedConfiguration_returnsStatusToCredentialBaseline() {
        val store = SettingsStore()
        val id = store.addProvider(ProviderKind.Llm, "deepseek")
        store.updateProvider(ProviderKind.Llm, id) {
            it.copy(
                credentialConfigured = true,
                status = ConnStatus.Connected,
            )
        }

        store.markProviderConnectionUnverified(ProviderKind.Llm, id)

        val updated = store.state.value.llmProviders.single { it.id == id }
        assertEquals(ConnStatus.Configured, updated.status)
        assertTrue(updated.credentialConfigured)
    }

    @Test
    fun editingDraftWithoutSavedCredential_returnsStatusToUnverified() {
        val store = SettingsStore()
        val id = store.addProvider(ProviderKind.Llm, "deepseek")
        store.updateProvider(ProviderKind.Llm, id) {
            it.copy(status = ConnStatus.Error)
        }

        store.markProviderConnectionUnverified(ProviderKind.Llm, id)

        assertEquals(
            ConnStatus.Idle,
            store.state.value.llmProviders.single { it.id == id }.status,
        )
    }

    @Test
    fun removeLlmProvider_deletesCredentialBeforePersistedRow() = runTest {
        val calls = mutableListOf<String>()

        val error = removeProviderCredentialFirst(
            kind = ProviderKind.Llm,
            provider = provider(),
            deleteCredential = {
                calls += "credential"
                ProviderCredentialSnapshot(
                    configuredProviderIds = emptySet(),
                    unavailableProviderIds = emptySet(),
                    storageEncrypted = true,
                )
            },
            removePersistedProvider = { calls += "provider" },
        )

        assertEquals(null, error)
        assertEquals(listOf("credential", "provider"), calls)
    }

    @Test
    fun removeLlmProvider_keepsRowWhenCredentialDeletionFails() = runTest {
        val calls = mutableListOf<String>()

        val error = removeProviderCredentialFirst(
            kind = ProviderKind.Llm,
            provider = provider(),
            deleteCredential = {
                calls += "credential"
                ProviderCredentialSnapshot(
                    configuredProviderIds = setOf("openai"),
                    unavailableProviderIds = emptySet(),
                    storageEncrypted = true,
                    error = "secure storage unavailable",
                )
            },
            removePersistedProvider = { calls += "provider" },
        )

        assertEquals("secure storage unavailable", error)
        assertEquals(listOf("credential"), calls)
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
