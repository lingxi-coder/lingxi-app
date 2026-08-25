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

    /**
     * Phase 1a twin of iOS `testSnapshotDegradesInAFixedOrderAndSaysSo`.
     *
     * Fix round 1: code review found the budget measured `.length` — UTF-16
     * CODE UNITS — while the native guard this ladder exists to stay under
     * measures real UTF-8 BYTES. A CJK character is 1 code unit but 3 bytes,
     * so a length-only check could call a payload "safe" at roughly a third
     * of its true size, and this product's default content is Chinese. The
     * `TextEncoder` token below is pinned so a future edit back to `.length`
     * fails here instead of silently reintroducing the gap.
     */
    @Test
    fun `snapshot degrades in a fixed order and says so`() {
        val script = buildLocalAppUiExecutionScript("""{"action":"inspect"}""")
        listOf(
            "const BUDGET = 200 * 1024",
            "for (const seg of ['elements', 'canvases', 'runtimeErrors'])",
            "truncated.push(seg)",
            "truncated: []",
            // Still TextEncoder (not `.length`), but through the reference the
            // bootstrap captured before page code could shadow it.
            "new (window.__lingxiTextEncoder || TextEncoder)().encode(JSON.stringify(out)).length",
        ).forEach { token ->
            assertTrue("missing payload budget: $token", script.contains(token))
        }
    }

    /**
     * Final review, finding 5 — Android twin of iOS
     * `testDocumentStartCapturesTextEncoderBeforeThePageCanShadowIt`.
     *
     * `size()` calls `new TextEncoder()` at snapshot time and
     * `window.TextEncoder` is PAGE-CONTROLLABLE, so an app that shadows it made
     * `snapshot()` throw and took `inspect_ui` down with it — strictly worse
     * than the `.length` measurement it replaced, which could not throw. The
     * bootstrap's console hook already solved this class by binding the real
     * function at document-start; this pins the same treatment for
     * `TextEncoder`, on both halves: the capture and the use.
     *
     * The injected JavaScript is byte-identical to iOS's by construction, so
     * the exact same two tokens are pinned on both platforms.
     */
    @Test
    fun `document start captures TextEncoder before the page can shadow it`() {
        val bootstrap = buildLingxiV1Bootstrap("phone")
        listOf(
            "Object.defineProperty(window, '__lingxiTextEncoder', { value: window.TextEncoder });",
            "if (!window.__lingxiTextEncoder) {",
        ).forEach { token ->
            assertTrue("missing captured TextEncoder: $token", bootstrap.contains(token))
        }
        assertFalse(
            "the snapshot budget must not reach for the page-controllable global",
            buildLocalAppUiExecutionScript("""{"action":"inspect"}""")
                .contains("new TextEncoder().encode(JSON.stringify(out))"),
        )
    }

    /**
     * Final review, finding 2 — Android twin of iOS
     * `testCaptureViewClampsANegativeOriginInsteadOfRefusingIt`.
     *
     * A NEGATIVE origin is the single most common crop an agent will compute,
     * because `inspect_ui`'s `elements[].rect` reports
     * `getBoundingClientRect().top`, which is negative for anything scrolled
     * above the fold. The engine host used to refuse it outright
     * (`x < 0 || y < 0` in `capture_ui_value`), which made this clamp
     * unreachable; with that gone, the client must clamp to the viewport and
     * report the CLAMPED region.
     *
     * Same numbers as the iOS test — `(-50, -40, 200, 150)` against a
     * 393x852 viewport at density 1 clamps to `(0, 0, 150, 110)`, with BOTH
     * axes negative in the one request so a one-axis bug cannot hide behind
     * the other being right. As with the other geometry tests in this file,
     * this pins the helper `captureFrame` actually calls (no instrumented
     * harness exists for a live `WebView`/`PixelCopy` here), and reproduces
     * `finishCapture`'s own `left / density` reporting arithmetic to pin the
     * reported `capture_rect` values.
     */
    @Test
    fun `crop source rect clamps a negative origin to the viewport instead of refusing it`() {
        val density = 1f
        val src = cropSourceRect(
            LocalAppCssRect(x = -50.0, y = -40.0, width = 200.0, height = 150.0),
            density = density,
            viewWidthPx = 393,
            viewHeightPx = 852,
        )
        assertFalse("a partly-above-the-fold crop must still capture something", src.isEmpty())
        assertEquals(0, src.left)
        assertEquals(0, src.top)
        assertEquals(150, src.width()) // -50 + 200: the visible part, not the requested 200
        assertEquals(110, src.height()) // -40 + 150: the visible part, not the requested 150

        // What `finishCapture` writes into `capture_rect`, off this same
        // `localCrop` — the CLAMPED region, never the request.
        assertEquals(0.0, src.left / density.toDouble(), 0.0001)
        assertEquals(0.0, src.top / density.toDouble(), 0.0001)
        assertEquals(150.0, src.width() / density.toDouble(), 0.0001)
        assertEquals(110.0, src.height() / density.toDouble(), 0.0001)
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

    /**
     * Phase 1a twin of iOS `testCropIsCappedOnItsOwnLongEdgeAndNeverUpscaled`,
     * ported from the task-9 brief's `RectF`/`Rect`-typed snippet to
     * [LocalAppCssRect]/[LocalAppPxRect].
     *
     * That retyping is deliberate, not cosmetic: this module's unit tests run
     * against AGP's mockable `android.jar` (`isReturnDefaultValues = true`),
     * which stubs EVERY `android.graphics.Rect`/`RectF` constructor and
     * method to return the return type's default — confirmed with a
     * throwaway test before writing this one, whose printed evidence was
     * `Rect(10,20,130,100)` reading back `left=0 width=0 isEmpty=false`. The
     * numbers pinned below are the brief's own worked example, just carried
     * by a type this test environment can actually exercise: `RectF(left=10,
     * top=20, right=130, bottom=100)` is `LocalAppCssRect(x=10, y=20,
     * width=120, height=80)`.
     */
    @Test
    fun `crop uses a density-scaled source rect and is never upscaled`() {
        val src = cropSourceRect(
            LocalAppCssRect(x = 10.0, y = 20.0, width = 120.0, height = 80.0),
            density = 3f,
            viewWidthPx = 1179,
            viewHeightPx = 2556,
        )
        assertEquals(30, src.left)
        assertEquals(60, src.top)
        assertEquals(360, src.width()) // (130-10) * 3, the brief's own worked number
        assertEquals(240, src.height()) // (100-20) * 3

        val (w, h) = cropTargetSize(src, capPx = 1024)
        assertEquals(360, w) // already under the cap -> untouched
        assertEquals(240, h)

        val (bigW, _) = cropTargetSize(LocalAppPxRect(0, 0, 600, 2400), capPx = 1024)
        assertEquals(256, bigW) // 600 * (1024/2400)
    }

    /**
     * Phase 1a twin of iOS `testCaptureViewReportsCaptureRectAsTheClampedRegionNotTheRequestedOne`.
     * The iOS test drives a live `WKWebView` at 393x852 points and asserts
     * `capture_rect == {x:300,y:800,width:93,height:52}` for a
     * `{x:300,y:800,width:200,height:200}` request — chosen to extend past
     * BOTH the right edge (300+200=500 > 393) and the bottom edge
     * (800+200=1000 > 852) in the same request, so a one-axis clamp bug could
     * not hide behind the other axis being correct. This test cannot drive a
     * live WebView (no instrumented/device harness exists for this module —
     * see the task report), so it pins the same scenario one layer down, at
     * the geometry helper `captureFrame` actually calls: density=1 makes CSS
     * pixels and surface pixels coincide, so the expected numbers are
     * identical to iOS's.
     */
    @Test
    fun `crop source rect clamps a partially off-screen request to the real viewport, not the requested size`() {
        val src = cropSourceRect(
            LocalAppCssRect(x = 300.0, y = 800.0, width = 200.0, height = 200.0),
            density = 1f,
            viewWidthPx = 393,
            viewHeightPx = 852,
        )
        assertEquals(300, src.left)
        assertEquals(800, src.top)
        assertEquals(93, src.width())
        assertEquals(52, src.height())
        assertFalse("a partially off-screen crop must still capture something", src.isEmpty())
    }

    /**
     * `PixelCopy`'s source rect is `android.graphics.Rect` — INTEGER pixels
     * only; there is no float-rect overload (confirmed by reading the
     * platform's `PixelCopy.java`/`request(Window, Rect, ...)` source, not
     * assumed). At a density that is not a clean divisor of the request (2.625
     * is a real Android bucket, 420dpi), a CSS `x` of exactly 10 does not land
     * on an integer surface pixel, so the region ACTUALLY captured — and thus
     * `capture_rect`, which is read off that same pixel value — is the
     * nearest real pixel, not the mathematically exact request. This is
     * honest, not a bug: `finishCapture`'s pre-existing `viewport` field
     * already rounds the same way (`Math.round(width / density)`). It is also
     * a genuine, small (< 1/density CSS px) numeric divergence from iOS,
     * whose `CGRect`-based capture never rounds the rect itself — the
     * CONTRACT (report the actually-captured region, clamped, off the same
     * variable the capture used) still matches exactly; only the platform's
     * pixel granularity differs. See the task report's "Concerns" section.
     */
    @Test
    fun `a density that is not a clean divisor rounds the reportable crop to the nearest real pixel`() {
        val density = 2.625f
        val src = cropSourceRect(
            LocalAppCssRect(x = 10.0, y = 10.0, width = 100.0, height = 100.0),
            density = density,
            viewWidthPx = 2000,
            viewHeightPx = 2000,
        )
        assertEquals(26, src.left) // (10 * 2.625).toInt() == 26.25 -> 26, not 26.25
        val reportedX = src.left / density.toDouble()
        assertTrue(
            "must round to the nearest real pixel, within 1/density CSS px of the request",
            kotlin.math.abs(reportedX - 10.0) < (1.0 / density),
        )
        assertTrue("but it is NOT exactly the request once a real pixel boundary is involved", reportedX != 10.0)
    }

    /**
     * A rect entirely outside the viewport, on each of the four sides, must
     * clamp to an EMPTY region — the signal `captureFrame` uses to answer
     * the existing "could not be captured" failure instead of silently
     * returning the whole frame for a region nobody asked for. Mirrors iOS's
     * `testCaptureViewRejectsARectEntirelyOutsideTheViewportInsteadOfWideningIt`
     * at the geometry-helper layer, for the same live-WebView-harness reason
     * as the test above.
     */
    @Test
    fun `crop source rect collapses to empty for a request entirely outside the viewport`() {
        val viewWidthPx = 1179
        val viewHeightPx = 2556
        val density = 3f
        val toTheRight = cropSourceRect(LocalAppCssRect(500.0, 100.0, 50.0, 50.0), density, viewWidthPx, viewHeightPx)
        val toTheLeft = cropSourceRect(LocalAppCssRect(-500.0, 100.0, 50.0, 50.0), density, viewWidthPx, viewHeightPx)
        val below = cropSourceRect(LocalAppCssRect(10.0, 5000.0, 50.0, 50.0), density, viewWidthPx, viewHeightPx)
        val above = cropSourceRect(LocalAppCssRect(10.0, -5000.0, 50.0, 50.0), density, viewWidthPx, viewHeightPx)
        listOf("right" to toTheRight, "left" to toTheLeft, "below" to below, "above" to above).forEach { (label, rect) ->
            assertTrue("disjoint $label must clamp to empty, not a positive-size region", rect.isEmpty())
        }
    }

    /**
     * Phase 1a twin of iOS `testParseRequestedRectAcceptsBothIntegerAndFractionalJSONNumbers`.
     * A parser exercised only against the brief's integer example breaks on
     * a routine fractional CSS pixel — accepting both is the point of this
     * test, not an edge case.
     */
    @Test
    fun `parseRequestedCaptureRect accepts both integer and fractional JSON numbers`() {
        val fromIntegers = parseRequestedCaptureRect("""{"rect":{"x":10,"y":20,"width":120,"height":80}}""")
        assertEquals(LocalAppCssRect(10.0, 20.0, 120.0, 80.0), fromIntegers)

        val fromFloats = parseRequestedCaptureRect("""{"rect":{"x":10.5,"y":20.25,"width":120.75,"height":80.125}}""")
        assertEquals(LocalAppCssRect(10.5, 20.25, 120.75, 80.125), fromFloats)

        val mixed = parseRequestedCaptureRect("""{"rect":{"x":10,"y":20.5,"width":120,"height":80.5}}""")
        assertEquals(LocalAppCssRect(10.0, 20.5, 120.0, 80.5), mixed)
    }

    /**
     * Phase 1a twin of iOS `testParseRequestedRectReturnsNilForAbsentOrMalformedValue`.
     * Every case collapses to "no rect" (null), the same as `value == null`
     * entirely — never a thrown exception, and never a partially-populated
     * rect.
     */
    @Test
    fun `parseRequestedCaptureRect returns null for absent or malformed input`() {
        assertNull("no value at all", parseRequestedCaptureRect(null))
        assertNull("no rect key", parseRequestedCaptureRect("""{"app_id":"demo"}"""))
        assertNull("incomplete rect (missing height)", parseRequestedCaptureRect("""{"rect":{"x":1,"y":2,"width":3}}"""))
        assertNull("string-typed number", parseRequestedCaptureRect("""{"rect":{"x":"1","y":2,"width":3,"height":4}}"""))
        assertNull("boolean-typed field", parseRequestedCaptureRect("""{"rect":{"x":true,"y":2,"width":3,"height":4}}"""))
        assertNull("rect is not an object", parseRequestedCaptureRect("""{"rect":"oops"}"""))
        assertNull("explicit JSON null field", parseRequestedCaptureRect("""{"rect":{"x":null,"y":2,"width":3,"height":4}}"""))
        assertNull("non-JSON text", parseRequestedCaptureRect("not json"))
        assertNull("empty string", parseRequestedCaptureRect(""))
    }

    /** Non-finite must be rejected even though it is syntactically a `Number` once parsed. */
    @Test
    fun `parseRequestedCaptureRect rejects a non-finite number`() {
        assertNull(parseRequestedCaptureRect("""{"rect":{"x":1e400,"y":2,"width":3,"height":4}}"""))
    }

    /**
     * `CaptureView` moved from a fieldless `data object` to a `data class`
     * carrying the crop request, which turns every exhaustive `when` branch
     * that matched it by value into a compile error unless updated to `is
     * LocalAppUiAutomationAction.CaptureView`. This pins that the native/
     * script-rejection branch in `buildLocalAppUiExecutionRequest` still
     * fires correctly after that change, for a value-carrying instance.
     */
    @Test
    fun `structured script still rejects capture view natively now that it carries a value`() {
        val error = org.junit.Assert.assertThrows(IllegalStateException::class.java) {
            buildLocalAppUiExecutionRequest(
                LocalAppUiAutomationAction.CaptureView("""{"rect":{"x":1,"y":2,"width":3,"height":4}}"""),
            )
        }
        assertTrue(error.message.orEmpty().contains("Structured script is not used"))
    }
}
