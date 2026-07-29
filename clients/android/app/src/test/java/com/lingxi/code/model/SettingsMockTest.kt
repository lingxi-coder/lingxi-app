package com.lingxi.code.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Integrity checks for the canonical settings mock dataset (`SettingsMock`) and
 * the preset catalogs (`Presets`), pinned against the iOS `SettingsModels.swift`
 * / prototype `SettingsSheet`. The settings main-list counts ("N 个启用",
 * "M / K 启用", "C 连接") are derived from these, so the counts are asserted
 * directly. Pure JVM.
 */
class SettingsMockTest {

    // --- providers (LLM / search / fetch) ---------------------------------

    @Test
    fun llmProviders_count_ids_andEnabledDefault() {
        val llm = SettingsMock.llmProviders
        assertEquals(3, llm.size)
        assertEquals(listOf("p_ant", "p_oai", "p_dsk"), llm.map { it.id })
        // 2 enabled (Anthropic + OpenAI); DeepSeek seeded disabled.
        assertEquals(2, llm.count { it.enabled })
        // Exactly one default — Anthropic.
        assertEquals(1, llm.count { it.isDefault })
        assertEquals("p_ant", llm.first { it.isDefault }.id)
    }

    @Test
    fun searchAndFetchProviders_countsAndDefaults() {
        assertEquals(2, SettingsMock.searchProviders.size)
        assertEquals(1, SettingsMock.searchProviders.count { it.enabled })
        assertEquals(1, SettingsMock.searchProviders.count { it.isDefault })

        assertEquals(1, SettingsMock.fetchProviders.size)
        assertEquals(1, SettingsMock.fetchProviders.count { it.enabled })
        assertTrue(SettingsMock.fetchProviders.first().isDefault)
    }

    // --- skills -----------------------------------------------------------

    @Test
    fun skills_count_ids_enabledAndBuiltinSplit() {
        val skills = SettingsMock.skills
        assertEquals(6, skills.size)
        assertEquals(listOf("sk1", "sk2", "sk3", "sk4", "sk5", "sk6"), skills.map { it.id })
        // 4 enabled (sk1, sk2, sk3, sk6); 2 disabled (sk4, sk5).
        assertEquals(4, skills.count { it.enabled })
        // 3 builtin (官方), 3 community/mine.
        assertEquals(3, skills.count { it.builtin })
        assertTrue("every skill has >= 1 trigger", skills.all { it.triggers.isNotEmpty() })
    }

    // --- MCP servers ------------------------------------------------------

    @Test
    fun mcpServers_count_ids_connectionsAndStatuses() {
        val mcp = SettingsMock.mcpServers
        assertEquals(5, mcp.size)
        assertEquals(listOf("mcp1", "mcp2", "mcp3", "mcp4", "mcp5"), mcp.map { it.id })
        // 4 enabled ("连接" count on the main list); 1 disabled (Notion).
        assertEquals(4, mcp.count { it.enabled })
        // Status spread: 3 Connected, 1 Idle, 1 Error.
        assertEquals(3, mcp.count { it.status == ConnStatus.Connected })
        assertEquals(1, mcp.count { it.status == ConnStatus.Idle })
        assertEquals(1, mcp.count { it.status == ConnStatus.Error })
        // Tool counts are positive.
        assertTrue(mcp.all { it.tools > 0 })
    }

    // --- preset catalogs --------------------------------------------------

    @Test
    fun presetCatalogs_haveExpectedCounts() {
        assertEquals(9, Presets.llm.size)
        assertEquals(5, Presets.search.size)
        assertEquals(4, Presets.fetch.size)
        assertEquals(3, Presets.voice.size)
    }

    @Test
    fun providerKind_mapsToItsPresetCatalog() {
        assertEquals(Presets.llm, ProviderKind.Llm.presets)
        assertEquals(Presets.search, ProviderKind.Search.presets)
        assertEquals(Presets.fetch, ProviderKind.Fetch.presets)
        // id prefixes are distinct (used to mint new provider ids).
        val prefixes = ProviderKind.entries.map { it.idPrefix }
        assertEquals(prefixes.size, prefixes.toSet().size)
    }

    @Test
    fun deepSeekPreset_usesCurrentOfficialEndpointAndModels() {
        val deepSeek = Presets.llm.single { it.id == "deepseek" }

        assertEquals("https://api.deepseek.com", deepSeek.defaultUrl)
        assertEquals(
            listOf("deepseek-v4-flash", "deepseek-v4-pro"),
            deepSeek.models,
        )
    }

    @Test
    fun kimiPreset_usesOfficialEndpointAndCuratedModels() {
        val kimi = Presets.llm.single { it.id == "kimi" }

        assertEquals("https://api.moonshot.cn/v1", kimi.defaultUrl)
        assertEquals(
            listOf("kimi-k3", "kimi-k2.7-code", "kimi-k2.7-code-highspeed", "kimi-k2.6"),
            kimi.models,
        )
    }

    @Test
    fun kimiCodePreset_usesDistinctMembershipEndpointAndModels() {
        val kimiCode = Presets.llm.single { it.id == "kimi-code" }

        assertEquals("https://api.kimi.com/coding/v1", kimiCode.defaultUrl)
        assertEquals(
            listOf("kimi-for-coding", "k3", "k3-256k", "kimi-for-coding-highspeed"),
            kimiCode.models,
        )
    }

    // --- ConnStatus labels ------------------------------------------------

    @Test
    fun connStatus_labels_matchPrototype() {
        assertEquals("已连接", ConnStatus.Connected.label)
        assertEquals("未验证", ConnStatus.Idle.label)
        assertEquals("检测中…", ConnStatus.Testing.label)
        assertEquals("连接失败", ConnStatus.Error.label)
    }

    // --- NotifConfig derived count ----------------------------------------

    @Test
    fun notifConfig_enabledCount_default3() {
        // Defaults: workflows + mentions + crons on, marketing off → 3.
        assertEquals(3, NotifConfig().enabledCount)
        assertEquals(0, NotifConfig(false, false, false, false).enabledCount)
        assertEquals(4, NotifConfig(true, true, true, true).enabledCount)
    }

    // --- newProvider (A7 add flow) ----------------------------------------

    @Test
    fun newProvider_buildsFromPreset_withUniquePrefixedIdAndDefaults() {
        val p = SettingsMock.newProvider(ProviderKind.Llm, "openai")
        assertTrue("id prefixed with kind", p.id.startsWith("l_"))
        assertEquals("openai", p.preset)
        assertEquals("OpenAI", p.name)
        assertEquals("https://api.openai.com/v1", p.url)
        // First preset model is pre-selected; status idle; enabled; not default.
        assertEquals("gpt-4o", p.model)
        assertEquals(ConnStatus.Idle, p.status)
        assertTrue(p.enabled)
        assertFalse(p.isDefault)
        // Two mints produce different ids (UUID suffix).
        val q = SettingsMock.newProvider(ProviderKind.Llm, "openai")
        assertNotEquals(p.id, q.id)
    }
}
