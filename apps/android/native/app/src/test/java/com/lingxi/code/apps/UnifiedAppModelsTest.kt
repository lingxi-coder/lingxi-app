package com.lingxi.code.apps

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class UnifiedAppModelsTest {
    private val launch = AppLaunch(AppRun("a123", "run123", 2, "digest", "running", null, setOf("api")),
        "http://127.0.0.1:49123/__run/run123/index.html", "http://127.0.0.1:49123")

    @Test fun `native calls require current main frame and exact run origin`() {
        assertTrue(isCurrentAppFrame(launch, launch.origin, launch.url, true))
        assertTrue(isCurrentAppFrame(launch, launch.origin, launch.url.replace("index.html", "my%20screen.html"), true))
        assertFalse(isCurrentAppFrame(launch, launch.origin, launch.url, false))
        assertFalse(isCurrentAppFrame(launch, "http://localhost:49123", launch.url, true))
        assertFalse(isCurrentAppFrame(launch, "http://127.0.0.1:49124", launch.url, true))
        assertFalse(isCurrentAppFrame(launch, launch.origin, launch.url.replace("run123", "run124"), true))
        assertFalse(isCurrentAppFrame(launch, launch.origin, "http://127.0.0.1:49123/", true))
    }
    @Test fun `stopped and encoded path callers are rejected`() {
        assertFalse(isCurrentAppFrame(launch.copy(run = launch.run.copy(phase = "stopped")), launch.origin, launch.url, true))
        assertFalse(isCurrentAppFrame(launch, launch.origin, launch.url.replace("index.html", "../secret"), true))
        assertFalse(isCurrentAppFrame(launch, launch.origin, launch.url.replace("index.html", "%2e%2e/secret"), true))
        assertFalse(isCurrentAppFrame(launch, launch.origin, launch.url.replace("127.0.0.1", "user@127.0.0.1"), true))
    }
    @Test fun `service routing only exposes registered current run services`() {
        assertEquals("ws://127.0.0.1:49123/__run/run123/__service/api/events", appServiceUrl(launch, "api", "/events", true))
        assertThrows(IllegalArgumentException::class.java) { appServiceUrl(launch, "other-app", "/", false) }
        assertThrows(IllegalArgumentException::class.java) { appServiceUrl(launch, "api", "//evil.test", false) }
        assertThrows(IllegalArgumentException::class.java) { appServiceUrl(launch, "api", "/../secret", false) }
        assertThrows(IllegalArgumentException::class.java) { appServiceUrl(launch, "api", "/%2e%2e/secret", false) }
        assertThrows(IllegalArgumentException::class.java) { appServiceUrl(launch, "api", "/x%2fy", false) }
        assertEquals("http://127.0.0.1:49123/__run/run123/__service/api/hello%20world?q=a..b", appServiceUrl(launch, "api", "/hello%20world?q=a..b", false))
    }
    @Test fun `bridge errors preserve explicit error code and message`() {
        val error = assertThrows(AppServiceException::class.java) { decodeAppResponse("""{"ok":false,"error":{"code":"executionConsentRequired","message":"Allow this release first"}}""") }
        assertEquals("executionConsentRequired", error.code)
        assertEquals("Allow this release first", error.message)
        assertNull(decodeAppResponse("""{"ok":true,"value":null}"""))
    }
    @Test fun `native task DTO retains observed progress usage and error`() {
        val task = JSONObject("""{"runId":"run-1","requestId":"opaque-Request-1","state":"failed","sequence":4,"phase":"waiting_for_permission","progress":{"completedModelTurns":2,"turnLimit":8},"createdAt":10,"updatedAt":12,"model":{"provider":"fixture","model":"fixture"},"limits":{"maxTurns":8,"timeoutMs":60000},"usage":{"inputTokens":11,"outputTokens":7,"costUsd":0.0123,"costEstimated":true,"turns":2},"error":{"code":"permissionDenied","message":"Tool access was revoked"}}""").appAgentTask()
        assertEquals("run-1", task.runId)
        assertEquals("opaque-Request-1", task.requestId)
        assertEquals("failed", task.state)
        assertEquals("waiting_for_permission", task.phase)
        assertEquals(2, task.progress?.getInt("completedModelTurns"))
        assertEquals(8, task.progress?.getInt("turnLimit"))
        assertEquals(11, task.usage?.getInt("inputTokens"))
        assertEquals(0.0123, task.usage?.getDouble("costUsd") ?: 0.0, 0.00001)
        assertEquals("permissionDenied", task.error?.getString("code"))
        assertEquals("Tool access was revoked", task.error?.getString("message"))
    }
    @Test fun `schema two record preserves tagged authoring and market identities`() {
        val workspaceId = "123e4567-e89b-12d3-a456-426614174000"
        val projectId = "223e4567-e89b-12d3-a456-426614174000"
        val sessionId = "323e4567-e89b-12d3-a456-426614174000"
        val source = "/tmp/.lingxi/.apps/calculator"
        val record = JSONObject("""{"appId":"a0123456789abcdef0123456789abcdef","name":"Calculator","summary":"Offline","currentRevision":2,"latestRevision":3,"originPort":49123,"executionGrants":["digest"],"updatedAtMs":3,"authoring":{"workspaceId":"$workspaceId","sourceDirectory":"$source","projectRef":{"kind":"mobileProject","projectId":"$projectId"},"authoringSessionId":"$sessionId","appId":"a0123456789abcdef0123456789abcdef"},"marketOrigin":{"appId":"market-calculator","releaseId":"release-3","digest":"public-digest"}}""").appRecord()
        assertEquals(33, record.id.length)
        assertEquals("market-calculator", record.marketOrigin?.appId)
        assertEquals(workspaceId, record.authoring?.workspaceId)
        assertEquals("mobileProject", record.authoring?.projectRef?.kind)
        assertEquals(projectId, record.authoring?.projectRef?.projectId)
        assertEquals(sessionId, record.authoring?.authoringSessionId)
        assertTrue(record.authoring!!.isBoundTo(record.id, projectId, source))
        assertFalse(record.authoring!!.isBoundTo(record.id, projectId, "$source/other"))
        assertEquals(2L, record.currentRevision)
        assertEquals(3L, record.latestRevision)
        val copy = JSONObject("""{"appId":"local-copy","name":"Custom","summary":"","currentRevision":1,"latestRevision":1,"originPort":49124,"executionGrants":[],"updatedAtMs":4,"marketOrigin":{"appId":"market-calculator","releaseId":"release-3","digest":"public-digest","remixedFrom":"market-calculator"}}""").appRecord()
        assertEquals("market-calculator", copy.marketOrigin?.remixedFrom)
    }
    @Test fun `legacy source pointer requires explicit authoring rebind`() {
        val record = JSONObject("""{"appId":"a0123456789abcdef0123456789abcdef","name":"Calculator","summary":"","currentRevision":1,"latestRevision":1,"originPort":49123,"executionGrants":[],"updatedAtMs":3,"authoring":{"sourceDirectory":"/tmp/source","sourceSessionId":"old-session","projectId":"old-project"}}""").appRecord()
        assertEquals("/tmp/source", record.authoring?.sourceDirectory)
        assertFalse(record.authoring?.isComplete ?: true)
        assertFalse(record.authoring!!.isBoundTo(record.id, "223e4567-e89b-12d3-a456-426614174000", "/tmp/source"))
    }
    @Test fun `execution consent includes build and install even for static entry`() {
        val static = AppRelease("app", 1, "digest", JSONObject("""{"entry":{"type":"static","path":"index.html"}}"""))
        assertFalse(static.executesCode)
        assertTrue(static.copy(manifest = JSONObject("""{"build":[{"command":"npm run build"}]}""")).executesCode)
        assertTrue(static.copy(manifest = JSONObject("""{"install":[{"command":"npm ci"}]}""")).executesCode)
        assertTrue(static.copy(manifest = JSONObject("""{"capabilities":["process"]}""")).executesCode)
        assertFalse(static.copy(manifest = JSONObject("""{"capabilities":["agent"]}""")).executesCode)
    }
    @Test fun `Android architecture uses canonical target vocabulary rather than APK ABI`() {
        assertEquals("aarch64", canonicalAppArchitecture("arm64-v8a"))
        assertEquals("x86_64", canonicalAppArchitecture("x86_64"))
        assertEquals("unknown", canonicalAppArchitecture(null))
    }

}
