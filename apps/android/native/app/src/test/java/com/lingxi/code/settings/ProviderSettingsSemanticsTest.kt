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
    fun successfulDraftTest_doesNotAdvertiseAnUnsavedProviderAsConnected() {
        val unsaved = provider().copy(credentialConfigured = false)
        val tested = providerAfterConnectionTest(unsaved, true, false)
        assertEquals(ConnStatus.Idle, tested.status)
        assertTrue(SettingsUiState(llmProviders = listOf(tested)).needsLlmSetup)
    }

    @Test
    fun successfulDraftTest_doesNotVerifyAnExistingStoredKey() {
        val saved = provider().copy(credentialConfigured = true)
        assertEquals(ConnStatus.Configured, providerAfterConnectionTest(saved, true, false).status)
        assertEquals(ConnStatus.Configured, providerAfterConnectionTest(saved, false, false).status)
    }

    @Test
    fun onlySuccessfulStoredKeyTest_marksProviderConnected() {
        val tested = providerAfterConnectionTest(provider(), true, true)
        assertEquals(ConnStatus.Connected, tested.status)
        assertFalse(SettingsUiState(llmProviders = listOf(tested)).needsLlmSetup)
        assertFalse(ProviderSettingsRepository.buildEngineLaunchConfig(listOf(tested)).defaultModel.isEmpty())
        assertFalse(providerLaunchConfigurationChanged(provider(), tested))
        assertEquals(ConnStatus.Error, providerAfterConnectionTest(provider(), false, true).status)
    }

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
    fun partialCredentialListingPreservesUnavailablePresenceAndAppliesHealthyAbsence() {
        val openai = provider().copy(credentialConfigured = true)
        val custom = provider().copy(id = "custom", url = "https://custom.example/v1", credentialConfigured = true)
        val refreshed = ProviderSettingsRepository.applyCredentialSnapshot(
            listOf(openai, custom),
            ProviderCredentialSnapshot(emptySet(), setOf("openai"), true, "one credential is unavailable"),
        )

        assertTrue(refreshed[0].credentialConfigured)
        assertEquals(ConnStatus.Error, refreshed[0].status)
        assertFalse(refreshed[1].credentialConfigured)
        assertEquals(ConnStatus.Idle, refreshed[1].status)
    }

    @Test
    fun transportFailureCannotErasePreviouslyConfirmedCredentialPresence() {
        val refreshed = ProviderSettingsRepository.applyCredentialSnapshot(
            listOf(provider().copy(credentialConfigured = true)),
            ProviderCredentialSnapshot(emptySet(), emptySet(), false, "engine unavailable"),
        )
        assertTrue(refreshed.single().credentialConfigured)
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
    fun rejectedReconnect_keepsSavedProviderChangesAvailableToRetry() {
        val store = SettingsStore()
        val id = store.addProvider(ProviderKind.Llm, "deepseek")
        store.updateProvider(ProviderKind.Llm, id) { it.copy(model = "deepseek-flash") }
        val pending = store.state.value.pendingLlmProviderChanges
        assertTrue(pending.contains(id))

        assertFalse(store.applyLlmConfiguration { false })
        assertEquals(pending, store.state.value.pendingLlmProviderChanges)
        assertTrue(providerApplyUiState(hasPendingConfiguration = true, hasCredentialDraft = false, busy = false).enabled)

        assertTrue(store.applyLlmConfiguration { true })
        assertTrue(store.state.value.pendingLlmProviderChanges.isEmpty())
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
    fun replacingSavedCredential_preservesRoutingWithoutRequiringEngineReconnect() {
        val store = SettingsStore()
        val id = store.addProvider(ProviderKind.Llm, "anthropic")
        val snapshot = ProviderCredentialSnapshot(
            configuredProviderIds = setOf("anthropic"),
            unavailableProviderIds = emptySet(),
            storageEncrypted = true,
        )
        store.recordSavedProviderCredential(ProviderKind.Llm, id, snapshot)
        assertTrue(store.state.value.llmProviders.single { it.id == id }.credentialConfigured)
        assertTrue(store.applyLlmConfiguration { true })

        store.recordSavedProviderCredential(ProviderKind.Llm, id, snapshot)

        assertTrue(store.state.value.llmProviders.single { it.id == id }.credentialConfigured)
        assertTrue(store.state.value.pendingLlmProviderChanges.isEmpty())
    }

    @Test
    fun sharedCredentialSaveAndDeleteUpdateAllProviderRowsWithoutChangingRouting() {
        val store = SettingsStore()
        val first = store.addProvider(ProviderKind.Llm, "deepseek")
        val second = store.addProvider(ProviderKind.Llm, "deepseek")
        store.updateProvider(ProviderKind.Llm, first) { it.copy(model = "deepseek-flash") }
        store.updateProvider(ProviderKind.Llm, second) { it.copy(model = "deepseek-flash") }
        val configured = ProviderCredentialSnapshot(setOf("deepseek"), emptySet(), true)
        store.applyLlmConfiguration { true }

        store.recordSavedProviderCredential(ProviderKind.Llm, first, configured)
        assertTrue(store.state.value.llmProviders.all { it.credentialConfigured })
        assertFalse(store.state.value.needsLlmSetup)
        assertTrue(store.state.value.pendingLlmProviderChanges.isEmpty())

        store.recordSavedProviderCredential(ProviderKind.Llm, second, configured.copy(configuredProviderIds = emptySet()))
        assertTrue(store.state.value.llmProviders.none { it.credentialConfigured })
        assertTrue(store.state.value.needsLlmSetup)
        assertTrue(store.state.value.pendingLlmProviderChanges.isEmpty())
    }

    @Test
    fun lateStoredKeyProbeCannotResurrectCredentialDeletedThroughAnotherProviderRow() {
        val store = SettingsStore()
        val first = store.addProvider(ProviderKind.Llm, "deepseek")
        val second = store.addProvider(ProviderKind.Llm, "deepseek")
        val configured = ProviderCredentialSnapshot(setOf("deepseek"), emptySet(), true)
        store.recordSavedProviderCredential(ProviderKind.Llm, first, configured)
        val attempt = store.beginProviderConnectionTest(store.state.value.llmProviders.first { it.id == first })

        store.recordSavedProviderCredential(ProviderKind.Llm, second, configured.copy(configuredProviderIds = emptySet()))
        val oldSuccess = ProviderConnectionTestResult(true, true, true, true, 200, 10, "verified", true)
        assertFalse(store.recordProviderConnectionTest(attempt, oldSuccess))
        assertTrue(store.state.value.llmProviders.none { it.credentialConfigured })
        assertTrue(store.state.value.needsLlmSetup)
    }

    @Test
    fun probeFromPreviousEngineBindingCannotConfigureReplacementEngine() {
        val store = SettingsStore()
        val id = store.addProvider(ProviderKind.Llm, "deepseek")
        val attempt = store.beginProviderConnectionTest(store.state.value.llmProviders.single { it.id == id })
        store.bindProviderCredentialSource(null)

        assertFalse(store.recordProviderConnectionTest(attempt, ProviderConnectionTestResult(true, true, true, true, 200, 10, "verified", true)))
        assertFalse(store.state.value.llmProviders.single { it.id == id }.credentialConfigured)
    }

    @Test
    fun genericCredentialMutationInvalidatesProbesWithoutChangingRouting() {
        val store = SettingsStore()
        val deepseek = store.addProvider(ProviderKind.Llm, "deepseek")
        val openai = store.addProvider(ProviderKind.Llm, "openai")
        val attempts = store.state.value.llmProviders.map(store::beginProviderConnectionTest)
        store.applyLlmConfiguration { true }

        store.refreshAfterProviderCredentialMutation()

        val oldSuccess = ProviderConnectionTestResult(true, true, true, true, 200, 10, "verified", true)
        attempts.forEach { assertFalse(store.recordProviderConnectionTest(it, oldSuccess)) }
        assertEquals(setOf(deepseek, openai), store.state.value.llmProviders.map { it.id }.toSet())
        assertTrue(store.state.value.llmProviders.none { it.credentialConfigured })
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
    fun connectionEditsRequireApplyingButDoNotReuseConfirmationFromAnotherCredentialId() {
        val store = SettingsStore()
        val id = store.addProvider(ProviderKind.Llm, "deepseek")
        store.recordSavedProviderCredential(ProviderKind.Llm, id, ProviderCredentialSnapshot(setOf("deepseek"), emptySet(), true))
        store.applyLlmConfiguration { true }
        val before = store.state.value.llmProviders.single { it.id == id }

        store.updateProvider(ProviderKind.Llm, id) { ProviderConnections.withAddedConnection(it) }

        val after = store.state.value.llmProviders.single { it.id == id }
        assertFalse(after.credentialConfigured)
        assertEquals(ConnStatus.Idle, after.status)
        assertTrue(providerLaunchConfigurationChanged(before, after))
        assertTrue(id in store.state.value.pendingLlmProviderChanges)
    }

    @Test
    fun changingCredentialIdentityWhileSavingKeepsConfirmationOnItsOriginalCredential() {
        val store = SettingsStore()
        val id = store.addProvider(ProviderKind.Llm, "deepseek")
        val other = store.addProvider(ProviderKind.Llm, "deepseek")
        store.updateProvider(ProviderKind.Llm, id) { it.copy(url = "https://custom.example/v1") }

        store.recordSavedProviderCredential(ProviderKind.Llm, id, ProviderCredentialSnapshot(setOf("deepseek"), emptySet(), true), "deepseek")

        assertFalse(store.state.value.llmProviders.single { it.id == id }.credentialConfigured)
        assertTrue(store.state.value.llmProviders.single { it.id == other }.credentialConfigured)
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
