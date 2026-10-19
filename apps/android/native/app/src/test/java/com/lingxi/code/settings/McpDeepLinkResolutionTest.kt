package com.lingxi.code.settings

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class McpDeepLinkResolutionTest {
    private fun definition(scope: String, name: String) = JSONObject().put("scope", scope)
        .put("raw_json", JSONObject().put("mcpServers", JSONObject().put(name, JSONObject().put("command", scope))).toString())
    @Test fun `project detail never resolves a same named user server`() {
        val user = definition("user", "shared")
        val project = definition("project", "shared")
        assertSame(project, resolveMcpDeepLinkScope(listOf(user, project), "shared", "project"))
        assertTrue(runCatching { resolveMcpDeepLinkScope(listOf(user), "shared", "project") }.isFailure)
    }
    @Test fun `unscoped detail requires a unique existing definition`() {
        val user = definition("user", "shared")
        assertSame(user, resolveMcpDeepLinkScope(listOf(user), "shared", null))
        assertTrue(runCatching { resolveMcpDeepLinkScope(listOf(user, definition("local", "shared")), "shared", null) }.isFailure)
        assertTrue(runCatching { resolveMcpDeepLinkScope(listOf(user), "missing", null) }.isFailure)
    }
}
