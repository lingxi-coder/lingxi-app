package com.lingxi.code.plugins

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PluginCatalogTest {
    private val json = """
        {
          "installed": [
            {"id": "local-app-builder@lingxi", "name": "local-app-builder", "display_name": "Local App Builder",
             "version": "2.1.0", "path": "/home/.lingxi/plugins/cache/lingxi/local-app-builder/2.1.0",
             "default_enabled": true, "description": "Build apps", "dependencies": [], "config_schema_json": "null"},
            {"id": "quiet@acme", "name": "quiet", "display_name": "", "version": "1.0.0", "path": "/p",
             "default_enabled": false, "description": "", "dependencies": ["base@acme"], "config_schema_json": "null"},
            {"id": "", "name": "broken"}
          ],
          "available": [
            {"id": "local-app-builder@lingxi", "name": "local-app-builder", "marketplace": "lingxi", "version": "",
             "description": "Build apps", "installed": true, "upgrade_available": false},
            {"id": "notes@acme", "name": "notes", "marketplace": "acme", "version": "0.3.0",
             "description": "Notes", "installed": false, "upgrade_available": false}
          ],
          "marketplaces": [
            {"name": "lingxi", "source_json": "{\"source\":\"github\",\"repo\":\"lingxi-coder/local-app-builder\"}",
             "install_location": "/home/.lingxi/plugins/marketplaces/lingxi", "last_updated": null},
            {"name": "acme", "source_json": "{\"source\":\"directory\",\"path\":\"/srv/acme\"}", "install_location": null}
          ],
          "enabled_json": "{\"local-app-builder@lingxi\": false}",
          "revisions": {"user": "abc", "project": "def"}
        }
    """.trimIndent()

    @Test
    fun parsesTheEngineRows() {
        val catalog = PluginCatalog.parse(json)
        assertEquals(listOf("local-app-builder@lingxi", "quiet@acme"), catalog.installed.map(InstalledPlugin::id))
        // A blank display name falls back to the plugin's name.
        assertEquals("quiet", catalog.installed(("quiet@acme"))?.displayName)
        assertEquals(listOf("base@acme"), catalog.installed("quiet@acme")?.dependencies)
        assertEquals("lingxi", catalog.installed("local-app-builder@lingxi")?.marketplace)
        assertEquals(mapOf("user" to "abc", "project" to "def"), catalog.revisions)
        assertEquals(listOf("notes@acme"), catalog.installable.map(AvailablePlugin::id))
    }

    @Test
    fun anExplicitSettingWinsOverTheManifestDefault() {
        val catalog = PluginCatalog.parse(json)
        assertFalse(catalog.isEnabled(catalog.installed("local-app-builder@lingxi")!!))
        assertFalse(catalog.isEnabled(catalog.installed("quiet@acme")!!))
        val enabled = catalog.copy(enabled = mapOf("quiet@acme" to true))
        assertTrue(enabled.isEnabled(enabled.installed("quiet@acme")!!))
    }

    @Test
    fun marketplacesSayWhereTheyComeFrom() {
        val catalog = PluginCatalog.parse(json)
        val lingxi = catalog.marketplace("lingxi")!!
        assertEquals("github", lingxi.sourceKind)
        assertEquals("lingxi-coder/local-app-builder", lingxi.sourceLocation)
        assertEquals("/srv/acme", catalog.marketplace("acme")?.sourceLocation)
        assertNull(catalog.marketplace("missing"))
    }

    @Test
    fun missingOrMalformedJsonIsAnEmptyCatalog() {
        assertEquals(PluginCatalog(), PluginCatalog.parse(null))
        assertEquals(PluginCatalog(), PluginCatalog.parse("not json"))
    }

    @Test
    fun theLingxiMarketplaceIsAddedOnceAndNeverBroughtBack() {
        val empty = PluginCatalog()
        assertEquals(LingxiMarketplace.GITHUB_SOURCE, LingxiMarketplace.sourceToAdd(empty, seen = false, providerRegion = "international"))
        // No mirror for mainland China yet: GitHub serves every region.
        assertEquals(LingxiMarketplace.GITHUB_SOURCE, LingxiMarketplace.sourceToAdd(empty, seen = false, providerRegion = "china_mainland"))
        // Already registered, or registered once and removed by the user since.
        assertNull(LingxiMarketplace.sourceToAdd(PluginCatalog.parse(json), seen = false, providerRegion = null))
        assertNull(LingxiMarketplace.sourceToAdd(empty, seen = true, providerRegion = null))
    }
}
