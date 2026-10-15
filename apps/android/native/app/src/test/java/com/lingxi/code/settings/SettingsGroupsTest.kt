package com.lingxi.code.settings

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SettingsGroupsTest {

    private val all = settingsGroupEntries(computerUseAvailable = true)

    @Test
    fun `the groups keep the designed order and each lists pages`() {
        assertEquals(
            listOf("models", "voice", "modes", "phone", "general", "data", "about"),
            SettingsGroup.entries.map { it.key },
        )
        SettingsGroup.entries.forEach { group ->
            assertTrue("${group.key} is empty", all.any { it.group == group })
        }
    }

    @Test
    fun `models and voice open their page directly, the rest list pages`() {
        assertEquals("settings/providers/Llm", SettingsGroup.Models.directRoute)
        assertEquals(SettingsRoutes.VOICE, SettingsGroup.Voice.directRoute)
        SettingsGroup.entries
            .filter { it != SettingsGroup.Models && it != SettingsGroup.Voice }
            .forEach { assertNull("${it.key} should list pages", it.directRoute) }
    }

    @Test
    fun `voice is no longer a models page`() {
        val voice = all.single { it.route == SettingsRoutes.VOICE }
        assertEquals(SettingsGroup.Voice, voice.group)
    }

    @Test
    fun `a page lives in exactly one group`() {
        val routes = all.map { it.route }
        assertEquals(routes.size, routes.toSet().size)
    }

    @Test
    fun `every page reachable from the old compact home is still reachable`() {
        // The desktop list used to be the home; only the General hub is retired.
        val reachable = all.map { it.route }.toSet() + SettingsRoutes.ACCOUNT
        val missing = desktopSettingsEntries.map { it.route }
            .filter { it != SettingsRoutes.GENERAL && it !in reachable }
        assertEquals(emptyList<String>(), missing)
    }

    @Test
    fun `scheduled tasks and placeholder pages are not settings pages`() {
        val routes = all.map { it.route }
        assertFalse(routes.any { SettingsRoutes.isStandalone(it) })
        assertFalse(SettingsRoutes.SKILLS in routes)
        assertFalse(SettingsRoutes.FUSION in routes)
        assertFalse(SettingsRoutes.DREAM in routes)
    }

    @Test
    fun `computer use is listed only where the device offers it`() {
        assertTrue(SettingsRoutes.COMPUTER_USE in settingsGroupEntries(true).map { it.route })
        assertFalse(SettingsRoutes.COMPUTER_USE in settingsGroupEntries(false).map { it.route })
    }

    private fun search(query: String) = searchSettingsEntries(
        entries = all,
        query = query,
        labelOf = { "L:" + it.title },
        groupLabelOf = { "G:" + it.key },
    )

    @Test
    fun `a blank query keeps every page`() {
        assertEquals(all, search(""))
        assertEquals(all, search("   "))
    }

    @Test
    fun `search matches titles, search keys and the group name`() {
        assertEquals(listOf(SettingsRoutes.CUSTOM_PROVIDERS), search(" routing ").map { it.route })
        assertEquals(listOf(SettingsRoutes.APPEARANCE), search("dark").map { it.route })
        assertEquals(listOf(SettingsRoutes.DIAGNOSTICS), search("settings.json").map { it.route })
        assertEquals(
            all.filter { it.group == SettingsGroup.General },
            searchSettingsEntries(all, "通用", { it.title }, { if (it == SettingsGroup.General) "通用" else "-" }),
        )
        assertTrue(search("no-such-setting").isEmpty())
    }

    @Test
    fun `search also matches the localized label`() {
        val hits = searchSettingsEntries(
            entries = all,
            query = "外观",
            labelOf = { if (it.route == SettingsRoutes.APPEARANCE) "外观" else it.title },
            groupLabelOf = { "-" },
        )
        assertEquals(listOf(SettingsRoutes.APPEARANCE), hits.map { it.route })
    }

    @Test
    fun `group routes round trip and unknown keys resolve to nothing`() {
        SettingsGroup.entries.forEach { group ->
            assertEquals("settings/group/${group.key}", SettingsRoutes.group(group.key))
            assertEquals(group, SettingsGroup.fromKey(group.key))
        }
        assertNull(SettingsGroup.fromKey("missing"))
        assertNull(SettingsGroup.fromKey(null))
    }

    @Test
    fun `only the scheduled task routes are standalone pages`() {
        assertTrue(SettingsRoutes.isStandalone(SettingsRoutes.CRON))
        assertTrue(SettingsRoutes.isStandalone("settings/cron/task/abc"))
        assertTrue(SettingsRoutes.isStandalone("settings/cron/run/abc"))
        assertTrue(SettingsRoutes.isStandalone("settings/cron/new/abc"))
        assertFalse(SettingsRoutes.isStandalone(SettingsRoutes.MAIN))
        assertFalse(SettingsRoutes.isStandalone(SettingsRoutes.APPEARANCE))
        assertFalse(SettingsRoutes.isStandalone("settings/cronjobs"))
    }

    @Test
    fun `a blank prompt opens the task list instead of an empty path segment`() {
        assertEquals(SettingsRoutes.CRON, SettingsRoutes.cronNew(""))
        assertEquals(SettingsRoutes.CRON, SettingsRoutes.cronNew("  \n"))
    }
}
