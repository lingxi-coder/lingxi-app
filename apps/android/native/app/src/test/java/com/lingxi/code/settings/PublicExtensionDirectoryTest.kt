package com.lingxi.code.settings

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import com.lingxi.code.plugins.PluginMarketplace

class PublicExtensionDirectoryTest {
    @Test
    fun directoryFiltersForAndroidAndResolvesFiveLanguageKeys() {
        val entries = parsePublicExtensionDirectory(
            """{"schemaVersion":1,"items":[
                {"id":"github","kind":"mcp","name":"GitHub","publisher":"GitHub","category":"development","platforms":["android","ios"],
                 "description":{"en":"Work with repositories","zh-Hans":"管理仓库","zh-Hant":"管理儲存庫","ja":"リポジトリを管理","ko":"저장소 관리"},
                 "sourceUrl":"https://example.com/source","documentationUrl":"https://example.com/docs",
                 "mcp":{"name":"github","config":{"url":"https://example.com/mcp","headers":{"Authorization":"Bearer reference"},"env":{"TOKEN":"GITHUB_TOKEN"}}}},
                {"id":"desktop-only","kind":"mcp","name":"Desktop","publisher":"Example","category":"development","platforms":["macos"],"description":{"en":"Desktop only"},"sourceUrl":"https://example.com/source","documentationUrl":"https://example.com/docs"}
            ]}""",
        )

        assertEquals(listOf("github"), entries.map { it.id })
        assertEquals("管理儲存庫", entries.single().localizedDescription("zh-Hant-TW"))
        assertEquals("Work with repositories", entries.single().localizedDescription("en-US"))
        assertEquals("GITHUB_TOKEN", org.json.JSONObject(entries.single().mcpConfigJson!!).getJSONObject("env").getString("TOKEN"))
        assertTrue(entries.single().mcpConfigJson!!.contains("Authorization"))
    }

    @Test
    fun searchAndCategoryComposeAndConfiguredMcpIsScoped() {
        val entries = parsePublicExtensionDirectory(
            """{"schemaVersion":1,"items":[
                {"id":"browser","kind":"mcp","name":"Browser Search","publisher":"Example","category":"browser","platforms":["android"],"description":{"en":"Search pages"},"sourceUrl":"https://example.com/source","documentationUrl":"https://example.com/docs"},
                {"id":"github","kind":"mcp","name":"GitHub","publisher":"Example","category":"development","platforms":["android"],"description":{"en":"Repositories"},"sourceUrl":"https://example.com/source","documentationUrl":"https://example.com/docs"}
            ]}""",
        )
        assertEquals(listOf("browser"), filterPublicExtensions(entries, "search", "browser", "en").map { it.id })
        assertEquals(listOf("github"), filterPublicExtensions(entries, "GitHub", "all", "en").map { it.id })

        val configured = configuredMcpEntries(
            """{"scopes":[
                {"scope":"user","raw_json":"{\"mcpServers\":{\"github\":{\"url\":\"https://example.com\"}}}"},
                {"scope":"project","raw_json":"{\"mcpServers\":{\"browser\":{\"command\":\"npx\"}}}"}
            ]}""",
        )
        assertEquals(listOf("browser:project", "github:user"), configured.map { "${it.name}:${it.scope}" })
        assertEquals("npx", org.json.JSONObject(configured.first().configJson).getString("command"))
    }

    @Test
    fun pluginMarketplaceMetadataIsRetainedWithoutMcpConfig() {
        val entry = parsePublicExtensionDirectory(
            """{"schemaVersion":1,"items":[
                {"id":"webmcp","kind":"plugin","name":"WebMCP","publisher":"Example","category":"browser","platforms":["android"],
                 "description":{"en":"Web tools"},"sourceUrl":"https://example.com/source","documentationUrl":"https://example.com/docs",
                 "marketplace":{"source":"example/webmcp","name":"example","plugin":"webmcp"}}
            ]}""",
        ).single()

        assertEquals("example/webmcp", entry.marketplace?.source)
        assertEquals("example", entry.marketplace?.name)
        assertEquals("webmcp", entry.marketplace?.pluginId)
        assertEquals(null, entry.mcpConfigJson)
    }

    @Test
    fun publicMcpStatusRequiresMatchingNameAndTransportIdentity() {
        val entry = parsePublicExtensionDirectory(
            """{"schemaVersion":1,"items":[
                {"id":"context7","kind":"mcp","name":"Context7","publisher":"Upstash","category":"development","platforms":["android"],
                 "description":{"en":"Docs"},"sourceUrl":"https://example.com/source","documentationUrl":"https://example.com/docs",
                 "mcp":{"name":"context7","config":{"type":"http","url":"https://mcp.context7.com/mcp"}}}
            ]}""",
        ).single()
        val configuredWithCustomHeaders = ConfiguredMcpEntry(
            "context7", "user",
            """{"type":"http","url":"https://mcp.context7.com/mcp","headers":{"Authorization":"custom"},"env":{"TOKEN":"CONTEXT7_TOKEN"}}""",
        )
        val sameNameDifferentUrl = configuredWithCustomHeaders.copy(
            configJson = """{"type":"http","url":"https://other.example/mcp"}""",
        )
        val differentNameSameUrl = configuredWithCustomHeaders.copy(name = "docs")

        assertTrue(isPublicMcpConfigured(entry, listOf(configuredWithCustomHeaders)))
        assertFalse(isPublicMcpConfigured(entry, listOf(sameNameDifferentUrl)))
        assertFalse(isPublicMcpConfigured(entry, listOf(differentNameSameUrl)))

        val stdioEntry = parsePublicExtensionDirectory(
            """{"schemaVersion":1,"items":[
                {"id":"playwright","kind":"mcp","name":"Playwright","publisher":"Example","category":"browser","platforms":["android"],
                 "description":{"en":"Browser automation"},"sourceUrl":"https://example.com/source","documentationUrl":"https://example.com/docs",
                 "mcp":{"name":"playwright","config":{"command":"npx","args":["-y","@playwright/mcp@1"]}}}
            ]}""",
        ).single()
        val sameNameDifferentCommand = ConfiguredMcpEntry("playwright", "user", """{"command":"node","args":["server.js"]}""")

        assertFalse(isPublicMcpConfigured(stdioEntry, listOf(sameNameDifferentCommand)))
    }

    @Test
    fun marketplaceAddedStatusMatchesSourceNotMarketplaceName() {
        val publicSource = "example/webmcp"
        val sameNameDifferentSource = PluginMarketplace(
            name = "example",
            sourceJson = """{"source":"github","repo":"different/webmcp"}""",
            installLocation = null,
            lastUpdated = null,
        )
        val differentNameSameSource = PluginMarketplace(
            name = "a-different-display-name",
            sourceJson = """{"source":"github","repo":"example/webmcp"}""",
            installLocation = null,
            lastUpdated = null,
        )
        val sameSourceGitUrl = PluginMarketplace(
            name = "mirror-name",
            sourceJson = """{"source":"git","url":"https://github.com/example/webmcp.git"}""",
            installLocation = null,
            lastUpdated = null,
        )
        val sameSourceGitSshUrl = PluginMarketplace(
            name = "ssh-mirror",
            sourceJson = """{"source":"git","url":"ssh://git@github.com/example/webmcp.git"}""",
            installLocation = null,
            lastUpdated = null,
        )
        val sameNameDirectoryPath = PluginMarketplace(
            name = "example",
            sourceJson = """{"source":"directory","path":"example/webmcp"}""",
            installLocation = null,
            lastUpdated = null,
        )
        val sameNameCustomRef = PluginMarketplace(
            name = "example",
            sourceJson = """{"source":"github","repo":"example/webmcp","ref":"release"}""",
            installLocation = null,
            lastUpdated = null,
        )
        val sameNameGitSubpath = PluginMarketplace(
            name = "example",
            sourceJson = """{"source":"git","url":"https://github.com/example/webmcp/tree/release"}""",
            installLocation = null,
            lastUpdated = null,
        )
        val sameNameGitShorthand = PluginMarketplace(
            name = "example",
            sourceJson = """{"source":"git","url":"example/webmcp"}""",
            installLocation = null,
            lastUpdated = null,
        )
        val sameNameUrlSource = PluginMarketplace(
            name = "example",
            sourceJson = """{"source":"url","url":"https://github.com/example/webmcp"}""",
            installLocation = null,
            lastUpdated = null,
        )

        assertFalse(marketplaceSourceMatches(publicSource, sameNameDifferentSource))
        assertTrue(marketplaceSourceMatches(publicSource, differentNameSameSource))
        assertTrue(marketplaceSourceMatches(publicSource, sameSourceGitUrl))
        assertTrue(marketplaceSourceMatches(publicSource, sameSourceGitSshUrl))
        assertFalse(marketplaceSourceMatches(publicSource, sameNameDirectoryPath))
        assertFalse(marketplaceSourceMatches(publicSource, sameNameCustomRef))
        assertFalse(marketplaceSourceMatches(publicSource, sameNameGitSubpath))
        assertFalse(marketplaceSourceMatches(publicSource, sameNameGitShorthand))
        assertFalse(marketplaceSourceMatches(publicSource, sameNameUrlSource))
    }

    @Test
    fun publicMcpNameConflictOnlyAllowsMatchingDirectoryTargetOrRename() {
        val expected = """{"url":"https://mcp.example/server"}"""
        val different = """{"command":"node","args":["other.js"]}"""
        val sameEndpointWithCustomHeaders = """{"url":"https://mcp.example/server","headers":{"Authorization":"custom"}}"""

        assertTrue(publicMcpNameConflicts("server", "user", "server", null, expected, expected, different))
        assertFalse(publicMcpNameConflicts("server", "user", "server", "user", null, expected, sameEndpointWithCustomHeaders))
        assertTrue(publicMcpNameConflicts("server", "project", "server", "user", null, expected, different))
        assertFalse(publicMcpNameConflicts("server-copy", "user", "server", null, expected, expected, null))
    }
}
