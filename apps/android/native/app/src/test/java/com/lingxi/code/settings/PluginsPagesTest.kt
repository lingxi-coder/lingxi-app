package com.lingxi.code.settings

import com.lingxi.code.components.LXIconName
import org.junit.Assert.assertEquals
import org.junit.Test

class PluginsPagesTest {
    @Test
    fun theLocalAppBuilderHasItsOwnIcon() {
        assertEquals(LXIconName.AppBuilder, pluginIcon("local-app-builder@lingxi"))
        assertEquals(LXIconName.AppBuilder, pluginIcon("local-app-builder"))
        assertEquals(LXIconName.Plug, pluginIcon("notes@acme"))
    }

    @Test
    fun rowsShowVendorAndVersion() {
        assertEquals("LingXi · 2.1.0", pluginSubtitle("LingXi", "2.1.0"))
        assertEquals("2.1.0", pluginSubtitle(null, "2.1.0"))
        assertEquals("LingXi", pluginSubtitle("LingXi", ""))
        assertEquals("", pluginSubtitle(" ", ""))
    }
}
