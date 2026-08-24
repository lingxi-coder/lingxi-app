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
            "get_location", "transcribe_speech", "post_notification",
            "clipboard_get_text", "clipboard_set_text", "share", "synthesize_speech",
            "file_read", "file_write",
            "device_status", "haptics", "deep_link",
            "llm_chat", "llm_stream", "agent_post",
            "agent_session_create", "agent_session_list", "agent_session_resume",
            "agent_session_close", "agent_send", "agent_stream", "agent_cancel",
            "agent_profile_propose_update",
            "background_schedule",
            "background_list",
            "background_status",
            "background_cancel",
            "background_retry",
            "calendar_list_events",
            "contacts_search",
            "media_get",
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
            "background_schedule", "background_list", "background_status",
            "background_cancel", "background_retry",
            "Content-Security-Policy", "XMLHttpRequest", "WebSocket", "EventSource", "sendBeacon",
            "External resources are blocked", "error.code = envelope.code",
            "const normalAnchor = attribute === 'href' && this.tagName === 'A'",
            "request_too_large", "worker-src 'self' blob:", "deviceContext", "os: 'android'",
            "formFactor: 'tablet'", "get viewport()", "get safeArea()", "get colorScheme()",
            "get reducedMotion()", "get inputMode()",
            "const channel = pending.get(frame.requestId)?.channel",
            "streamListeners.set(listener, 'llm')", "streamListeners.set(listener, 'agent')",
        ).forEach { token -> assertTrue("missing bootstrap contract: $token", bootstrap.contains(token)) }
        assertFalse(bootstrap.contains("window.screen"))
        assertFalse(bootstrap.contains("addJavascriptInterface"))
        assertFalse(bootstrap.contains("navigator.userAgent"))
    }

    /**
     * Asserted as one whole string, not by `contains` on the directives that
     * happen to be interesting: a CSP is only as strong as its most permissive
     * directive, and this injected meta INTERSECTS with the host's header, so a
     * drift here silently overrides the engine.
     *
     * What this CANNOT do is compare against the engine: `LOCAL_APP_CONTENT_
     * SECURITY_POLICY` lives in Rust and never reaches this target, so the
     * literal below is a hand-kept copy — and it is deliberately not
     * byte-identical (the engine orders `img-src; font-src; connect-src;
     * media-src`, this orders `img-src; media-src; font-src; connect-src`;
     * directive ORDER is not meaningful to a CSP parser). Locking the two
     * together needs the policy to travel over the protocol.
     */
    @Test
    fun `injected CSP pins the whole policy`() {
        val bootstrap = buildLingxiV1Bootstrap(formFactor = "phone")
        assertTrue(
            bootstrap.contains(
                "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
            ),
        )
        // The one placeholder this template really has. Asserting a
        // `__LINGXI_CSP__` token that exists nowhere in the repo could never
        // fail; this one goes red if the substitution is dropped, which would
        // boot every app with the literal string as its `formFactor`.
        assertFalse(bootstrap.contains("__LINGXI_NATIVE_FORM_FACTOR__"))
        assertTrue(bootstrap.contains("formFactor: 'phone'"))
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

    /**
     * Ionic keeps a component's interactive internals in a SHADOW ROOT, which
     * `document.querySelectorAll` does not cross. Before the walk below, an app
     * built from `ion-*` components reported `elements: []` — indistinguishable
     * from a blank screen or a crash, which is the very ambiguity `canvasCount`
     * exists to resolve for a drawn surface.
     *
     * Pinned as tokens rather than by driving a DOM because this file has no
     * WebView: the assertion is that the SHIPPED script still contains the walk,
     * the bounds that keep it from hanging, and the shadow-aware id lookup.
     */
    @Test
    fun `ui inspection crosses shadow roots and resolves the native control`() {
        val inspectScript = buildLocalAppUiExecutionScript(
            buildLocalAppUiExecutionRequest(LocalAppUiAutomationAction.Inspect),
        )
        listOf(
            "const deepQuery = (selector, limit)",
            "if (host.shadowRoot) visit(host.shadowRoot, depth + 1)",
            // Both bounds, or a nested/looping page hangs the tool call.
            "depth > 8 || found.length >= limit",
            "const candidates = () => deepQuery(SELECTOR, 400)",
            // A shadow root is its own id scope; `getElementById` cannot see in.
            "|| deepQuery('[id=\"' +",
            // `ion-input` holds the real <input> inside its shadow root.
            "const nativeControl = element =>",
            "element.shadowRoot.querySelector('input,textarea,select')",
            // Every OTHER lookup has to cross the boundary too, or the walk
            // only fixes enumeration. `canvasCount` is the single signal the
            // render gate keys on for a drawn app; the key receiver falls back
            // to `document.body` (events go UP, not into the canvas) when it
            // cannot see one; and a shadow root is its own id scope, so labels
            // must resolve against the element's OWN root.
            "canvasCount: deepQuery('canvas', 64).length",
            "deepQuery('canvas', 1)[0] || document.body",
            "const scope = element.getRootNode?.() || document",
        ).forEach { token ->
            assertTrue("missing shadow-DOM contract: $token", inspectScript.contains(token))
        }
        assertFalse(
            "the light-DOM-only walk must be gone, not merely supplemented",
            inspectScript.contains("Array.from(document.querySelectorAll(\n"),
        )
    }

    /**
     * Phase 1a twin of the iOS `testUIInspectionReportsElementAndCanvasGeometry`.
     * The two injected scripts are near-copies, so both are pinned or neither is.
     */
    @Test
    fun `ui inspection reports element and canvas geometry`() {
        val inspectScript = buildLocalAppUiExecutionScript("""{"action":"inspect"}""")
        listOf(
            "rect: [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)]",
            "canvases: deepQuery('canvas', 16).map",
            "canvasCount:",
            "documentState: document.readyState",
            "offsetLeft: Math.round(vv.offsetLeft)",
            "scale: vv.scale",
        ).forEach { token ->
            assertTrue("missing geometry contract: $token", inspectScript.contains(token))
        }
    }

    /** Phase 1a twin of iOS `testDocumentStartInstallsABoundedRuntimeErrorLedger`. */
    @Test
    fun `document start installs a bounded runtime error ledger`() {
        val bootstrap = buildLingxiV1Bootstrap("phone")
        listOf(
            "addEventListener('error'",
            "addEventListener('unhandledrejection'",
            "console.error = function",
            "kind: 'console'",
            "const cap = 8",
            "__lingxiRuntimeErrors.length >= cap",
            "__lingxiRuntimeErrorsDropped",
        ).forEach { token ->
            assertTrue("missing runtime-error ledger: $token", bootstrap.contains(token))
        }
        assertTrue(
            buildLocalAppUiExecutionScript("""{"action":"inspect"}""").contains("runtimeErrors:")
        )
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
