package com.lingxi.code.computeruse

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Test

class BrowserSettingsPolicyTest {
    private val browsers = setOf("org.example.browser", "vendor.other.browser")
    private val configuration = ComputerUseConfiguration(
        listenEnabled = true,
        maxListenSeconds = 30,
        appSelections = mapOf("org.example.editor" to ComputerUseTier.Full),
    )

    @Test
    fun browserChoicesShareAppPermissionsAndPreserveOtherAppsAndAudio() {
        val selected = browserAppSelection(
            configuration, ComputerUseSessionState.Inactive, browsers,
            "vendor.other.browser", ComputerUseTier.Click,
        )
        assertEquals(ComputerUseTier.Click, selected.appSelections["vendor.other.browser"])
        assertEquals(ComputerUseTier.Full, selected.appSelections["org.example.editor"])
        assertEquals(true, selected.listenEnabled)
        assertEquals(30, selected.maxListenSeconds)
        val removed = browserAppSelection(
            selected, ComputerUseSessionState.Inactive, browsers, "vendor.other.browser", null,
        )
        assertEquals(configuration, removed)
    }

    @Test
    fun permissionChangesWaitUntilTheOldSessionHasStopped() {
        for (state in ComputerUseSessionState.entries.filter { it != ComputerUseSessionState.Inactive }) {
            assertSame(configuration, browserAppSelection(configuration, state, browsers, "org.example.browser", ComputerUseTier.Full))
        }
        assertThrows(IllegalArgumentException::class.java) {
            browserAppSelection(configuration, ComputerUseSessionState.Inactive, browsers, "unknown.app", ComputerUseTier.Full)
        }
    }

    @Test
    fun savedBrowserPermissionsNeverAppearAsLiveGrants() {
        val selected = configuration.copy(appSelections = mapOf("org.example.browser" to ComputerUseTier.Full))
        assertEquals(ComputerUseTier.Full, browserControlTier(ComputerUseUiState(), selected, "org.example.browser"))
        val active = ComputerUseUiState(sessionState = ComputerUseSessionState.Active)
        assertNull(browserControlTier(active, selected, "org.example.browser"))
        assertEquals(
            ComputerUseTier.Read,
            browserControlTier(active.copy(grants = listOf(ComputerUseGrant("org.example.browser", "Browser", ComputerUseTier.Read))), selected, "org.example.browser"),
        )
    }
}
