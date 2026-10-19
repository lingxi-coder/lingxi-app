package com.lingxi.code.settings

import org.junit.Assert.*
import org.junit.Test

class DeepLinkImportProposalTest {
    @Test fun `edited external proposals cannot supply credentials or preauthorize plugin actions`() {
        val rejected = listOf(
            Triple("mcp", """{"name":"server","config":{"url":"https://example.test/mcp","headers":{"Authorization":"secret"}}}""", "user"),
            Triple("provider", """{"providers":{"example":{"type":"openai","apiKey":"secret","models":["model"]}}}""", "user"),
            Triple("plugin", """{"plugin":"package","confirmed":true,"action":"uninstall"}""", "user"),
            Triple("plugin", """{"source":"https://example.test/plugin?token=secret"}""", "user"),
            Triple("skill", """{"name":"test","markdown":"# Test"}""", "local"),
        )
        rejected.forEach { (kind, draft, scope) -> assertTrue("$kind must remain a rejected proposal", runCatching { prepareDeepLinkImportProposal(kind, draft, scope) }.isFailure) }
    }
    @Test fun `valid imports remain inert review data with explicit destination`() {
        val mcp = prepareDeepLinkImportProposal("mcp", """{"name":"example","config":{"command":"echo","args":["review"]}}""", "user")
        assertEquals("echo", mcp.getJSONObject("config").getString("command"))
        val skill = prepareDeepLinkImportProposal("skill", """{"name":"test","markdown":"# Test"}""", "project")
        assertEquals("# Test", skill.getString("markdown"))
        val plugin = prepareDeepLinkImportProposal("plugin", """{"plugin":"package@market"}""", "user")
        assertEquals("install", plugin.getString("action"))
        assertFalse(plugin.has("confirmed"))
        val provider = prepareDeepLinkImportProposal("provider", """{"providers":{"example":{"type":"openai","baseUrl":"https://example.test/v1","models":["model"]}}}""", "user")
        assertTrue(provider.getJSONObject("providers").has("example"))
    }
}
