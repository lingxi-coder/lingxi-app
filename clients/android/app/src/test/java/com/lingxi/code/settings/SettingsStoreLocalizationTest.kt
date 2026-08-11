package com.lingxi.code.settings

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Everything [SettingsStore] seeds into [SettingsUiState] is user-facing prose,
 * so every seeded string has to leave through the injected resolver — the one
 * hook that turns a resource id into the DEVICE's language.
 *
 * The skills roster regressed exactly here: it was swapped for a resolver-less
 * roster whose rows carried literal Simplified Chinese, and `SkillsPages`
 * renders `name`/`desc` verbatim, so Settings → Skills read Chinese on an
 * English / Japanese / Korean / zh-TW device.
 */
class SettingsStoreLocalizationTest {

    @Test
    fun seededSkillsAreResolvedThroughTheInjectedStringResolver() {
        val store = SettingsStore(resolveString = { id -> "localized-$id" })

        val skills = store.state.value.skills
        assertEquals(5, skills.size)
        assertTrue(
            "every bundled skill description must be localized, not a literal: " +
                skills.map { it.desc },
            skills.all { it.desc.startsWith("localized-") },
        )
    }

    @Test
    fun seededMcpServersStayResolvedToo() {
        // The same resolver, still wired — the skills regression must not have
        // been a symptom of it being dropped everywhere.
        val store = SettingsStore(resolveString = { id -> "localized-$id" })
        assertTrue(store.state.value.mcpServers.any { it.name.startsWith("localized-") })
    }

    @Test
    fun anAbsentResolverFallsBackToTheSeedCopyInsteadOfEmptyText() {
        // Every JVM test constructs the store with no Context at all; a blank
        // description would be a worse failure than the untranslated seed.
        val store = SettingsStore()
        assertTrue(store.state.value.skills.all { it.desc.isNotBlank() })
    }
}
