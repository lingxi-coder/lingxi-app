package com.lingxi.code.plugins

import java.io.File
import java.nio.file.Files
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PluginDetailsTest {
    private val root: File = Files.createTempDirectory("plugin-details").toFile()

    @After
    fun cleanUp() {
        root.deleteRecursively()
    }

    private fun write(path: String, text: String) {
        File(root, path).apply { parentFile?.mkdirs() }.writeText(text)
    }

    @Test
    fun readsTheStandardLayout() {
        write(
            "plugin/.lingxi-plugin/plugin.json",
            """{"name":"p","author":{"name":"LingXi"},"license":"MIT","repository":{"url":"https://example.test/p"},"commands":["./extra"],"agents":"../outside"}""",
        )
        write("plugin/skills/create-app/SKILL.md", "---\nname: create-local-app\ndescription: >\n  Plan and build\n  a local app.\n---\nBody")
        write("plugin/skills/notes.md", "not a skill")
        write("plugin/commands/build.md", "---\ndescription: \"Build it\"\n---\n")
        write("plugin/extra/nested/ship.md", "no frontmatter")
        write("plugin/hooks/hooks.json", """{"hooks":{"PreToolUse":[{"matcher":"Bash"}],"Stop":[]}}""")
        write("plugin/.mcp.json", """{"mcpServers":{"files":{"command":"x"}}}""")
        write("outside/evil.md", "---\nname: evil\n---\n")

        val details = PluginDetailsReader.read(File(root, "plugin"))

        assertEquals("LingXi", details.vendor)
        assertEquals("MIT", details.license)
        assertEquals("https://example.test/p", details.repository)
        assertEquals(listOf(PluginComponent("create-local-app", "Plan and build a local app.")), details.skills)
        assertEquals(listOf(PluginComponent("build", "Build it"), PluginComponent("ship", "")), details.commands)
        // A declared path that leaves the plugin is ignored.
        assertEquals(emptyList<PluginComponent>(), details.agents)
        assertEquals(listOf("PreToolUse", "Stop"), details.hooks)
        assertEquals(listOf("files"), details.mcpServers)
    }

    @Test
    fun theVendorFallsBackToTheMarketplaceOwner() {
        write("plugin/.lingxi-plugin/plugin.json", """{"name":"p"}""")
        write("market/.lingxi-plugin/marketplace.json", """{"name":"lingxi","owner":{"name":"LingXi"}}""")
        assertEquals("LingXi", PluginDetailsReader.vendor(File(root, "plugin"), File(root, "market")))
        assertNull(PluginDetailsReader.vendor(File(root, "plugin")))
        // A string author counts too.
        write("plugin/.lingxi-plugin/plugin.json", """{"name":"p","author":"Acme"}""")
        assertEquals("Acme", PluginDetailsReader.vendor(File(root, "plugin"), File(root, "market")))
    }

    @Test
    fun frontmatterHandlesQuotesAndBlockScalars() {
        val fields = frontmatter("---\nname: 'it''s'\ndescription: |\n  line one\n  line two\nother: plain\n---\nbody: no")
        assertEquals(mapOf("name" to "it's", "description" to "line one\nline two", "other" to "plain"), fields)
        assertEquals(emptyMap<String, String>(), frontmatter("no frontmatter"))
    }
}
