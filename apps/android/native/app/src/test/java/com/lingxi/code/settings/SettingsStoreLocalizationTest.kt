package com.lingxi.code.settings

import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Everything [SettingsStore] seeds into [SettingsUiState] is user-facing prose,
 * so every seeded string has to leave through the injected resolver — the one
 * hook that turns a resource id into the DEVICE's language.
 */
class SettingsStoreLocalizationTest {

    @Test
    fun seededMcpServersStayResolvedToo() {
        // The same resolver, still wired.
        val store = SettingsStore(resolveString = { id -> "localized-$id" })
        assertTrue(store.state.value.mcpServers.any { it.name.startsWith("localized-") })
    }

    @Test
    fun anAbsentResolverFallsBackToTheSeedCopyInsteadOfEmptyText() {
        // Every JVM test constructs the store with no Context at all; a blank
        // name would be a worse failure than the untranslated seed.
        val store = SettingsStore()
        assertTrue(store.state.value.mcpServers.all { it.name.isNotBlank() })
    }
}
