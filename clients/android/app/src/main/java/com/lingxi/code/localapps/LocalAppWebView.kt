package com.lingxi.code.localapps

import android.annotation.SuppressLint
import android.content.res.Configuration
import android.graphics.Bitmap
import android.net.Uri
import android.os.Build
import android.webkit.CookieManager
import android.webkit.SafeBrowsingResponse
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.viewinterop.AndroidView
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import com.lingxi.code.BuildConfig
import com.lingxi.code.R
import org.json.JSONArray
import org.json.JSONObject
import org.json.JSONTokener
import java.lang.ref.WeakReference

data class LocalAppBridgeMessage(
    val appId: String,
    val requestId: String,
    val operation: String,
    val payloadJson: String?,
)

data class LocalAppUiTarget(
    val elementId: String? = null,
    val role: String? = null,
    val name: String? = null,
)

sealed interface LocalAppUiAutomationAction {
    data object Inspect : LocalAppUiAutomationAction
    data class Click(val target: LocalAppUiTarget) : LocalAppUiAutomationAction
    data class Fill(val target: LocalAppUiTarget, val value: String) : LocalAppUiAutomationAction
    data class Select(val target: LocalAppUiTarget, val value: String) : LocalAppUiAutomationAction
    data class Toggle(val target: LocalAppUiTarget, val checked: Boolean) : LocalAppUiAutomationAction
    data class Scroll(val x: Int, val y: Int) : LocalAppUiAutomationAction
    data class Navigate(val path: String) : LocalAppUiAutomationAction
    data object Back : LocalAppUiAutomationAction
    data object Reload : LocalAppUiAutomationAction

    /// Capture the WebView as an image. Read-only like [Inspect], but it renders
    /// what the DOM snapshot redacts, so the host prompts for it.
    data object CaptureView : LocalAppUiAutomationAction

    /// Pointer event at viewport coordinates. `Click` resolves an element and
    /// fires at (0,0); a canvas has no element and needs real coordinates.
    data class Pointer(val x: Int, val y: Int, val phase: String) : LocalAppUiAutomationAction

    /// Keyboard event. There was no key action at all before this.
    data class Key(val key: String, val phase: String) : LocalAppUiAutomationAction
}

data class LocalAppUiExecutionResult(
    val resultJson: String?,
    val error: String?,
)

private data class RawJson(val json: String)

/**
 * Executes only the versioned, structured UI action vocabulary.
 *
 * Agent input is encoded as JSON string literals and inserted into fixed host
 * scripts; callers cannot supply executable JavaScript. The controller never
 * exposes [WebView.evaluateJavascript] itself.
 */
class LocalAppWebViewController internal constructor(
    private val webView: WebView,
    private val broker: LocalAppBridgeBroker,
    private val guardedWebViewClient: WebViewClient,
    private val initialUrl: String,
) {
    private var suspendedUrl: String? = null
    private var deletionSuspended = false
    fun execute(
        action: LocalAppUiAutomationAction,
        onResult: (LocalAppUiExecutionResult) -> Unit = {},
    ) {
        when (action) {
            LocalAppUiAutomationAction.Inspect,
            is LocalAppUiAutomationAction.Click,
            is LocalAppUiAutomationAction.Fill,
            is LocalAppUiAutomationAction.Select,
            is LocalAppUiAutomationAction.Toggle,
            is LocalAppUiAutomationAction.Scroll,
            is LocalAppUiAutomationAction.Pointer,
            is LocalAppUiAutomationAction.Key -> executeStructuredAction(action, onResult)
            is LocalAppUiAutomationAction.Navigate -> {
                val current = Uri.parse(webView.url.orEmpty())
                val requested = Uri.parse(action.path)
                val target = if (requested.isAbsolute) {
                    requested
                } else {
                    current.buildUpon().encodedPath(action.path).clearQuery().build()
                }
                if (target.sameTrustedOrigin(current)) {
                    webView.loadUrl(target.toString())
                    onResult(
                        LocalAppUiExecutionResult(
                            resultJson = jsonObjectString(
                                "ok" to true,
                                "action" to "navigate",
                                "url" to target.toString(),
                            ),
                            error = null,
                        ),
                    )
                } else {
                    onResult(LocalAppUiExecutionResult(resultJson = null, error = "Only trusted loopback navigation is allowed"))
                }
            }
            LocalAppUiAutomationAction.Back -> if (webView.canGoBack()) {
                webView.goBack()
                onResult(
                    LocalAppUiExecutionResult(
                        resultJson = jsonObjectString("ok" to true, "action" to "back"),
                        error = null,
                    ),
                )
            } else {
                onResult(LocalAppUiExecutionResult(resultJson = null, error = "WebView cannot navigate back"))
            }
            LocalAppUiAutomationAction.Reload -> {
                webView.reload()
                onResult(
                    LocalAppUiExecutionResult(
                        resultJson = jsonObjectString("ok" to true, "action" to "reload"),
                        error = null,
                    ),
                )
            }
            LocalAppUiAutomationAction.CaptureView -> captureFrame(webView, onResult)
        }
    }

    /**
     * Capture the app view as a JPEG small enough to survive the result channel.
     *
     * Read off the window's composited surface with `PixelCopy`, not off a
     * canvas element and not with `View.draw(Canvas)`.
     *
     * Not a canvas read-back, because a WebGL app would need
     * `preserveDrawingBuffer`, which a generated app has to opt into and which
     * costs a frame copy on every frame it draws.
     *
     * Not `View.draw(Canvas)` either, and that distinction is the whole point of
     * this tool on Android: a `WebView` is hardware-accelerated, and drawing it
     * into a software `Canvas` takes Chromium's software path, which does not
     * rasterize hardware-composited layers — WebGL and video among them. The
     * capture came back showing the DOM chrome around a blank rectangle exactly
     * where the game is. `PixelCopy` copies the real surface, so what lands in
     * the frame is what is on the screen.
     *
     * Asynchronous because `PixelCopy` is; the dispatcher was already
     * callback-shaped, so nothing blocks a thread waiting for it.
     *
     * iOS caps `result_json` at 256 KiB and Android historically did not; the
     * budget here is the same either way, because the string still has to cross
     * the same bridge and land in the model's context. Base64 inflates by 4/3, so
     * the JPEG is stepped down until it fits rather than encoded once and hoped
     * for.
     */
    private fun captureFrame(webView: WebView, onResult: (LocalAppUiExecutionResult) -> Unit) {
        val width = webView.width
        val height = webView.height
        if (width <= 0 || height <= 0) {
            onResult(
                LocalAppUiExecutionResult(
                    resultJson = null,
                    error = "The app view could not be captured; it may be offscreen.",
                ),
            )
            return
        }
        // Cap the long edge before encoding: a 3x tablet view is several
        // megabytes of bitmap before the quality ladder ever runs.
        val maxEdge = 1_024
        val scale = if (maxOf(width, height) > maxEdge) {
            maxEdge.toFloat() / maxOf(width, height).toFloat()
        } else {
            1f
        }
        val targetWidth = maxOf(1, (width * scale).toInt())
        val targetHeight = maxOf(1, (height * scale).toInt())

        val bitmap = try {
            android.graphics.Bitmap.createBitmap(
                targetWidth,
                targetHeight,
                android.graphics.Bitmap.Config.ARGB_8888,
            )
        } catch (error: OutOfMemoryError) {
            onResult(
                LocalAppUiExecutionResult(resultJson = null, error = "Not enough memory to capture the app view"),
            )
            return
        }

        // `PixelCopy` scales the source rect into whatever bitmap it is handed,
        // so the long-edge cap above is applied by the copy itself rather than
        // by allocating a full-resolution frame first and shrinking it after.
        val window = webView.hostActivityWindow()
        if (window == null) {
            // No Activity window to copy from — a detached or test host. The
            // software draw still captures DOM chrome, which is worth more than
            // an error, and `render_check` has the canvas count to tell the
            // agent the drawn surface is the part it cannot trust.
            finishCapture(webView, bitmap, scale, softwareDraw = true, width, height, targetWidth, targetHeight, onResult)
            return
        }
        val location = IntArray(2)
        webView.getLocationInWindow(location)
        val source = android.graphics.Rect(
            location[0],
            location[1],
            location[0] + width,
            location[1] + height,
        )
        try {
            android.view.PixelCopy.request(
                window,
                source,
                bitmap,
                { status ->
                    if (status == android.view.PixelCopy.SUCCESS) {
                        finishCapture(webView, bitmap, scale, false, width, height, targetWidth, targetHeight, onResult)
                    } else {
                        finishCapture(webView, bitmap, scale, true, width, height, targetWidth, targetHeight, onResult)
                    }
                },
                android.os.Handler(android.os.Looper.getMainLooper()),
            )
        } catch (error: IllegalArgumentException) {
            // The rect can leave the window between measuring and requesting —
            // a scroll or a rotation is enough. Fall back rather than fail.
            finishCapture(webView, bitmap, scale, true, width, height, targetWidth, targetHeight, onResult)
        }
    }

    /** Unwrap the view's context to the hosting Activity's window, if there is one. */
    private fun WebView.hostActivityWindow(): android.view.Window? {
        var candidate: android.content.Context? = context
        while (candidate is android.content.ContextWrapper) {
            if (candidate is android.app.Activity) return candidate.window
            candidate = candidate.baseContext
        }
        return null
    }

    /**
     * Encode an already-populated (or still-empty) frame and answer the caller.
     *
     * [softwareDraw] paints the view into the bitmap first — the fallback path
     * for when `PixelCopy` could not run. It captures the DOM but not WebGL.
     */
    @Suppress("LongParameterList")
    private fun finishCapture(
        webView: WebView,
        bitmap: android.graphics.Bitmap,
        scale: Float,
        softwareDraw: Boolean,
        width: Int,
        height: Int,
        targetWidth: Int,
        targetHeight: Int,
        onResult: (LocalAppUiExecutionResult) -> Unit,
    ) {
        val budget = 170 * 1_024
        var encoded: ByteArray? = null
        var usedQuality = 0
        // try/finally, and the OutOfMemoryError catch widened past
        // `createBitmap`: the ladder allocates a second full-frame copy per
        // quality step (the compress buffer plus `toByteArray`), so the OOM is
        // far likelier to land HERE than on the initial allocation — and on the
        // straight-line `recycle()` this method left the bitmap alive and let
        // the error escape the caller's coroutine, so the app-ui request was
        // never resolved and the tool call died on FLOW_STEP_TIMEOUT with no
        // message at all.
        try {
            if (softwareDraw) {
                val canvas = android.graphics.Canvas(bitmap)
                if (scale != 1f) canvas.scale(scale, scale)
                webView.draw(canvas)
            }
            for (quality in intArrayOf(70, 50, 30)) {
                val stream = java.io.ByteArrayOutputStream()
                if (!bitmap.compress(android.graphics.Bitmap.CompressFormat.JPEG, quality, stream)) continue
                usedQuality = quality
                encoded = stream.toByteArray()
                if (encoded.size <= budget) break
            }
        } catch (error: OutOfMemoryError) {
            onResult(
                LocalAppUiExecutionResult(resultJson = null, error = "Not enough memory to capture the app view"),
            )
            return
        } finally {
            bitmap.recycle()
        }
        val bytes = encoded
        if (bytes == null || bytes.size > budget) {
            onResult(
                LocalAppUiExecutionResult(
                    resultJson = null,
                    error = "The captured frame is too large to return, even at reduced quality.",
                ),
            )
            return
        }

        val image = org.json.JSONObject()
            .put("data", android.util.Base64.encodeToString(bytes, android.util.Base64.NO_WRAP))
            .put("mime_type", "image/jpeg")
            // The frame's OWN pixel size. Without it the agent cannot turn a
            // feature it sees in the image into a `pointer` coordinate, because
            // `pointer` is in CSS pixels and the frame was downscaled by an
            // amount nothing reported — so it guesses, the tap lands elsewhere,
            // and the call still answers ok:true.
            .put("width", targetWidth)
            .put("height", targetHeight)
        // Viewport + density travel with the frame: the same app is a different
        // layout on a tablet in landscape, and the pixels alone do not say which.
        //
        // Reported in CSS PIXELS, not `View` pixels. This number is the only
        // size the agent has when it computes a `pointer` coordinate, and that
        // action is documented — and implemented, via `elementFromPoint` — in
        // CSS pixels. `View.getWidth()` is physical pixels, so reporting it raw
        // handed the agent a viewport `density`x too large on every Android
        // device: a centre tap landed off-screen, `elementFromPoint` returned
        // null, the event went to `document.body`, and the call still answered
        // ok:true. iOS reports `bounds` in points, which already IS CSS pixels.
        val density = webView.resources.displayMetrics.density
        val viewport = org.json.JSONObject()
            .put("width", Math.round(width / density))
            .put("height", Math.round(height / density))
        val payload = org.json.JSONObject()
            .put("ok", true)
            .put("action", "capture_view")
            .put("image", image)
            .put("viewport", viewport)
            .put("device_pixel_ratio", density.toDouble())
            .put("jpeg_quality", usedQuality / 100.0)
            // Says which path produced these pixels. A software-draw frame is
            // blind to WebGL, so an agent looking at a blank game board needs to
            // be able to tell "the app is broken" from "this capture cannot see
            // it" — otherwise it reports render_check failed and burns a repair
            // round on an app that renders correctly.
            .put("capture_path", if (softwareDraw) "software_draw" else "pixel_copy")
        onResult(LocalAppUiExecutionResult(resultJson = payload.toString(), error = null))
    }

    fun resolveBridgeRequest(
        requestId: String,
        ok: Boolean,
        resultJson: String?,
        error: String?,
        errorCode: String?,
    ) {
        broker.resolve(
            requestId = requestId,
            resultJson = resultJson,
            error = if (ok) null else error ?: "Bridge request failed",
            errorCode = errorCode.takeUnless { ok },
        )
    }

    fun deliverStreamFrame(frameJson: String) {
        webView.evaluateJavascript(
            "window.lingxi?.__stream(JSON.parse(${jsonStringLiteral(frameJson)}));",
            null,
        )
    }

    internal fun detach() {
        broker.failAllInFlight()
        webView.stopLoading()
        WebViewCompat.removeWebMessageListener(webView, LINGXI_V1_MESSAGE_OBJECT)
        // Replace the document immediately so a page cannot keep reading or
        // mutating its origin while Rust processes the subsequent DeleteApp.
        // The durable origin purge still happens only after AppsChanged proves
        // the app record is gone.
        webView.webViewClient = WebViewClient()
        webView.loadUrl("about:blank")
    }

    internal fun suspendForDeletion() {
        if (deletionSuspended) return
        deletionSuspended = true
        suspendedUrl = webView.url?.takeIf { Uri.parse(it).isTrustedLoopback() } ?: initialUrl
        broker.failAllInFlight()
        webView.stopLoading()
        // about:blank must not be rejected by the normal loopback-only client.
        // Keep the broker listener registered: its exact-origin rule excludes
        // the blank document, and retaining it permits a safe resume if command
        // submission fails.
        webView.webViewClient = WebViewClient()
        webView.loadUrl("about:blank")
    }

    internal fun resumeAfterFailedDeletion() {
        if (!deletionSuspended) return
        deletionSuspended = false
        val url = suspendedUrl ?: initialUrl
        suspendedUrl = null
        webView.webViewClient = guardedWebViewClient
        webView.loadUrl(url)
    }

    private fun executeStructuredAction(
        action: LocalAppUiAutomationAction,
        onResult: (LocalAppUiExecutionResult) -> Unit,
    ) {
        val requestJson = buildLocalAppUiExecutionRequest(action)
        fixedScript(buildLocalAppUiExecutionScript(requestJson)) { rawResult ->
            onResult(parseLocalAppUiExecutionResult(rawResult))
        }
    }

    private fun fixedScript(script: String, onResult: (String?) -> Unit) {
        webView.evaluateJavascript(script, onResult)
    }
}

internal fun buildLocalAppUiExecutionRequest(action: LocalAppUiAutomationAction): String {
    return when (action) {
        LocalAppUiAutomationAction.Inspect -> jsonObjectString("action" to "inspect")
        is LocalAppUiAutomationAction.Click -> {
            jsonObjectString(
                "action" to "click",
                "target" to RawJson(action.target.toJsonString()),
            )
        }
        is LocalAppUiAutomationAction.Fill -> {
            jsonObjectString(
                "action" to "fill",
                "target" to RawJson(action.target.toJsonString()),
                "value" to action.value,
            )
        }
        is LocalAppUiAutomationAction.Select -> {
            jsonObjectString(
                "action" to "select",
                "target" to RawJson(action.target.toJsonString()),
                "value" to action.value,
            )
        }
        is LocalAppUiAutomationAction.Toggle -> {
            jsonObjectString(
                "action" to "toggle",
                "target" to RawJson(action.target.toJsonString()),
                "checked" to action.checked,
            )
        }
        is LocalAppUiAutomationAction.Scroll -> {
            jsonObjectString(
                "action" to "scroll",
                "x" to action.x,
                "y" to action.y,
            )
        }
        is LocalAppUiAutomationAction.Pointer -> {
            jsonObjectString(
                "action" to "pointer",
                "x" to action.x,
                "y" to action.y,
                "phase" to action.phase,
            )
        }
        is LocalAppUiAutomationAction.Key -> {
            jsonObjectString(
                "action" to "key",
                "key" to action.key,
                "phase" to action.phase,
            )
        }
        is LocalAppUiAutomationAction.Navigate,
        LocalAppUiAutomationAction.Back,
        LocalAppUiAutomationAction.Reload,
        // Handled natively (`captureFrame`) — a screenshot cannot be produced by
        // injected script, which is the whole reason it captures the composited
        // view rather than reading back a canvas.
        LocalAppUiAutomationAction.CaptureView -> error("Structured script is not used for $action")
    }
}

internal fun buildLocalAppUiExecutionScript(requestJson: String): String =
    """
    (() => {
      const request = $requestJson;
      const clean = value => String(value ?? '').replace(/\s+/g, ' ').trim().slice(0, 500);
      const roleOf = element => clean(element.getAttribute('role') || ({
        BUTTON: 'button',
        A: 'link',
        INPUT: ['button', 'submit', 'reset'].includes((element.type || '').toLowerCase())
          ? 'button'
          : element.type === 'checkbox'
            ? 'checkbox'
            : (element.type === 'radio' ? 'radio' : 'textbox'),
        SELECT: 'combobox',
        TEXTAREA: 'textbox'
      })[element.tagName] || '');
      const nameOf = element => {
        const labelledBy = clean(element.getAttribute('aria-labelledby'));
        const labelled = labelledBy
          ? labelledBy.split(/\s+/).map(id => document.getElementById(id)).find(Boolean)
          : null;
        const explicit = element.id
          ? document.querySelector(`label[for="${'$'}{CSS.escape(element.id)}"]`)
          : null;
        return clean(
          element.getAttribute('aria-label')
            || labelled?.textContent
            || explicit?.textContent
            || element.placeholder
            || element.innerText
            || element.value
        );
      };
      const candidates = () => Array.from(document.querySelectorAll(
        'button,a[href],input,select,textarea,[role],[tabindex],[contenteditable="true"]'
      ));
      const findTarget = target => {
        if (!target) return null;
        if (target.elementId) {
          const byId = document.getElementById(target.elementId);
          if (byId) return byId;
        }
        return candidates().find(element =>
          (!target.role || roleOf(element).toLowerCase() === clean(target.role).toLowerCase()) &&
          (!target.name || nameOf(element).toLowerCase() === clean(target.name).toLowerCase())
        ) || null;
      };
      const dispatchValueChange = element => {
        element.dispatchEvent(new Event('input', { bubbles: true }));
        element.dispatchEvent(new Event('change', { bubbles: true }));
      };
      const setNativeValue = (element, value) => {
        const prototype = element instanceof HTMLTextAreaElement
          ? HTMLTextAreaElement.prototype
          : element instanceof HTMLSelectElement
            ? HTMLSelectElement.prototype
            : HTMLInputElement.prototype;
        const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set;
        if (!setter) throw new Error('Target value cannot be changed');
        setter.call(element, value);
      };
      const encode = result => JSON.stringify({ ok: true, result });
      const fail = message => JSON.stringify({
        ok: false,
        error: clean(message) || 'Lingxi UI action failed'
      });
      const snapshot = () => ({
        title: clean(document.title),
        url: location.href,
        // A `<canvas>` matches NONE of the selectors `candidates()` uses, so a
        // drawn interface is invisible in `elements` — an empty list means
        // the same thing whether the app renders correctly, renders nothing,
        // or crashed. Reporting the count separately is what lets the
        // verifier tell "no controls" apart from "cannot be seen this way",
        // and it does not depend on the DOM being empty: a canvas game with
        // a score bar and a restart button still needs its frame looked at.
        canvasCount: document.querySelectorAll('canvas').length,
        elements: candidates().slice(0, 200).map(element => {
          const rect = element.getBoundingClientRect();
          const sensitive = element instanceof HTMLInputElement && ['hidden', 'password'].includes(element.type);
          return {
            elementId: clean(element.id) || null,
            role: roleOf(element) || null,
            name: nameOf(element) || null,
            value: sensitive ? null : clean('value' in element ? element.value : null),
            checked: typeof element.checked === 'boolean' ? element.checked : null,
            disabled: !!element.disabled,
            visible: rect.width > 0 && rect.height > 0
          };
        })
      });
      try {
        if (request.action === 'inspect') return encode(snapshot());
        if (request.action === 'scroll') {
          window.scrollBy({ left: Number(request.x || 0), top: Number(request.y || 0), behavior: 'auto' });
          return encode({ ok: true, action: 'scroll', x: Number(request.x || 0), y: Number(request.y || 0) });
        }
        // `pointer` and `key` resolve NO element, so they return before the
        // findTarget block — a canvas app has nothing for it to find, which is
        // exactly why these two actions exist.
        if (request.action === 'pointer') {
          const x = Number(request.x);
          const y = Number(request.y);
          const phase = String(request.phase || 'tap');
          if (!Number.isFinite(x) || !Number.isFinite(y)) throw new Error('pointer needs x and y in CSS pixels');
          // Validated HERE, not swallowed in Kotlin: the two ports mirror one
          // wire contract, and a phase the agent got wrong must fail the same
          // way on both. Coercing it to 'tap' turned an intended hold into a
          // tap-and-release that still reported ok:true.
          if (!['tap', 'down', 'move', 'up'].includes(phase)) throw new Error('pointer phase must be tap, down, move or up');
          // A null hit means the point is OUTSIDE the viewport — say so instead
          // of quietly retargeting to body, which dispatched a tap that reached
          // nothing and still answered ok:true, so the agent recorded an
          // interaction that never happened.
          const receiver = document.elementFromPoint(x, y);
          if (!receiver) throw new Error('pointer ' + x + ',' + y + ' is outside the ' + Math.round(window.innerWidth) + 'x' + Math.round(window.innerHeight) + ' CSS-pixel viewport');
          const base = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, pointerId: 1, pointerType: 'touch', isPrimary: true, button: 0, buttons: 1 };
          const fire = (type, overrides) => {
            const init = Object.assign({}, base, overrides || {});
            receiver.dispatchEvent(new PointerEvent(type, init));
            const mouseType = type === 'pointerdown' ? 'mousedown' : type === 'pointerup' ? 'mouseup' : 'mousemove';
            receiver.dispatchEvent(new MouseEvent(mouseType, init));
          };
          // `buttons` on a move must say whether a button is HELD. A drag is
          // driven as down -> move -> up, and canvas/slider handlers almost
          // universally open with `if (!e.buttons) return;` — a move that always
          // reports 0 is a hover, so every drag registered as a click at the
          // start point with no travel.
          if (phase === 'down') window.__lingxiPointerHeld = true;
          const held = window.__lingxiPointerHeld ? 1 : 0;
          if (phase === 'move') fire('pointermove', { buttons: held });
          else if (phase === 'down') fire('pointerdown');
          else if (phase === 'up') { fire('pointerup', { buttons: 0 }); window.__lingxiPointerHeld = false; }
          else {
            fire('pointerdown');
            fire('pointerup', { buttons: 0 });
            receiver.dispatchEvent(new MouseEvent('click', Object.assign({}, base, { buttons: 0 })));
            window.__lingxiPointerHeld = false;
          }
          return encode({ ok: true, action: 'pointer', phase: phase, x: x, y: y, receiver: receiver.tagName || null });
        }
        if (request.action === 'key') {
          const named = String(request.key || '');
          const phase = String(request.phase || 'press');
          if (!named) throw new Error('key needs a DOM key name, e.g. ArrowLeft');
          // SPACE: the DOM key name is a single space, which cannot survive the
          // wire — the value is trimmed on the way in, so " " arrives empty and
          // used to be rejected. `Space` (the code name) is the only spelling
          // that gets here; translate it back to the real key.
          const key = named === 'Space' ? ' ' : named;
          // Falling back to `document.body` reached NOTHING for the case this
          // action exists for: a synthetic pointer does not move focus, so after
          // tapping a canvas `activeElement` is still body — and an event
          // dispatched ON body propagates UP, never down into the canvas, so a
          // canvas-scoped keydown listener never fired while the call still
          // answered ok:true. Dispatching on the canvas instead reaches
          // listeners at every level (canvas -> body -> document -> window).
          const receiver = document.activeElement && document.activeElement !== document.body
            ? document.activeElement
            : (document.querySelector('canvas') || document.body);
          // Ordered so the space case is reached: a space HAS length 1, so
          // testing it after the length check made that arm dead code and
          // emitted `code: ''`, which `e.code === 'Space'` never matches.
          const code = key === ' '
            ? 'Space'
            : key.length === 1
            ? (/[a-z]/i.test(key) ? 'Key' + key.toUpperCase() : /[0-9]/.test(key) ? 'Digit' + key : '')
            : key;
          const init = { key: key, code: code, bubbles: true, cancelable: true, composed: true };
          if (phase === 'down' || phase === 'press') receiver.dispatchEvent(new KeyboardEvent('keydown', init));
          if (phase === 'up' || phase === 'press') receiver.dispatchEvent(new KeyboardEvent('keyup', init));
          return encode({ ok: true, action: 'key', key: key, phase: phase });
        }
        const element = findTarget(request.target);
        if (!element) throw new Error('UI target was not found');
        if (element.disabled) throw new Error('UI target is disabled');
        if (request.action === 'click') {
          element.click();
        } else if (request.action === 'fill') {
          if (!(element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement || element.isContentEditable)) {
            throw new Error('Target cannot be filled');
          }
          if (element.isContentEditable) {
            element.textContent = request.value || '';
          } else {
            setNativeValue(element, request.value || '');
          }
          dispatchValueChange(element);
        } else if (request.action === 'select') {
          if (!(element instanceof HTMLSelectElement)) throw new Error('Target is not a select element');
          const option = Array.from(element.options).find(item =>
            item.value === request.value || clean(item.textContent) === clean(request.value)
          );
          if (!option) throw new Error('Select option was not found');
          setNativeValue(element, option.value);
          dispatchValueChange(element);
        } else if (request.action === 'toggle') {
          const desired = !!request.checked;
          if (element instanceof HTMLInputElement && (element.type === 'checkbox' || element.type === 'radio')) {
            if (!!element.checked !== desired) element.click();
          } else if (['checkbox', 'switch'].includes(roleOf(element).toLowerCase())) {
            const current = clean(element.getAttribute('aria-checked')).toLowerCase() === 'true';
            if (current !== desired) element.click();
          } else {
            throw new Error('Target is not toggleable');
          }
        } else {
          throw new Error('Unsupported UI action');
        }
        return encode({
          ok: true,
          action: request.action,
          target: {
            elementId: clean(element.id) || null,
            role: roleOf(element) || null,
            name: nameOf(element) || null
          }
        });
      } catch (error) {
        return fail(error && error.message ? error.message : String(error));
      }
    })()
    """.trimIndent()

internal fun parseLocalAppUiExecutionResult(rawResult: String?): LocalAppUiExecutionResult {
    val trimmed = rawResult?.trim().orEmpty()
    if (trimmed.isEmpty() || trimmed == "null" || trimmed == "false" || trimmed == "undefined") {
        return LocalAppUiExecutionResult(resultJson = null, error = "WebView UI automation returned no result")
    }
    val envelopeText = (if (trimmed.startsWith('"')) decodeJsonStringLiteral(trimmed) else trimmed)
        ?: return LocalAppUiExecutionResult(resultJson = null, error = "WebView UI automation returned malformed payload")
    val okField = extractTopLevelJsonField(envelopeText, "ok")
        ?: return LocalAppUiExecutionResult(resultJson = null, error = "WebView UI automation returned malformed payload")
    if (okField == "false") {
        return LocalAppUiExecutionResult(
            resultJson = null,
            error = extractTopLevelJsonField(envelopeText, "error")
                ?.takeIf { it.startsWith('"') }
                ?.let(::decodeJsonStringLiteral)
                ?.takeIf { it.isNotBlank() }
                ?: "WebView UI automation failed",
        )
    }
    if (okField != "true") {
        return LocalAppUiExecutionResult(resultJson = null, error = "WebView UI automation returned malformed payload")
    }
    val resultField = extractTopLevelJsonField(envelopeText, "result")
        ?: return LocalAppUiExecutionResult(resultJson = null, error = "WebView UI automation returned no result")
    if (resultField == "null") {
        return LocalAppUiExecutionResult(resultJson = null, error = "WebView UI automation returned no result")
    }
    return LocalAppUiExecutionResult(resultJson = resultField, error = null)
}

private fun LocalAppUiTarget.toJsonString(): String =
    jsonObjectString(
        "elementId" to elementId,
        "role" to role,
        "name" to name,
    )

private fun jsonObjectString(vararg entries: Pair<String, Any?>): String = buildString {
    append('{')
    var first = true
    for ((key, value) in entries) {
        if (value == null || value == JSONObject.NULL) continue
        if (!first) append(',')
        first = false
        append(jsonStringLiteral(key))
        append(':')
        append(jsonValueToJson(value))
    }
    append('}')
}

private fun jsonValueToJson(value: Any?): String = when (value) {
    null,
    JSONObject.NULL -> "null"
    is RawJson -> value.json
    is JSONObject -> value.toString()
    is JSONArray -> value.toString()
    is Number,
    is Boolean -> value.toString()
    is String -> jsonStringLiteral(value)
    else -> JSONObject.wrap(value)?.toString() ?: jsonStringLiteral(value.toString())
}

private fun jsonStringLiteral(value: String): String = buildString(value.length + 2) {
    append('"')
    value.forEach { ch ->
        when (ch) {
            '\\' -> append("\\\\")
            '"' -> append("\\\"")
            '\b' -> append("\\b")
            '\u000C' -> append("\\f")
            '\n' -> append("\\n")
            '\r' -> append("\\r")
            '\t' -> append("\\t")
            else -> if (ch.code < 0x20 || ch == '\u2028' || ch == '\u2029') {
                append("\\u%04x".format(ch.code))
            } else {
                append(ch)
            }
        }
    }
    append('"')
}

private fun decodeJsonStringLiteral(value: String): String? {
    if (value.length < 2 || value.first() != '"' || value.last() != '"') return null
    val out = StringBuilder(value.length - 2)
    var index = 1
    while (index < value.lastIndex) {
        val ch = value[index++]
        if (ch != '\\') {
            out.append(ch)
            continue
        }
        if (index >= value.lastIndex + 1) return null
        when (val escaped = value[index++]) {
            '"', '\\', '/' -> out.append(escaped)
            'b' -> out.append('\b')
            'f' -> out.append('\u000C')
            'n' -> out.append('\n')
            'r' -> out.append('\r')
            't' -> out.append('\t')
            'u' -> {
                if (index + 4 > value.length) return null
                val hex = value.substring(index, index + 4)
                out.append(hex.toIntOrNull(16)?.toChar() ?: return null)
                index += 4
            }
            else -> return null
        }
    }
    return out.toString()
}

private fun extractTopLevelJsonField(json: String, fieldName: String): String? {
    val text = json.trim()
    if (text.length < 2 || text.first() != '{' || text.last() != '}') return null
    var index = 1
    while (index < text.lastIndex) {
        index = skipJsonWhitespace(text, index)
        if (index >= text.lastIndex) break
        if (text[index] == ',') {
            index++
            continue
        }
        if (text[index] != '"') return null
        val keyEnd = findJsonStringEnd(text, index) ?: return null
        val key = decodeJsonStringLiteral(text.substring(index, keyEnd + 1)) ?: return null
        index = skipJsonWhitespace(text, keyEnd + 1)
        if (index >= text.lastIndex || text[index] != ':') return null
        index = skipJsonWhitespace(text, index + 1)
        val valueEnd = findJsonValueEnd(text, index) ?: return null
        if (key == fieldName) return text.substring(index, valueEnd)
        index = valueEnd
    }
    return null
}

private fun skipJsonWhitespace(text: String, start: Int): Int {
    var index = start
    while (index < text.length && text[index].isWhitespace()) index++
    return index
}

private fun findJsonStringEnd(text: String, start: Int): Int? {
    var index = start + 1
    while (index < text.length) {
        when (text[index]) {
            '\\' -> index += 2
            '"' -> return index
            else -> index++
        }
    }
    return null
}

private fun findJsonValueEnd(text: String, start: Int): Int? {
    if (start >= text.length) return null
    return when (text[start]) {
        '"' -> findJsonStringEnd(text, start)?.plus(1)
        '{', '[' -> {
            val open = text[start]
            val close = if (open == '{') '}' else ']'
            var depth = 0
            var index = start
            while (index < text.length) {
                when (text[index]) {
                    '"' -> {
                        index = findJsonStringEnd(text, index) ?: return null
                    }
                    open -> depth++
                    close -> {
                        depth--
                        if (depth == 0) return index + 1
                    }
                }
                index++
            }
            null
        }
        else -> {
            var index = start
            while (index < text.length && text[index] != ',' && text[index] != '}' && text[index] != ']') {
                index++
            }
            index
        }
    }
}

internal const val LOCAL_APP_BRIDGE_MAX_TEXT_LENGTH = 128
internal const val LOCAL_APP_BRIDGE_MAX_CONTROL_BYTES = 64 * 1024
// Long model inputs are data, not control traffic. Eight MiB accommodates a
// typical one-million-token text context plus JSON framing without turning
// every bridge operation into an unbounded WebView IPC surface.
internal const val LOCAL_APP_BRIDGE_MAX_LLM_BYTES = 8 * 1024 * 1024
internal const val LOCAL_APP_BRIDGE_MAX_IN_FLIGHT = 128

internal fun localAppBridgeByteLimit(operation: String): Int =
    if (operation == "llm_chat" || operation == "llm_stream") {
        LOCAL_APP_BRIDGE_MAX_LLM_BYTES
    } else if (operation == "file_read" || operation == "file_write") {
        4 * 1024 * 1024
    } else {
        LOCAL_APP_BRIDGE_MAX_CONTROL_BYTES
    }

internal sealed interface LocalAppBridgeIngress {
    data class Accepted(val message: LocalAppBridgeMessage) : LocalAppBridgeIngress
    data class Rejected(
        val requestId: String?,
        val message: String,
        val code: String,
    ) : LocalAppBridgeIngress
}

/** Pure validation seam used by the real WebMessageListener and JVM tests. */
internal fun parseLocalAppBridgeMessage(
    appId: String,
    rawMessage: String,
    inFlightRequestIds: Set<String>,
): LocalAppBridgeIngress {
    val requestBytes = rawMessage.toByteArray(Charsets.UTF_8).size
    // Reject truly oversized input before doing any structural parsing. The
    // operation-specific (usually much smaller) limit is applied below once
    // the operation name has been validated.
    if (requestBytes > LOCAL_APP_BRIDGE_MAX_LLM_BYTES) {
        return LocalAppBridgeIngress.Rejected(
            null,
            "Bridge request is $requestBytes bytes; the absolute limit is $LOCAL_APP_BRIDGE_MAX_LLM_BYTES",
            "request_too_large",
        )
    }
    val requestId = extractTopLevelJsonField(rawMessage, "requestId")
        ?.takeIf { it.startsWith('"') }
        ?.let(::decodeJsonStringLiteral)
        ?.takeIf { it.isNotBlank() }
        ?: return LocalAppBridgeIngress.Rejected(null, "Bridge requestId is required", "request_id_invalid")
    if (requestId.length > LOCAL_APP_BRIDGE_MAX_TEXT_LENGTH) {
        return LocalAppBridgeIngress.Rejected(
            requestId,
            "Bridge requestId exceeds $LOCAL_APP_BRIDGE_MAX_TEXT_LENGTH characters",
            "request_id_invalid",
        )
    }
    val operation = extractTopLevelJsonField(rawMessage, "operation")
        ?.takeIf { it.startsWith('"') }
        ?.let(::decodeJsonStringLiteral)
        ?.takeIf { it.isNotBlank() }
        ?: return LocalAppBridgeIngress.Rejected(requestId, "Bridge operation is required", "operation_invalid")
    if (operation.length > LOCAL_APP_BRIDGE_MAX_TEXT_LENGTH) {
        return LocalAppBridgeIngress.Rejected(
            requestId,
            "Bridge operation exceeds $LOCAL_APP_BRIDGE_MAX_TEXT_LENGTH characters",
            "operation_invalid",
        )
    }
    val byteLimit = localAppBridgeByteLimit(operation)
    if (requestBytes > byteLimit) {
        return LocalAppBridgeIngress.Rejected(
            requestId,
            "Bridge request is $requestBytes bytes; the limit for $operation is $byteLimit",
            "request_too_large",
        )
    }
    if (requestId in inFlightRequestIds) {
        return LocalAppBridgeIngress.Rejected(requestId, "Bridge requestId is already in flight", "duplicate_request_id")
    }
    if (inFlightRequestIds.size >= LOCAL_APP_BRIDGE_MAX_IN_FLIGHT) {
        return LocalAppBridgeIngress.Rejected(
            requestId,
            "Bridge has too many outstanding requests",
            "too_many_requests",
        )
    }
    val payloadJson = extractTopLevelJsonField(rawMessage, "payload")?.trim() ?: "{}"
    if (!payloadJson.startsWith('{') || !payloadJson.endsWith('}')) {
        return LocalAppBridgeIngress.Rejected(
            requestId,
            "Bridge payload must be a JSON object",
            "payload_invalid",
        )
    }
    val payloadBytes = payloadJson.toByteArray(Charsets.UTF_8).size
    if (payloadBytes > byteLimit) {
        return LocalAppBridgeIngress.Rejected(
            requestId,
            "Bridge payload is $payloadBytes bytes; the limit for $operation is $byteLimit",
            "payload_too_large",
        )
    }
    return LocalAppBridgeIngress.Accepted(
        LocalAppBridgeMessage(
            appId = appId,
            requestId = requestId,
            operation = operation,
            payloadJson = payloadJson,
        ),
    )
}

internal class LocalAppBridgeBroker(
    private val appId: String,
    private val trustedOrigin: Uri,
    private val webView: WebView,
    private val onMessage: (LocalAppBridgeMessage) -> Unit,
) {
    private val inFlightRequestIds = linkedSetOf<String>()

    fun receive(rawMessage: String, sourceOrigin: Uri, isMainFrame: Boolean) {
        if (!isMainFrame || !sourceOrigin.sameTrustedOrigin(trustedOrigin)) return
        when (val ingress = parseLocalAppBridgeMessage(appId, rawMessage, inFlightRequestIds)) {
            is LocalAppBridgeIngress.Accepted -> {
                inFlightRequestIds += ingress.message.requestId
                onMessage(ingress.message)
            }
            is LocalAppBridgeIngress.Rejected -> ingress.requestId?.let { requestId ->
                // Admission failures were never inserted. In particular, a
                // duplicate must not evict the original request while its
                // Rust operation is still in flight.
                rejectUntracked(requestId, ingress.message, ingress.code)
            }
        }
    }

    fun resolve(requestId: String, resultJson: String?, error: String?, errorCode: String?) {
        inFlightRequestIds.remove(requestId)
        val result = resultJson?.let { encoded ->
            runCatching {
                val tokener = JSONTokener(encoded)
                val value = tokener.nextValue()
                require(tokener.nextClean() == '\u0000') { "trailing JSON content" }
                value
            }.getOrElse {
                sendResolution(
                    requestId = requestId,
                    result = JSONObject.NULL,
                    error = "Bridge returned invalid JSON",
                    errorCode = "result_invalid",
                )
                return
            }
        } ?: JSONObject.NULL
        sendResolution(requestId, result, error, errorCode)
    }

    private fun rejectUntracked(requestId: String, error: String, errorCode: String) {
        sendResolution(requestId, JSONObject.NULL, error, errorCode)
    }

    fun failAllInFlight() {
        val outstanding = inFlightRequestIds.toList()
        inFlightRequestIds.clear()
        outstanding.forEach { requestId ->
            sendResolution(
                requestId = requestId,
                result = JSONObject.NULL,
                error = "The Lingxi bridge was detached",
                errorCode = "bridge_detached",
            )
        }
    }

    private fun sendResolution(requestId: String, result: Any, error: String?, errorCode: String?) {
        val envelope = JSONObject().apply {
            put("requestId", requestId)
            put("result", result)
            put("error", error ?: JSONObject.NULL)
            put("code", errorCode ?: JSONObject.NULL)
        }
        webView.evaluateJavascript(
            "window.lingxi?.__resolve(JSON.parse(${jsonStringLiteral(envelope.toString())}));",
            null,
        )
    }
}

/** Active controllers are host-owned and keyed by the bound app id, never by page input. */
internal object LocalAppWebViewRegistry {
    private val controllers = mutableMapOf<String, WeakReference<LocalAppWebViewController>>()

    @Synchronized
    fun register(appId: String, controller: LocalAppWebViewController) {
        controllers.put(appId, WeakReference(controller))?.get()?.detach()
    }

    @Synchronized
    fun unregister(appId: String, controller: LocalAppWebViewController) {
        if (controllers[appId]?.get() === controller) controllers.remove(appId)
    }

    @Synchronized
    fun detach(appId: String) {
        controllers.remove(appId)?.get()?.detach()
    }

    @Synchronized
    fun suspendForDeletion(appId: String) {
        controllers[appId]?.get()?.suspendForDeletion()
    }

    @Synchronized
    fun resumeAfterFailedDeletion(appId: String) {
        controllers[appId]?.get()?.resumeAfterFailedDeletion()
    }

    @Synchronized
    fun deliverStreamFrame(appId: String, frameJson: String) {
        controllers[appId]?.get()?.deliverStreamFrame(frameJson)
    }
}

@SuppressLint("SetJavaScriptEnabled")
@Composable
fun LocalAppWebView(
    appId: String,
    url: String,
    onExternalNavigation: (String) -> Unit,
    modifier: Modifier = Modifier,
    onBridgeRequest: (LocalAppBridgeMessage) -> Unit = {},
    onControllerReady: (LocalAppWebViewController) -> Unit = {},
) {
    var pendingExternalUrl by remember(appId) { mutableStateOf<String?>(null) }
    var webView by remember(appId) { mutableStateOf<WebView?>(null) }
    var controller by remember(appId) { mutableStateOf<LocalAppWebViewController?>(null) }
    val currentBridgeHandler by rememberUpdatedState(onBridgeRequest)
    val currentControllerHandler by rememberUpdatedState(onControllerReady)
    val trustedOrigin = remember(url) { Uri.parse(url).takeIf { it.isTrustedLoopback() } }
    val bridgeSupported = remember {
        WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT) &&
            WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)
    }

    if (!bridgeSupported) {
        Text(
            text = stringResource(R.string.local_apps_webview_upgrade_required),
            modifier = modifier,
        )
        return
    }
    if (trustedOrigin == null) {
        Text(
            text = stringResource(R.string.local_apps_webview_untrusted_origin),
            modifier = modifier,
        )
        return
    }
    val trustedOriginRule = trustedOrigin.toTrustedOriginString()
    val applicationContext = LocalContext.current.applicationContext
    var storageGate by remember(appId, trustedOriginRule) {
        mutableStateOf(LocalAppWebStorageGate.Waiting)
    }
    LaunchedEffect(appId, trustedOriginRule, applicationContext) {
        storageGate = LocalAppWebStorageGate.Waiting
        storageGate = if (
            AndroidLocalAppWebStorageCleanup.get(applicationContext)
                .awaitOriginReadyAndRemember(appId, trustedOriginRule)
        ) {
            LocalAppWebStorageGate.Ready
        } else {
            LocalAppWebStorageGate.Failed
        }
    }
    when (storageGate) {
        LocalAppWebStorageGate.Waiting -> {
            Text(stringResource(R.string.local_apps_webview_storage_cleanup_waiting), modifier = modifier)
            return
        }
        LocalAppWebStorageGate.Failed -> {
            Text(stringResource(R.string.local_apps_webview_storage_cleanup_failed), modifier = modifier)
            return
        }
        LocalAppWebStorageGate.Ready -> Unit
    }

    AndroidView(
        modifier = modifier,
        factory = { context ->
            WebView(context).apply webView@ {
                webView = this
                WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)
                settings.javaScriptEnabled = true
                settings.domStorageEnabled = true
                settings.allowFileAccess = false
                settings.allowContentAccess = false
                @Suppress("DEPRECATION")
                settings.allowFileAccessFromFileURLs = false
                @Suppress("DEPRECATION")
                settings.allowUniversalAccessFromFileURLs = false
                settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
                settings.safeBrowsingEnabled = true
                CookieManager.getInstance().apply {
                    setAcceptCookie(false)
                    setAcceptThirdPartyCookies(this@webView, false)
                }
                val broker = LocalAppBridgeBroker(
                    appId = appId,
                    trustedOrigin = trustedOrigin,
                    webView = this,
                    onMessage = { currentBridgeHandler(it) },
                )
                WebViewCompat.addWebMessageListener(
                    this,
                    LINGXI_V1_MESSAGE_OBJECT,
                    setOf(trustedOriginRule),
                ) { _, message, sourceOrigin, isMainFrame, _ ->
                    broker.receive(message.data.orEmpty(), sourceOrigin, isMainFrame)
                }
                WebViewCompat.addDocumentStartJavaScript(
                    this,
                    buildLingxiV1Bootstrap(androidFormFactor(context.resources.configuration)),
                    setOf(trustedOriginRule),
                )
                val guardedClient = object : WebViewClient() {
                    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                        val target = request.url
                        if (target.sameTrustedOrigin(trustedOrigin)) return false
                        if (request.isForMainFrame) pendingExternalUrl = target.toString()
                        return true
                    }

                    override fun onPageStarted(view: WebView, url: String, favicon: Bitmap?) {
                        if (!Uri.parse(url).sameTrustedOrigin(trustedOrigin)) view.stopLoading()
                    }

                    override fun onSafeBrowsingHit(
                        view: WebView,
                        request: WebResourceRequest,
                        threatType: Int,
                        callback: SafeBrowsingResponse,
                    ) {
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
                            callback.backToSafety(true)
                        }
                    }

                    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                        if (!request.url.sameTrustedOrigin(trustedOrigin)) {
                            return WebResourceResponse("text/plain", "utf-8", 403, "Blocked", emptyMap(), null)
                        }
                        return super.shouldInterceptRequest(view, request)
                    }
                }
                webViewClient = guardedClient
                controller = LocalAppWebViewController(this, broker, guardedClient, url).also { attached ->
                    LocalAppWebViewRegistry.register(appId, attached)
                    currentControllerHandler(attached)
                }
                tag = url
                loadUrl(url)
            }
        },
        update = { view ->
            if (view.tag != url) {
                view.tag = url
                view.loadUrl(url)
            }
        },
    )

    DisposableEffect(appId) {
        onDispose {
            controller?.let { attached ->
                LocalAppWebViewRegistry.unregister(appId, attached)
                attached.detach()
            }
            webView?.apply {
                stopLoading()
                WebViewCompat.removeWebMessageListener(this, LINGXI_V1_MESSAGE_OBJECT)
                destroy()
            }
            controller = null
            webView = null
        }
    }

    pendingExternalUrl?.let { target ->
        AlertDialog(
            onDismissRequest = { pendingExternalUrl = null },
            title = { Text(stringResource(R.string.local_apps_leave_app_title)) },
            text = { Text(stringResource(R.string.local_apps_leave_app_detail, target)) },
            dismissButton = {
                TextButton(onClick = { pendingExternalUrl = null }) { Text(stringResource(R.string.common_cancel)) }
            },
            confirmButton = {
                TextButton(onClick = {
                    pendingExternalUrl = null
                    onExternalNavigation(target)
                }) { Text(stringResource(R.string.onboarding_cta_continue)) }
            },
        )
    }
}

private enum class LocalAppWebStorageGate {
    Waiting,
    Ready,
    Failed,
}

private fun Uri?.sameTrustedOrigin(other: Uri?): Boolean =
    this != null && other != null &&
        isTrustedLoopback() && other.isTrustedLoopback() &&
        scheme == other.scheme && normalizedHost() == other.normalizedHost() && effectivePort() == other.effectivePort()

private fun Uri.isTrustedLoopback(): Boolean =
    scheme == "http" && normalizedHost() in setOf("127.0.0.1", "localhost", "::1")

private fun Uri.normalizedHost(): String? = host?.lowercase()?.removePrefix("[")?.removeSuffix("]")

private fun Uri.effectivePort(): Int = if (port >= 0) port else if (scheme == "https") 443 else 80

internal fun Uri.toTrustedOriginString(): String {
    require(isTrustedLoopback()) { "Only loopback HTTP origins are supported" }
    val normalizedHost = normalizedHost()
    val renderedHost = if (normalizedHost == "::1") "[::1]" else normalizedHost
    return "$scheme://$renderedHost:${effectivePort()}"
}

private const val LINGXI_V1_MESSAGE_OBJECT = "LingXiNativeV1"

internal fun androidFormFactor(configuration: Configuration): String =
    androidFormFactor(configuration.smallestScreenWidthDp)

internal fun androidFormFactor(smallestScreenWidthDp: Int): String =
    if (smallestScreenWidthDp >= 600) "tablet" else "phone"

internal fun buildLingxiV1Bootstrap(formFactor: String): String {
    require(formFactor == "phone" || formFactor == "tablet") { "Unsupported Android form factor" }
    return LINGXI_V1_BOOTSTRAP_TEMPLATE.replace("__LINGXI_NATIVE_FORM_FACTOR__", formFactor)
}

private const val LINGXI_V1_BOOTSTRAP_TEMPLATE = """
(() => {
  if (window.lingxi?.v2) return;
  const installCsp = () => {
    if (!document.head || document.head.querySelector('meta[data-lingxi-csp]')) return false;
    const meta = document.createElement('meta');
    meta.httpEquiv = 'Content-Security-Policy';
    meta.dataset.lingxiCsp = 'v2';
    meta.content = "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";
    document.head.prepend(meta);
    return true;
  };
  if (!installCsp()) {
    const observer = new MutationObserver(() => {
      if (installCsp()) observer.disconnect();
    });
    observer.observe(document.documentElement || document, { childList: true, subtree: true });
  }
  const localOnly = input => {
    const raw = typeof input === 'string' || input instanceof URL ? input : input?.url;
    const target = new URL(raw, location.href);
    if (target.origin !== location.origin) {
      throw new TypeError('External network access must use window.lingxi.v2.network');
    }
    return target;
  };
  const nativeFetch = window.fetch.bind(window);
  window.fetch = (input, init) => { localOnly(input); return nativeFetch(input, init); };
  const nativeOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function(method, target, ...rest) {
    localOnly(target);
    return nativeOpen.call(this, method, target, ...rest);
  };
  const NativeWebSocket = window.WebSocket;
  window.WebSocket = function(target, protocols) {
    localOnly(target);
    return protocols === undefined ? new NativeWebSocket(target) : new NativeWebSocket(target, protocols);
  };
  window.WebSocket.prototype = NativeWebSocket.prototype;
  const NativeEventSource = window.EventSource;
  window.EventSource = function(target, options) {
    localOnly(target);
    return new NativeEventSource(target, options);
  };
  window.EventSource.prototype = NativeEventSource.prototype;
  const nativeSendBeacon = navigator.sendBeacon?.bind(navigator);
  if (nativeSendBeacon) {
    navigator.sendBeacon = (target, data) => { localOnly(target); return nativeSendBeacon(target, data); };
  }
  const resourceUrlAllowed = (element, value) => {
    const target = new URL(String(value), location.href);
    if (target.origin === location.origin) return;
    const media = ['IMG', 'AUDIO', 'VIDEO', 'SOURCE'].includes(element.tagName);
    if (media && ['data:', 'blob:'].includes(target.protocol)) return;
    throw new TypeError('External resources are blocked');
  };
  const guardedAttributes = new Set(['src', 'href', 'action', 'poster']);
  const nativeSetAttribute = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function(name, value) {
    const attribute = String(name).toLowerCase();
    // A[href] is navigation, not a subresource. Let WebViewClient inspect it
    // and show the external-navigation confirmation; LINK[href] and every
    // other resource-bearing attribute remain local-only.
    const normalAnchor = attribute === 'href' && this.tagName === 'A';
    if (guardedAttributes.has(attribute) && !normalAnchor) resourceUrlAllowed(this, value);
    return nativeSetAttribute.call(this, name, value);
  };
  const pending = new Map();
  const streamListeners = new Map();
  const emitStream = frame => {
    const channel = pending.get(frame.requestId)?.channel;
    for (const [listener, listenerChannel] of streamListeners) {
      if (listenerChannel !== channel) continue;
      try { listener(frame); } catch (_) { /* app listener isolation */ }
    }
  };
  const request = (operation, payload = {}, channel = null) => new Promise((resolve, reject) => {
    const requestId = (crypto.randomUUID ? crypto.randomUUID() : Date.now().toString(36) + Math.random().toString(36).slice(2));
    const handler = window.LingXiNativeV1;
    if (!handler?.postMessage) {
      pending.delete(requestId);
      const error = new Error('Lingxi bridge unavailable');
      error.code = 'bridge_unavailable';
      reject(error);
      return;
    }
    let serialized;
    try {
      serialized = JSON.stringify({requestId, operation, payload});
    } catch (_) {
      const error = new Error('Bridge payload is not serializable');
      error.code = 'payload_invalid';
      reject(error);
      return;
    }
    const byteLimit = operation === 'llm_chat' || operation === 'llm_stream' ? 8388608 :
      (operation === 'file_read' || operation === 'file_write' ? 4194304 : 65536);
    if (new TextEncoder().encode(serialized).byteLength > byteLimit) {
      const error = new Error('Bridge request exceeds ' + byteLimit + ' bytes');
      error.code = 'request_too_large';
      reject(error);
      return;
    }
    pending.set(requestId, {resolve, reject, channel});
    handler.postMessage(serialized);
  });
  const readInsets = () => {
    const probe = document.createElement('div');
    probe.style.cssText = 'position:fixed;inset:0;padding:env(safe-area-inset-top) env(safe-area-inset-right) env(safe-area-inset-bottom) env(safe-area-inset-left);pointer-events:none;';
    (document.documentElement || document.body).appendChild(probe);
    const style = getComputedStyle(probe);
    const number = value => Number.parseFloat(value) || 0;
    const result = {top: number(style.paddingTop), right: number(style.paddingRight), bottom: number(style.paddingBottom), left: number(style.paddingLeft)};
    probe.remove();
    return result;
  };
  const readViewport = () => ({
    width: Math.round(window.visualViewport?.width || window.innerWidth || 0),
    height: Math.round(window.visualViewport?.height || window.innerHeight || 0)
  });
  const deviceContext = Object.freeze({
    os: 'android',
    formFactor: '__LINGXI_NATIVE_FORM_FACTOR__',
    get viewport() { return readViewport(); },
    get safeArea() { return readInsets(); },
    get colorScheme() { return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'; },
    get reducedMotion() { return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches === true; },
    get inputMode() { return window.matchMedia?.('(pointer: fine)').matches ? 'pointer' : 'touch'; },
  });
  const v2 = Object.freeze({
    deviceContext,
    data: Object.freeze({
      query: (payload) => request('query_data', payload),
      mutate: (payload) => request('mutate_data', payload)
    }),
    network: Object.freeze({
      fetch: (payload) => request('network_request', payload),
      request: (payload) => request('network_request', payload)
    }),
    runtime: Object.freeze({
      info: () => request('runtime_status', {}),
      status: () => request('runtime_status', {}),
      deviceContext,
    }),
    device: Object.freeze({
      capturePhoto: (payload = {}) => request('capture_photo', payload),
      pickImage: (payload = {}) => request('pick_image', payload),
      recordAudioStart: (payload = {}) => request('record_audio_start', payload),
      recordAudioStop: () => request('record_audio_stop', {}),
      getLocation: () => request('get_location', {}),
      transcribeSpeech: (payload = {}) => request('transcribe_speech', payload),
      postNotification: payload => request('post_notification', payload),
      share: (payload = {}) => request('share', payload),
      synthesizeSpeech: (payload = {}) => request('synthesize_speech', payload),
      status: () => request('device_status', {}),
      haptics: style => request('haptics', { style }),
      deepLink: url => request('deep_link', { url }),
    }),
    clipboard: Object.freeze({
      getText: () => request('clipboard_get_text', {}),
      setText: text => request('clipboard_set_text', { text }),
    }),
        files: Object.freeze({
          read: payload => request('file_read', payload),
          write: payload => request('file_write', payload),
        }),
        calendar: Object.freeze({
          listEvents: payload => request('calendar_list_events', payload),
        }),
        contacts: Object.freeze({
          search: payload => request('contacts_search', payload),
        }),
        media: Object.freeze({
          get: payload => request('media_get', payload),
        }),
    llm: Object.freeze({
      chat: payload => request('llm_chat', payload),
      stream: payload => request('llm_stream', payload, 'llm'),
      onFrame: listener => {
        if (typeof listener !== 'function') throw new TypeError('LLM stream listener must be a function');
        streamListeners.set(listener, 'llm');
        return () => streamListeners.delete(listener);
      }
    }),
    agent: Object.freeze({
      post: payload => request('agent_post', payload),
      sessions: Object.freeze({
        create: (payload = {}) => request('agent_session_create', payload),
        list: () => request('agent_session_list', {}),
        resume: payload => request('agent_session_resume', payload),
        close: payload => request('agent_session_close', payload),
      }),
      send: payload => request('agent_send', payload),
      stream: payload => request('agent_stream', payload, 'agent'),
      cancel: payload => request('agent_cancel', payload),
      onFrame: listener => {
        if (typeof listener !== 'function') throw new TypeError('Agent stream listener must be a function');
        streamListeners.set(listener, 'agent');
        return () => streamListeners.delete(listener);
      },
      profiles: Object.freeze({
        proposeUpdate: payload => request('agent_profile_propose_update', payload),
      }),
    }),
    background: Object.freeze({
      schedule: payload => request('background_schedule', payload),
      list: (payload = {}) => request('background_list', payload),
      status: payload => request('background_status', payload),
      cancel: payload => request('background_cancel', payload),
      retry: payload => request('background_retry', payload),
    })
  });
  const resolveNative = envelope => {
    const handler = pending.get(envelope.requestId);
    if (!handler) return;
    pending.delete(envelope.requestId);
    if (envelope.error) {
      const error = new Error(envelope.error);
      if (envelope.code) error.code = envelope.code;
      handler.reject(error);
    } else {
      handler.resolve(envelope.result);
    }
  };
  Object.defineProperty(window, 'lingxi', {
    value: Object.freeze({v2, __resolve: resolveNative, __stream: emitStream}),
    configurable: false,
    writable: false
  });
})();
"""
