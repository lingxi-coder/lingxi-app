package com.lingxi.code.settings

import com.lingxi.code.model.ConnStatus
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
    fun engineLaunchConfig_emitsOnlyEnabledProfilesInMobileAllowlist() {
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

        val allowlist = ProviderSettingsRepository.mobileEnabledProfileNames(
            listOf(enabled, disabled),
        )

        assertTrue("enabled builtin is retained", "openai" in allowlist)
        assertFalse("disabled builtin is excluded", "deepseek" in allowlist)
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
    )
}
