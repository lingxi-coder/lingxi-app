package com.lingxi.code.settings

import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.LlmProviderCatalogEntry
import com.lingxi.code.model.CatalogModelDetails
import com.lingxi.code.model.ModelMetadata
import com.lingxi.code.model.GenericProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ProviderProfileMappingTest {
    @Test
    fun catalogEndpoint_usesStableBuiltInCredentialId() {
        val provider = provider(
            preset = "openai",
            url = "https://api.openai.com/v1",
        )

        assertEquals("openai", ProviderSettingsRepository.engineCredentialIdFor(provider))
    }

    @Test
    fun kimiCatalogEndpoint_usesStableBuiltInCredentialId() {
        val provider = provider(
            preset = "kimi",
            url = "https://api.moonshot.cn/v1",
        )

        assertEquals("kimi", ProviderSettingsRepository.engineCredentialIdFor(provider))
    }

    @Test
    fun kimiCodeCatalogEndpoint_isSeparateFromOpenPlatform() {
        val provider = provider(
            preset = "kimi-code",
            url = "https://api.kimi.com/coding/v1",
        )

        assertEquals("kimi-code", ProviderSettingsRepository.engineCredentialIdFor(provider))
    }

    @Test
    fun customEndpoint_usesPersistedRowIdSoAccountsDoNotCollide() {
        val provider = provider(
            id = "l_custom_42",
            preset = "openai",
            url = "https://gateway.example/v1",
        )

        assertEquals("l_custom_42", ProviderSettingsRepository.engineCredentialIdFor(provider))
    }

    @Test
    fun qwen_isRoutedAsARealUserProfile() {
        val provider = provider(
            id = "l_qwen",
            preset = "qwen",
            url = "https://dashscope.aliyuncs.com/v1",
        )

        assertEquals("l_qwen", ProviderSettingsRepository.engineCredentialIdFor(provider))
    }

    @Test
    fun engineLaunchConfig_emitsOnlyConfiguredEnabledProfilesInMobileAllowlist() {
        val enabled = provider(
            id = "l_openai",
            preset = "openai",
            url = "https://api.openai.com/v1",
        )
        val disabled = provider(
            id = "l_deepseek",
            preset = "deepseek",
            url = "https://api.deepseek.com",
        ).copy(enabled = false)
        val missingCredential = provider(
            id = "l_anthropic",
            preset = "anthropic",
            url = "https://api.anthropic.com",
        ).copy(credentialConfigured = false)

        val allowlist = ProviderSettingsRepository.mobileEnabledProfileNames(
            listOf(enabled, disabled, missingCredential),
        )

        assertTrue("configured enabled builtin is retained", "openai" in allowlist)
        assertFalse("disabled builtin is excluded", "deepseek" in allowlist)
        assertFalse("provider without a credential is excluded", "anthropic" in allowlist)
    }

    @Test
    fun legacyDeepSeekConfig_migratesToCurrentOfficialEndpointAndModel() {
        val legacyChat = provider(
            preset = "deepseek",
            url = "https://api.deepseek.com/v1/",
        ).copy(model = "deepseek-chat")
        val legacyReasoner = legacyChat.copy(model = "deepseek-reasoner")

        assertEquals(
            legacyChat.copy(
                url = "https://api.deepseek.com",
                model = "deepseek-v4-flash",
            ),
            ProviderSettingsRepository.migrateLegacyDeepSeek(legacyChat),
        )
        assertEquals(
            "deepseek-v4-flash",
            ProviderSettingsRepository.migrateLegacyDeepSeek(legacyReasoner).model,
        )
    }

    @Test
    fun engineLaunchConfig_emptySavedList_emitsExplicitEmptyAllowlist() {
        assertEquals(
            """{"mobileEnabledProfiles":[]}""",
            ProviderSettingsRepository.buildMobileRoutingJson(emptyList()),
        )
    }

    @Test
    fun engineLaunchConfig_enabledProfiles_emitsValidQuotedJsonValues() {
        assertEquals(
            """{"mobileEnabledProfiles":["deepseek","openai"]}""",
            ProviderSettingsRepository.buildMobileRoutingJson(listOf("deepseek", "openai")),
        )
    }

    @Test
    fun engineLaunchConfig_defaultsVisionDelegationEnabled() {
        val config = ProviderSettingsRepository.buildEngineLaunchConfig(
            savedProviders = listOf(
                provider(
                    id = "l_openai",
                    preset = "openai",
                    url = "https://api.openai.com/v1",
                ),
            ),
        )

        assertTrue(config.visionDelegationEnabled)
    }

    @Test
    fun engineLaunchConfig_propagatesDisabledVisionDelegation() {
        val config = ProviderSettingsRepository.buildEngineLaunchConfig(
            savedProviders = listOf(
                provider(
                    id = "l_openai",
                    preset = "openai",
                    url = "https://api.openai.com/v1",
                ),
            ),
            visionDelegationEnabled = false,
        )

        assertFalse(config.visionDelegationEnabled)
    }

    @Test
    fun newProvider_prefersBuiltinCatalogModelOverStalePresetOrder() {
        val provider = ProviderSettingsRepository.newProvider(
            kind = com.lingxi.code.model.ProviderKind.Llm,
            preset = com.lingxi.code.model.Presets.llm.first { it.id == "openai" },
            catalogEntries = listOf(
                LlmProviderCatalogEntry(
                    profileId = "openai",
                    displayName = "OpenAI",
                    baseUrl = "https://api.openai.com/v1",
                    protocol = "responses",
                    auth = "apiKey",
                    credentialEnv = "OPENAI_API_KEY",
                    modelIds = listOf("gpt-5.6-luna"),
                    modelDetails = listOf(
                        CatalogModelDetails(
                            reference = "openai/gpt-5.6-luna",
                            providerId = "openai",
                            providerLabel = "OpenAI",
                            displayName = "GPT-5.6 Luna",
                            modelId = "gpt-5.6-luna",
                            description = null,
                            family = null,
                            status = null,
                            releaseDate = null,
                            lastUpdated = null,
                            knowledgeCutoff = null,
                            inputModalities = emptyList(),
                            outputModalities = emptyList(),
                            contextWindowTokens = null,
                            maxInputTokens = null,
                            maxOutputTokens = null,
                            openWeights = null,
                            attachments = null,
                            temperatureControl = null,
                            pricing = null,
                            capabilities = emptyList(),
                            reasoningOptions = emptyList(),
                            reasoningEditable = true,
                            reasoningForced = false,
                            reasoningDefault = null,
                            metadata = ModelMetadata(displayName = "GPT-5.6 Luna"),
                        ),
                    ),
                ),
            ),
        )

        assertEquals("gpt-5.6-luna", provider.model)
    }

    private fun provider(
        id: String = "l_provider",
        preset: String,
        url: String,
    ) = GenericProvider(
        id = id,
        preset = preset,
        name = preset,
        url = url,
        key = "",
        model = "model",
        status = ConnStatus.Idle,
        enabled = true,
        credentialConfigured = true,
    )
}
