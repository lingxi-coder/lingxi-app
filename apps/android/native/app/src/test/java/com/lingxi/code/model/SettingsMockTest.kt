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
        val skills = SettingsMock.skills()
        assertEquals(6, skills.size)
        assertEquals(listOf("sk1", "sk2", "sk3", "sk4", "sk5", "sk6"), skills.map { it.id })
        // 4 enabled (sk1, sk2, sk3, sk6); 2 disabled (sk4, sk5).
        assertEquals(4, skills.count { it.enabled })
        // 3 builtin (官方), 3 community/mine.
        assertEquals(3, skills.count { it.builtin })
        assertTrue("every skill has >= 1 trigger", skills.all { it.triggers.isNotEmpty() })
    }

    @Test
    fun bundledSkills_areTheEightLocalAppSkills() {
        val skills = SettingsMock.bundledSkills()
        assertEquals(8, skills.size)
        assertEquals(
            listOf(
                "create-local-app",
                "frontend-design",
                "ionic-react-local-app",
                "canvas-2d-local-app",
                "threejs-local-app",
                "frontend-qa",
                "accessibility",
                "react-best-practices",
            ),
            skills.map { it.id },
        )
        assertTrue(skills.all { it.builtin && it.enabled && it.author == "官方" })
        assertTrue(skills.all { it.triggers == listOf("/${it.id}") })
    }

    @Test
    fun bundledSkills_resolveEveryDescriptionThroughTheStringCatalog() {
        // `SkillsPages` renders `desc` VERBATIM. A resolver-less roster meant an
        // English / Japanese / Korean / zh-TW device read Simplified Chinese in
        // Settings → Skills, because no `settings_skill_*` key was ever consulted.
        val requested = mutableListOf<Int>()
        val skills = SettingsMock.bundledSkills { id, _ ->
            requested += id
            "localized-$id"
        }
        assertEquals(
            "every bundled description must go through a string resource",
            skills.size,
            requested.size,
        )
        assertEquals(skills.size, requested.toSet().size)
        assertTrue(skills.all { it.desc == "localized-${requested[skills.indexOf(it)]}" })
        assertTrue("no description may be a hardcoded literal", skills.none { it.desc.isBlank() })

        // The NAME stays the canonical slug — it is the identifier the
        // `/create-local-app` trigger and the engine's registry both use.
        assertTrue(skills.all { it.name == it.id })
    }

    // --- MCP servers ------------------------------------------------------

    @Test
    fun mcpServers_count_ids_connectionsAndStatuses() {
        val mcp = SettingsMock.mcpServers()
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
        assertEquals(1, Presets.voice.size)
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
            listOf("deepseek-flash", "deepseek-v4-pro"),
            deepSeek.models,
        )
    }

    @Test
    fun kimiPreset_usesOfficialEndpointAndCuratedModels() {
        val kimi = Presets.llm.single { it.id == "kimi" }

        assertEquals("https://api.moonshot.cn/v1", kimi.defaultUrl)
        assertEquals(
            listOf("kimi-k3"),
            kimi.models,
        )
    }

    @Test
    fun kimiCodePreset_usesDistinctMembershipEndpointAndModels() {
        val kimiCode = Presets.llm.single { it.id == "kimi-code" }

        assertEquals("https://api.kimi.com/coding/v1", kimiCode.defaultUrl)
        assertEquals(
            listOf("k3"),
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
    fun notifConfig_enabledCount_countsTheFourTypes() {
        // Every type defaults ON: these are local notifications on the device
        // the user is holding, not a remote push that costs a round trip.
        assertEquals(4, NotifConfig().enabledCount)
        assertEquals(
            3,
            NotifConfig(taskCompleteNotifEnabled = false).enabledCount,
        )
    }

    @Test
    fun notifConfig_masterSwitchZeroesTheCount_whateverTheTypesSay() {
        // The rule `enabledCount` actually encodes: with the master switch off
        // the per-type toggles are unreachable, so reporting "4 enabled" next
        // to a disabled feature would be a lie the settings row would render.
        assertEquals(0, NotifConfig(enabled = false).enabledCount)
        assertEquals(
            0,
            NotifConfig(
                enabled = false,
                idlePromptNotifEnabled = true,
                inputNeededNotifEnabled = true,
                taskCompleteNotifEnabled = true,
                scheduledRunNotifEnabled = true,
            ).enabledCount,
        )
    }

    @Test
    fun notifConfig_defaultIdleThresholdIsUpstreams() {
        // Byte-faithful to Claude Code's
        // `DEFAULT_GLOBAL_CONFIG.messageIdleNotifThresholdMs = 60000`.
        assertEquals(60_000L, NotifConfig().messageIdleNotifThresholdMs)
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
        assertEquals("gpt-5.6-sol", p.model)
        assertEquals(ConnStatus.Idle, p.status)
        assertTrue(p.enabled)
        assertFalse(p.isDefault)
        // Two mints produce different ids (UUID suffix).
        val q = SettingsMock.newProvider(ProviderKind.Llm, "openai")
        assertNotEquals(p.id, q.id)
    }
}
