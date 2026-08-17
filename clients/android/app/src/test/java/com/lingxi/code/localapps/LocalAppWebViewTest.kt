package com.lingxi.code.localapps

import com.lingxi.code.bindings.AppBridgeOperationDto
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalAppWebViewTest {

    @Test
    fun `all bridge operations have an exhaustive Android wire mapping`() {
        val expected = setOf(
            "query_data", "mutate_data", "network_request", "runtime_status",
            "capture_photo", "pick_image", "record_audio_start", "record_audio_stop",
            "get_location", "transcribe_speech", "post_notification", "llm_chat", "agent_post",
            "agent_session_create", "agent_session_list", "agent_session_resume",
            "agent_session_close", "agent_send", "agent_stream", "agent_cancel",
            "agent_profile_propose_update",
            "background_schedule",
        )

        assertEquals(expected, AppBridgeOperationDto.entries.map { it.bridgeWireName() }.toSet())
        AppBridgeOperationDto.entries.forEach { operation ->
            assertEquals(operation, bridgeOperationFor(operation.bridgeWireName()))
        }
    }

    @Test
    fun `bridge parser binds the host app and enforces shape and byte limits`() {
        val accepted = parseLocalAppBridgeMessage(
            appId = "host-app",
            rawMessage = """{"requestId":"r-1","operation":"get_location","payload":{}}""",
            inFlightRequestIds = emptySet(),
        ) as LocalAppBridgeIngress.Accepted
        assertEquals("host-app", accepted.message.appId)
        assertEquals("get_location", accepted.message.operation)

        val largeText = "x".repeat(LOCAL_APP_BRIDGE_MAX_CONTROL_BYTES)
        val acceptedLargeLlm = parseLocalAppBridgeMessage(
            appId = "host-app",
            rawMessage = """{"requestId":"r-2","operation":"llm_chat","payload":{"messages":[{"role":"user","content":"$largeText"}]}}""",
            inFlightRequestIds = emptySet(),
        ) as LocalAppBridgeIngress.Accepted
        assertEquals("llm_chat", acceptedLargeLlm.message.operation)

        val oversizedControl = parseLocalAppBridgeMessage(
            appId = "host-app",
            rawMessage = """{"requestId":"r-control","operation":"query_data","payload":{"value":"$largeText"}}""",
            inFlightRequestIds = emptySet(),
        ) as LocalAppBridgeIngress.Rejected
        assertEquals("request_too_large", oversizedControl.code)

        val oversizedUnknownField = parseLocalAppBridgeMessage(
            appId = "host-app",
            rawMessage = """{"requestId":"r-unknown","operation":"query_data","payload":{},"ignored":"${"x".repeat(LOCAL_APP_BRIDGE_MAX_CONTROL_BYTES)}"}""",
            inFlightRequestIds = emptySet(),
        ) as LocalAppBridgeIngress.Rejected
        assertEquals("request_too_large", oversizedUnknownField.code)

        val longOperation = parseLocalAppBridgeMessage(
            appId = "host-app",
            rawMessage = """{"requestId":"r-3","operation":"${"x".repeat(LOCAL_APP_BRIDGE_MAX_TEXT_LENGTH + 1)}","payload":{}}""",
            inFlightRequestIds = emptySet(),
        ) as LocalAppBridgeIngress.Rejected
        assertEquals("operation_invalid", longOperation.code)
    }

    @Test
    fun `LLM chat has a bounded large-context lane`() {
        assertEquals(64 * 1024, localAppBridgeByteLimit("query_data"))
        assertEquals(8 * 1024 * 1024, localAppBridgeByteLimit("llm_chat"))

        val oversizedLlm = parseLocalAppBridgeMessage(
            appId = "host-app",
            rawMessage = """{"requestId":"large","operation":"llm_chat","payload":{"value":"${"x".repeat(LOCAL_APP_BRIDGE_MAX_LLM_BYTES)}"}}""",
            inFlightRequestIds = emptySet(),
        ) as LocalAppBridgeIngress.Rejected
        assertEquals("request_too_large", oversizedLlm.code)
    }

    @Test
    fun `bridge parser rejects duplicate and excess outstanding requests`() {
        val duplicate = parseLocalAppBridgeMessage(
            "app",
            """{"requestId":"same","operation":"query_data","payload":{}}""",
            setOf("same"),
        ) as LocalAppBridgeIngress.Rejected
        assertEquals("duplicate_request_id", duplicate.code)

        val full = (0 until LOCAL_APP_BRIDGE_MAX_IN_FLIGHT).mapTo(linkedSetOf()) { "r-$it" }
        val overflow = parseLocalAppBridgeMessage(
            "app",
            """{"requestId":"next","operation":"query_data","payload":{}}""",
            full,
        ) as LocalAppBridgeIngress.Rejected
        assertEquals("too_many_requests", overflow.code)
    }

    @Test
    fun `document start bootstrap exposes parity APIs and blocks direct external channels`() {
        val bootstrap = buildLingxiV1Bootstrap(formFactor = "tablet")
        listOf(
            "capturePhoto", "pickImage", "recordAudioStart", "recordAudioStop", "getLocation",
            "transcribeSpeech", "postNotification", "llm_chat", "agent_post",
            "Content-Security-Policy", "XMLHttpRequest", "WebSocket", "EventSource", "sendBeacon",
            "External resources are blocked", "error.code = envelope.code",
            "const normalAnchor = attribute === 'href' && this.tagName === 'A'",
            "request_too_large", "worker-src 'none'", "deviceContext", "os: 'android'",
            "formFactor: 'tablet'", "get viewport()", "get safeArea()", "get colorScheme()",
            "get reducedMotion()", "get inputMode()",
        ).forEach { token -> assertTrue("missing bootstrap contract: $token", bootstrap.contains(token)) }
        assertFalse(bootstrap.contains("window.screen"))
        assertFalse(bootstrap.contains("addJavascriptInterface"))
        assertFalse(bootstrap.contains("navigator.userAgent"))
    }

    @Test
    fun `native Android form factor uses host configuration rather than viewport geometry`() {
        assertEquals("phone", androidFormFactor(smallestScreenWidthDp = 599))
        assertEquals("tablet", androidFormFactor(smallestScreenWidthDp = 600))
    }

    @Test
    fun `structured host script preserves semantic button and labelled textbox lookup`() {
        val clickRequest = buildLocalAppUiExecutionRequest(
            LocalAppUiAutomationAction.Click(
                LocalAppUiTarget(role = "button", name = "Save"),
            ),
        )
        assertTrue(clickRequest.contains("\"action\":\"click\""))
        assertTrue(clickRequest.contains("\"role\":\"button\""))
        assertTrue(clickRequest.contains("\"name\":\"Save\""))

        val fillRequest = buildLocalAppUiExecutionRequest(
            LocalAppUiAutomationAction.Fill(
                LocalAppUiTarget(role = "textbox", name = "Title"),
                value = "Orders",
            ),
        )
        assertTrue(fillRequest.contains("\"role\":\"textbox\""))
        assertTrue(fillRequest.contains("\"name\":\"Title\""))
        assertTrue(fillRequest.contains("\"value\":\"Orders\""))

        val inspectScript = buildLocalAppUiExecutionScript(
            buildLocalAppUiExecutionRequest(LocalAppUiAutomationAction.Inspect),
        )
        assertTrue(inspectScript.contains("BUTTON: 'button'"))
        assertTrue(inspectScript.contains("label[for=\"\${CSS.escape(element.id)}\"]"))
        assertTrue(inspectScript.contains("SELECT: 'combobox'"))
        assertTrue(inspectScript.contains("element.placeholder"))
    }

    @Test
    fun `ui execution parsing rejects false null and no result envelopes`() {
        assertEquals(
            "WebView UI automation returned no result",
            parseLocalAppUiExecutionResult("false").error,
        )
        assertEquals(
            "WebView UI automation returned no result",
            parseLocalAppUiExecutionResult("null").error,
        )
        assertEquals(
            "UI target was not found",
            parseLocalAppUiExecutionResult("""{"ok":false,"error":"UI target was not found"}""").error,
        )
        assertEquals(
            "WebView UI automation returned no result",
            parseLocalAppUiExecutionResult("""{"ok":true}""").error,
        )
    }

    @Test
    fun `ui execution parsing unwraps successful inspect payloads`() {
        val result = parseLocalAppUiExecutionResult(
            """{"ok":true,"result":{"elements":[{"role":"button","name":"Save"},{"role":"textbox","name":"Title"}]}}""",
        )

        assertNull(result.error)
        assertTrue(result.resultJson?.contains("Save") == true)
        assertTrue(result.resultJson?.contains("Title") == true)
    }
}
