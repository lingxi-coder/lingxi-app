package com.lingxi.code.localapps

import android.annotation.SuppressLint
import android.content.res.Configuration
import android.graphics.Bitmap
import android.net.Uri
import android.os.Build
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.RenderProcessGoneDetail
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
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
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
import java.net.URI

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
    ///
    /// [value] is the same opaque `AppUiRequestDto.value` string every other
    /// action already carries, holding an optional crop request: shape
    /// `{"rect":{"x":..,"y":..,"width":..,"height":..}}`. Null (or anything
    /// that fails to parse) means the whole view, exactly as before this
    /// field existed.
    data class CaptureView(val value: String? = null) : LocalAppUiAutomationAction

    /// Pointer event at viewport coordinates. `Click` resolves an element and
    /// fires at (0,0); a canvas has no element and needs real coordinates.
    data class Pointer(val x: Int, val y: Int, val phase: String) : LocalAppUiAutomationAction

    /// Keyboard event. There was no key action at all before this.
    data class Key(val key: String, val phase: String) : LocalAppUiAutomationAction

    /** Host-owned QA identity; the nested action retains the original value bytes. */
    data class Qa(
        val expectedRuntimeUrl: String,
        val action: LocalAppUiAutomationAction,
    ) : LocalAppUiAutomationAction
}

internal data class LocalAppQaEnvelope(
    val expectedRuntimeUrl: String,
    val actionValue: String?,
)

internal data class LocalAppQaDocument(
    val loadedRuntimeUrl: String,
    val navigationGeneration: Long,
    /**
     * How many full documents this WebView has started loading. Every
     * cross-document load raises it in `onPageStarted`; a same-document
     * history commit never does. `navigationGeneration` alone cannot separate
     * the two, so this is what lets an event action attest an SPA route it
     * committed without also attesting a whole new document it merely raced.
     * Never reset, for the same reason `navigationGeneration` is not: a stale
     * snapshot must never compare equal to a later one.
     */
    val documentLoadGeneration: Long = 0,
)

/** Pure lifecycle state so same-port A → B races can be regression-tested on the JVM. */
internal class LocalAppQaDocumentState(initialRuntimeUrl: String) {
    private var expectedRuntimeUrl = initialRuntimeUrl.asUriOrNull()
    private var pendingStartUrl: URI? = null
    private var pendingHistoryNavigation = false
    private var activeStartGeneration: Long? = null
    private var activeStartUrl: URI? = null
    private var committedUrl: URI? = null
    private var deferredVisitedHistoryObserved = false
    private var deferredVisitedHistoryUrl: URI? = null
    /**
     * The last document that reached a committed state. `committedUrl` is
     * cleared as soon as Host navigation starts; retaining this identity lets
     * Back reject a delayed callback for the current entry and authenticate
     * only the requested copied-list destination.
     */
    private var lastCommittedUrl: URI? = null
    private var documentLoadGeneration: Long = 0
    private var ready = false
    private var visualReadyGeneration: Long? = null
    private var activeVisualFenceToken: Long? = null
    private var nextVisualFenceToken = 0L
    var isDetached = false
        private set

    var navigationGeneration: Long = 0
        private set

    fun beginNavigation(url: String, isHistoryNavigation: Boolean = false): Long {
        navigationGeneration += 1
        val requested = url.asUriOrNull()
        if (requested?.let(::isLocalAppQaRuntimeUrl) == true || expectedRuntimeUrl == null) {
            expectedRuntimeUrl = requested
        }
        pendingStartUrl = requested
        pendingHistoryNavigation = isHistoryNavigation
        activeStartGeneration = null
        activeStartUrl = null
        committedUrl = null
        deferredVisitedHistoryObserved = false
        deferredVisitedHistoryUrl = null
        ready = false
        visualReadyGeneration = null
        activeVisualFenceToken = null
        return navigationGeneration
    }

    fun onPageStarted(url: String) {
        if (isDetached) return
        val started = url.asUriOrNull()
        if (!pendingHistoryNavigation && started != pendingStartUrl) navigationGeneration += 1
        documentLoadGeneration += 1
        pendingStartUrl = null
        pendingHistoryNavigation = false
        activeStartGeneration = navigationGeneration
        activeStartUrl = started
        committedUrl = null
        deferredVisitedHistoryObserved = false
        deferredVisitedHistoryUrl = null
        ready = false
        visualReadyGeneration = null
        activeVisualFenceToken = null
    }

    fun onPageFinished(url: String): Long? {
        if (isDetached || activeStartGeneration != navigationGeneration) return null
        val finished = url.asUriOrNull() ?: return null
        val started = activeStartUrl
        val deferred = deferredVisitedHistoryUrl
        if (deferredVisitedHistoryObserved && deferred == null) {
            // A foreign or malformed same-document URL was committed while
            // this document was loading. The later load event must not revive
            // the pre-history URL as a trusted QA document.
            invalidateDocument()
            return null
        }
        // Ignore an old runtime's late finish without consuming the current
        // generation; the current document may still finish afterward.
        if (!sameLocalAppRuntimeIdentity(expectedRuntimeUrl, finished) ||
            (finished != started && finished != deferred)
        ) return null
        val committed = deferred ?: finished
        if (deferredVisitedHistoryObserved && committed != started) navigationGeneration += 1
        committedUrl = committed
        lastCommittedUrl = committed
        activeStartGeneration = null
        activeStartUrl = null
        deferredVisitedHistoryObserved = false
        deferredVisitedHistoryUrl = null
        ready = true
        visualReadyGeneration = null
        activeVisualFenceToken = null
        return navigationGeneration
    }

    /**
     * Fragment history does not reliably produce onPageStarted callbacks.
     * This is the explicit Host fallback for a navigation already classified
     * as same-document (Navigate/Back); the native history callback is the
     * normal path below.
     */
    fun onSameDocumentNavigationObserved(url: String): Long? =
        commitSameDocumentNavigation(url, requirePriorDocument = false)

    /**
     * Mirrors WebViewClient.doUpdateVisitedHistory. Chromium invokes this for
     * pushState/replaceState/hash history commits without onPageStarted. An
     * update observed during a full page-start sequence is retained until its
     * finish so startup routing cannot be dropped; a full navigation's own
     * same-URL history callback then collapses to a no-op.
     */
    fun onVisitedHistoryUpdated(url: String, isReload: Boolean = false): Long? {
        if (isDetached || isReload) return null
        val observed = url.asUriOrNull()
        if (activeStartGeneration != null) {
            // Chromium reports same-document commits as soon as that nested
            // navigation commits, which can be before the enclosing document's
            // onPageFinished (startup routers commonly replaceState here).
            // Retain the latest observed URL and fold it into that finish.
            // A null value deliberately records an untrusted observation.
            deferredVisitedHistoryObserved = true
            deferredVisitedHistoryUrl = observed?.takeIf {
                sameLocalAppRuntimeIdentity(expectedRuntimeUrl, it)
            }
            visualReadyGeneration = null
            activeVisualFenceToken = null
            return null
        }
        if (observed == null || !sameLocalAppRuntimeIdentity(expectedRuntimeUrl, observed)) {
            // A same-document callback carrying a foreign/malformed runtime
            // must invalidate the old ready document. Otherwise a QA Inspect
            // could attest stale state after the page changed its URL.
            invalidateDocument()
            return null
        }
        return commitSameDocumentNavigation(url, requirePriorDocument = true)
    }

    private fun commitSameDocumentNavigation(
        url: String,
        requirePriorDocument: Boolean,
    ): Long? {
        if (isDetached || activeStartGeneration != null) return null
        val committed = url.asUriOrNull() ?: return null
        if (!sameLocalAppRuntimeIdentity(expectedRuntimeUrl, committed)) return null
        val pending = pendingStartUrl
        val historyNavigation = pendingHistoryNavigation
        val prior = committedUrl ?: lastCommittedUrl
        if (historyNavigation) {
            if (pending == null || prior == null || committed != pending || committed == prior) return null
        } else if (pending != null) {
            if (pending != committed ||
                (requirePriorDocument &&
                    !isSameLocalAppHistoryDocument(expectedRuntimeUrl, prior, committed))
            ) return null
        } else {
            // A page-created history entry must move from the currently loaded
            // URL. Duplicate doUpdateVisitedHistory callbacks are no-ops.
            if (!ready || committedUrl == null || committedUrl == committed) return null
        }
        pendingStartUrl = null
        pendingHistoryNavigation = false
        activeStartGeneration = null
        activeStartUrl = null
        if (pending == null && !historyNavigation) navigationGeneration += 1
        committedUrl = committed
        lastCommittedUrl = committed
        ready = true
        visualReadyGeneration = null
        activeVisualFenceToken = null
        return navigationGeneration
    }

    fun isCurrentVisualDocument(url: String, generation: Long): Boolean {
        val committed = committedUrl
        return !isDetached && generation == navigationGeneration &&
            committed?.toString() == url &&
            sameLocalAppRuntimeIdentity(expectedRuntimeUrl, committed)
    }

    fun beginVisualFrameFence(url: String, generation: Long): Long? {
        if (!isCurrentVisualDocument(url, generation)) return null
        nextVisualFenceToken += 1
        activeVisualFenceToken = nextVisualFenceToken
        visualReadyGeneration = null
        return nextVisualFenceToken
    }

    fun onVisualFrame(url: String, generation: Long, token: Long? = null): Boolean {
        if (!isCurrentVisualDocument(url, generation) ||
            (token != null && activeVisualFenceToken != token)
        ) return false
        visualReadyGeneration = generation
        return true
    }

    fun document(
        expected: URI,
        minimumGeneration: Long = 0,
        requireVisualFrame: Boolean = false,
    ): LocalAppQaDocument? {
        val committed = committedUrl
        if (isDetached || !ready || navigationGeneration < minimumGeneration ||
            !sameLocalAppRuntimeIdentity(expected, committed) ||
            (requireVisualFrame && visualReadyGeneration != navigationGeneration)
        ) return null
        return LocalAppQaDocument(committed.toString(), navigationGeneration, documentLoadGeneration)
    }

    /** Validate the observed route against the canonical Host runtime identity. */
    fun currentDocument(
        minimumGeneration: Long = 0,
        requireVisualFrame: Boolean = false,
    ): LocalAppQaDocument? = expectedRuntimeUrl?.let {
        document(it, minimumGeneration, requireVisualFrame)
    }

    fun detach() {
        isDetached = true
        pendingStartUrl = null
        pendingHistoryNavigation = false
        activeStartGeneration = null
        activeStartUrl = null
        committedUrl = null
        lastCommittedUrl = null
        deferredVisitedHistoryObserved = false
        deferredVisitedHistoryUrl = null
        ready = false
        visualReadyGeneration = null
        activeVisualFenceToken = null
    }

    private fun invalidateDocument() {
        pendingStartUrl = null
        pendingHistoryNavigation = false
        activeStartGeneration = null
        activeStartUrl = null
        committedUrl = null
        lastCommittedUrl = null
        deferredVisitedHistoryObserved = false
        deferredVisitedHistoryUrl = null
        ready = false
        visualReadyGeneration = null
        activeVisualFenceToken = null
    }
}

internal fun isLocalAppQaRequestId(requestId: String): Boolean = requestId.startsWith("qa-ui-")

/** Decode only the Host-reserved wrapper; ordinary action values remain opaque. */
internal fun parseLocalAppQaEnvelope(value: String?, requestId: String): LocalAppQaEnvelope? {
    // The request id is minted by the Host and cannot be supplied by page
    // content. Value shape alone would hijack literal JSON text entry.
    if (!isLocalAppQaRequestId(requestId)) return null
    val root = runCatching { JSONObject(value.orEmpty()) }.getOrNull() ?: return null
    val qa = root.optJSONObject("lingxi_qa") ?: return null
    val version = qa.opt("version")
    if (version !is Number || version.toDouble() != 1.0) return null
    val expected = qa.optString("expected_runtime_url", "")
    val parsed = expected.asUriOrNull() ?: return null
    if (!isLocalAppQaRuntimeUrl(parsed) || root.keys().asSequence().any { it != "lingxi_qa" } ||
        qa.keys().asSequence().any { it !in setOf("version", "expected_runtime_url", "action_value") }) {
        return null
    }
    val actionValue = when {
        !qa.has("action_value") || qa.isNull("action_value") -> null
        qa.opt("action_value") is String -> qa.getString("action_value")
        else -> return null
    }
    return LocalAppQaEnvelope(expected, actionValue)
}

private fun String.asUriOrNull(): URI? = runCatching { URI(this) }.getOrNull()

private fun URI.normalizedLoopbackHost(): String? =
    host?.lowercase()?.removePrefix("[")?.removeSuffix("]")
        ?.takeIf { it in setOf("127.0.0.1", "localhost", "::1") }

private fun URI.runtimeMarkers(): List<String> =
    rawQuery.orEmpty().split('&').mapNotNull { item ->
        item.split('=', limit = 2).takeIf { it.size == 2 && it[0] == "lingxi_runtime" }?.get(1)
    }

internal fun isLocalAppQaRuntimeUrl(url: URI): Boolean {
    val markers = url.runtimeMarkers()
    return url.scheme.equals("http", ignoreCase = true) &&
        url.normalizedLoopbackHost() != null &&
        url.userInfo == null && url.fragment == null && url.rawPath == "/" &&
        url.port in 1..65_535 && markers.size == 1 &&
        markers[0].isNotEmpty() && markers[0].all(Char::isDigit) &&
        url.rawQuery.orEmpty().split('&').size == 1
}

internal fun sameLocalAppRuntimeIdentity(expected: URI?, committed: URI?): Boolean {
    if (expected == null || committed == null || !isLocalAppQaRuntimeUrl(expected)) return false
    val expectedMarker = expected.runtimeMarkers().single()
    val committedMarkers = committed.runtimeMarkers()
    return committed.scheme.equals(expected.scheme, ignoreCase = true) &&
        committed.normalizedLoopbackHost() == expected.normalizedLoopbackHost() &&
        committed.port == expected.port && committed.userInfo == null &&
        committedMarkers.size == 1 && committedMarkers[0] == expectedMarker
}

/** A Host-requested fragment history step must stay in the same document. */
private fun isSameLocalAppHistoryDocument(
    expected: URI?,
    previous: URI?,
    next: URI?,
): Boolean =
    previous != null && next != null &&
        sameLocalAppRuntimeIdentity(expected, previous) &&
        sameLocalAppRuntimeIdentity(expected, next) &&
        previous.rawPath == next.rawPath &&
        previous.rawQuery == next.rawQuery &&
        previous.fragment != next.fragment

internal fun sameLocalAppQaDocument(
    before: LocalAppQaDocument,
    after: LocalAppQaDocument,
    intentionalNavigation: Boolean,
    allowInteractiveNavigation: Boolean = false,
): Boolean = if (intentionalNavigation) {
    after.navigationGeneration > before.navigationGeneration
} else if (allowInteractiveNavigation) {
    // Event actions may synchronously commit a same-document SPA route, so a
    // successful action may advance the generation -- but ONLY through
    // same-document history commits. A full document load starting during the
    // action raises documentLoadGeneration, and the action's result was
    // produced in the document that load replaced: attesting it against the
    // new one would bind the evidence to a route it never ran in, and which
    // route won would depend on when awaitQaDocument happened to poll. A no-op
    // event still has to certify the exact pre-action document.
    after.documentLoadGeneration == before.documentLoadGeneration &&
        (after.navigationGeneration > before.navigationGeneration ||
            (after.navigationGeneration == before.navigationGeneration &&
                after.loadedRuntimeUrl == before.loadedRuntimeUrl))
} else {
    after.navigationGeneration == before.navigationGeneration &&
        after.loadedRuntimeUrl == before.loadedRuntimeUrl
}

internal fun localAppQaNavigationWasAccepted(
    action: LocalAppUiAutomationAction,
    result: LocalAppUiExecutionResult,
): Boolean = result.error == null && (
    action is LocalAppUiAutomationAction.Navigate ||
        action is LocalAppUiAutomationAction.Back ||
        action is LocalAppUiAutomationAction.Reload
    )

/** Event actions can legitimately submit a form or commit an SPA history entry. */
internal fun localAppQaActionMayAdvanceDocument(
    action: LocalAppUiAutomationAction,
    result: LocalAppUiExecutionResult,
): Boolean = result.error == null && when (action) {
    is LocalAppUiAutomationAction.Click,
    is LocalAppUiAutomationAction.Fill,
    is LocalAppUiAutomationAction.Select,
    is LocalAppUiAutomationAction.Toggle,
    is LocalAppUiAutomationAction.Pointer,
    is LocalAppUiAutomationAction.Key -> true
    else -> false
}

internal fun localAppFrameCommitFenceAvailable(
    sdkInt: Int,
    attached: Boolean,
    hardwareAccelerated: Boolean,
): Boolean = sdkInt >= Build.VERSION_CODES.Q && attached && hardwareAccelerated

internal fun localAppCaptureFrameFenceUnavailable(
    qaCapture: Boolean,
    sdkInt: Int,
    attached: Boolean,
    hardwareAccelerated: Boolean,
): Boolean = qaCapture && !localAppFrameCommitFenceAvailable(sdkInt, attached, hardwareAccelerated)

internal fun buildLocalAppQaExecutionResult(
    originalResultJson: String?,
    operationError: String? = null,
    requested: URI,
    document: LocalAppQaDocument,
    platform: String,
    formFactor: String,
    width: Int,
    height: Int,
    devicePixelRatio: Float,
): String {
    val metadata = jsonObjectString(
        "version" to 1,
        "requested_runtime_url" to requested.toString(),
        "loaded_runtime_url" to document.loadedRuntimeUrl,
        "platform" to platform,
        "form_factor" to formFactor,
        "navigation_generation" to document.navigationGeneration,
        "width" to width,
        "height" to height,
        "device_pixel_ratio" to devicePixelRatio,
    )
    val result = operationError?.let { error ->
        val objectResult = runCatching { JSONObject(originalResultJson ?: "{}") }
            .getOrElse { JSONObject() }
            .put("ok", false)
            .put("error", error)
        RawJson(objectResult.toString())
    } ?: originalResultJson?.let(::RawJson)
        ?: RawJson(JSONObject.NULL.toString())
    return jsonObjectString(
        "lingxi_qa" to RawJson(metadata),
        "result" to result,
    )
}

data class LocalAppUiExecutionResult(
    val resultJson: String?,
    val error: String?,
)

private data class RawJson(val json: String)

/**
 * Shared by every path in `captureFrame` that cannot produce a frame: an
 * offscreen view, and a requested crop that clamps to nothing, or that
 * cannot be honoured on a path with no source-rect concept.
 * One literal, not a copy at each call site, so the three cannot drift apart.
 */
private const val LOCAL_APP_CAPTURE_UNAVAILABLE_ERROR = "The app view could not be captured; it may be offscreen."
private const val LOCAL_APP_CAPTURE_FRAME_FENCE_UNAVAILABLE_ERROR =
    "The app view could not be captured; a hardware frame-commit fence is unavailable."
private const val LOCAL_APP_VISUAL_FENCE_TIMEOUT_MS = 1_500L

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
    private val platform = "android"
    private val formFactor = androidFormFactor(webView.resources.configuration)
    private var suspendedUrl: String? = null
    private var deletionSuspended = false
    private val qaDocumentState = LocalAppQaDocumentState(initialUrl)

    internal fun beginNavigation(url: String, isHistoryNavigation: Boolean = false) {
        qaDocumentState.beginNavigation(url, isHistoryNavigation)
    }

    internal fun onPageStarted(url: String) {
        qaDocumentState.onPageStarted(url)
    }

    internal fun onPageFinished(url: String) {
        val generation = qaDocumentState.onPageFinished(url) ?: return
        val committed = qaDocumentState.currentDocument(minimumGeneration = generation) ?: return
        armVisualReadiness(committed.loadedRuntimeUrl, committed.navigationGeneration)
    }

    internal fun onVisitedHistoryUpdated(url: String, isReload: Boolean) {
        // Chromium queues onPageStarted before this callback for a committed
        // cross-document load, while same-document commits deliberately have
        // no page-start callback. Observe this callback on its native UI turn;
        // another post would reorder startup replaceState after onPageFinished.
        val generation = qaDocumentState.onVisitedHistoryUpdated(url, isReload) ?: return
        armVisualReadiness(url, generation)
    }

    internal fun observeSameDocumentNavigation(expectedUrl: String, attempts: Int = 0) {
        val url = webView.url ?: return
        if (url != expectedUrl) {
            if (attempts < 8) {
                webView.postOnAnimation {
                    observeSameDocumentNavigation(expectedUrl, attempts + 1)
                }
            }
            return
        }
        val generation = qaDocumentState.onSameDocumentNavigationObserved(url) ?: return
        armVisualReadiness(url, generation)
    }

    private fun armVisualReadiness(url: String, generation: Long) {
        requestVisualFrameFence(url, generation) {}
    }

    /**
     * Requests a new DOM-state and compositor-frame proof. A navigation's
     * readiness proof is deliberately not reusable for a later QA capture:
     * the page may have painted new canvas/DOM content without changing URL.
     */
    private fun requestVisualFrameFence(
        url: String,
        generation: Long,
        onReady: (Boolean) -> Unit,
    ) {
        if (localAppCaptureFrameFenceUnavailable(
                qaCapture = true,
                sdkInt = Build.VERSION.SDK_INT,
                attached = webView.isAttachedToWindow,
                hardwareAccelerated = webView.isHardwareAccelerated,
            )
        ) {
            onReady(false)
            return
        }
        val token = qaDocumentState.beginVisualFrameFence(url, generation)
        if (token == null) {
            onReady(false)
            return
        }
        var completed = false
        fun complete(success: Boolean) {
            if (completed) return
            completed = true
            onReady(success)
        }
        webView.postDelayed({ complete(false) }, LOCAL_APP_VISUAL_FENCE_TIMEOUT_MS)
        webView.postVisualStateCallback(
            token,
            object : WebView.VisualStateCallback() {
                override fun onComplete(requestId: Long) {
                    if (completed) return
                    if (requestId != token ||
                        !qaDocumentState.isCurrentVisualDocument(url, generation)
                    ) {
                        complete(false)
                        return
                    }
                    val observer = webView.viewTreeObserver
                    if (!observer.isAlive) {
                        complete(false)
                        return
                    }
                    // registerFrameCommitCallback was added in API 29; keep
                    // the platform proof explicit for lint and future callers.
                    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
                        complete(false)
                        return
                    }
                    observer.registerFrameCommitCallback {
                        if (completed) return@registerFrameCommitCallback
                        // The visual-state callback proves WebView's DOM state;
                        // the frame-commit callback proves the host compositor
                        // submitted that frame before PixelCopy samples it.
                        complete(qaDocumentState.onVisualFrame(url, generation, token))
                    }
                    webView.postInvalidateOnAnimation()
                }
            },
        )
    }

    private fun awaitQaDocument(
        expected: URI,
        minimumGeneration: Long,
        requireVisualFrame: Boolean = false,
        attempts: Int = 0,
        onReady: (LocalAppQaDocument?) -> Unit,
    ) {
        qaDocumentState.document(expected, minimumGeneration, requireVisualFrame)
            ?.takeIf { document -> webView.url == document.loadedRuntimeUrl }
            ?.let { document ->
                onReady(document)
                return
            }
        if (qaDocumentState.isDetached || attempts >= 100) {
            onReady(null)
            return
        }
        webView.postDelayed({
            awaitQaDocument(expected, minimumGeneration, requireVisualFrame, attempts + 1, onReady)
        }, 50L)
    }

    fun execute(
        action: LocalAppUiAutomationAction,
        onResult: (LocalAppUiExecutionResult) -> Unit = {},
    ) {
        val qa = action as? LocalAppUiAutomationAction.Qa
        if (qa != null) {
            val expected = qa.expectedRuntimeUrl.asUriOrNull()
            if (expected == null || !isLocalAppQaRuntimeUrl(expected)) {
                onResult(LocalAppUiExecutionResult(null, "Invalid Local App QA runtime URL"))
                return
            }
            val startedGeneration = qaDocumentState.navigationGeneration
            val requiresVisualFrame = qa.action is LocalAppUiAutomationAction.CaptureView
            if (localAppCaptureFrameFenceUnavailable(
                    qaCapture = requiresVisualFrame,
                    sdkInt = Build.VERSION.SDK_INT,
                    attached = webView.isAttachedToWindow,
                    hardwareAccelerated = webView.isHardwareAccelerated,
                )
            ) {
                onResult(LocalAppUiExecutionResult(null, LOCAL_APP_CAPTURE_FRAME_FENCE_UNAVAILABLE_ERROR))
                return
            }
            // A capture requests its own fresh frame fence below. Navigation's
            // earlier visual proof is only a lifecycle hint, never capture
            // evidence for the current DOM/canvas state.
            awaitQaDocument(expected, startedGeneration) { before ->
                if (before == null) {
                    val error = if (requiresVisualFrame) {
                        "Local App QA capture unavailable: hardware frame-commit fence is unsupported"
                    } else {
                        "Local App QA document identity did not become ready"
                    }
                    onResult(LocalAppUiExecutionResult(null, error))
                    return@awaitQaDocument
                }
                executeOrdinary(
                    action = qa.action,
                    onResult = { result ->
                        val intentionalNavigation = localAppQaNavigationWasAccepted(qa.action, result)
                        val interactiveNavigation = localAppQaActionMayAdvanceDocument(qa.action, result)
                        val minimumGeneration = if (intentionalNavigation) {
                            before.navigationGeneration + 1
                        } else {
                            before.navigationGeneration
                        }
                        // Give synchronous page navigation (for example a click
                        // handler) one UI turn to enter onPageStarted before the
                        // post-action identity validation.
                        webView.postDelayed({
                            awaitQaDocument(expected, minimumGeneration, requiresVisualFrame) { after ->
                                if (after == null) {
                                    val error = if (requiresVisualFrame) {
                                        "Local App QA capture unavailable: visual frame was not committed"
                                    } else {
                                        "Local App QA document identity changed during the action"
                                    }
                                    onResult(LocalAppUiExecutionResult(null, error))
                                } else if (!sameLocalAppQaDocument(
                                        before,
                                        after,
                                        intentionalNavigation,
                                        allowInteractiveNavigation = interactiveNavigation,
                                    )
                                ) {
                                    onResult(LocalAppUiExecutionResult(null, "Local App QA document changed during the action"))
                                } else {
                                    onResult(attestQaResult(result, expected, after))
                                }
                            }
                        }, 50L)
                    },
                    qaCapture = requiresVisualFrame,
                )
            }
            return
        }
        executeOrdinary(action, onResult)
    }

    private fun executeOrdinary(
        action: LocalAppUiAutomationAction,
        onResult: (LocalAppUiExecutionResult) -> Unit,
        qaCapture: Boolean = false,
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
                val target = Uri.parse(resolveLocalAppNavigation(webView.url.orEmpty(), action.path))
                if (target.sameTrustedOrigin(current)) {
                    beginNavigation(target.toString())
                    webView.loadUrl(target.toString())
                    if (isSameDocumentNavigation(current, target)) {
                        webView.post { observeSameDocumentNavigation(target.toString()) }
                    }
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
            LocalAppUiAutomationAction.Back -> {
                val current = Uri.parse(webView.url ?: initialUrl)
                val history = webView.copyBackForwardList()
                val target = history.currentIndex
                    .takeIf { it > 0 }
                    ?.let { history.getItemAtIndex(it - 1)?.url }
                    ?.let(Uri::parse)
                if (target == null) {
                    onResult(LocalAppUiExecutionResult(resultJson = null, error = "WebView cannot navigate back"))
                    return
                }
                // canGoBack()/goBack() can ignore same-document pushState
                // entries even though the authoritative copied list exposes a
                // previous item. Use that list for admission and the document's
                // History traversal; the native history callback still owns
                // destination authentication and completion.
                beginNavigation(target.toString(), isHistoryNavigation = true)
                fixedScript("window.history.back();") {}
                if (isSameDocumentNavigation(current, target)) {
                    webView.post { observeSameDocumentNavigation(target.toString()) }
                }
                onResult(
                    LocalAppUiExecutionResult(
                        resultJson = jsonObjectString("ok" to true, "action" to "back"),
                        error = null,
                    ),
                )
            }
            LocalAppUiAutomationAction.Reload -> {
                beginNavigation(webView.url ?: initialUrl)
                webView.reload()
                onResult(
                    LocalAppUiExecutionResult(
                        resultJson = jsonObjectString("ok" to true, "action" to "reload"),
                        error = null,
                    ),
                )
            }
            is LocalAppUiAutomationAction.CaptureView -> captureFrame(
                webView = webView,
                value = action.value,
                qaCapture = qaCapture,
                onResult = onResult,
            )
            is LocalAppUiAutomationAction.Qa -> error("QA action must be unwrapped before execution")
        }
    }

    private fun isSameDocumentNavigation(current: Uri, target: Uri): Boolean =
        target.sameTrustedOrigin(current) &&
            target.path == current.path &&
            target.query == current.query &&
            target.fragment != current.fragment

    private fun attestQaResult(
        result: LocalAppUiExecutionResult,
        requested: URI,
        document: LocalAppQaDocument,
    ): LocalAppUiExecutionResult {
        return LocalAppUiExecutionResult(
            resultJson = buildLocalAppQaExecutionResult(
                originalResultJson = result.resultJson,
                operationError = result.error,
                requested = requested,
                document = document,
                platform = platform,
                formFactor = formFactor,
                width = webView.width,
                height = webView.height,
                devicePixelRatio = webView.resources.displayMetrics.density,
            ),
            error = null,
        )
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
     *
     * [value] is the optional crop request (see [LocalAppUiAutomationAction.CaptureView]).
     * When present and it parses, the region it names becomes `PixelCopy`'s
     * SOURCE rect and the destination bitmap is sized from the CROP, never
     * from the whole view — cropping a bitmap that `PixelCopy` already
     * returned at the whole-view's capped resolution would sample out of a
     * frame already thrown away ~2.3x too much detail.
     */
    private fun captureFrame(
        webView: WebView,
        value: String?,
        qaCapture: Boolean = false,
        expectedQaDocument: LocalAppQaDocument? = null,
        onResult: (LocalAppUiExecutionResult) -> Unit,
    ) {
        if (qaCapture) {
            val currentDocument = webView.url?.asUriOrNull()
            val document = currentDocument?.let { observedURL ->
                qaDocumentState.currentDocument()
                    ?.takeIf { it.loadedRuntimeUrl == observedURL.toString() }
            }
            if (document == null) {
                onResult(
                    LocalAppUiExecutionResult(
                        resultJson = null,
                        error = LOCAL_APP_CAPTURE_FRAME_FENCE_UNAVAILABLE_ERROR,
                    ),
                )
                return
            }
            requestVisualFrameFence(document.loadedRuntimeUrl, document.navigationGeneration) { ready ->
                if (!ready) {
                    onResult(
                        LocalAppUiExecutionResult(
                            resultJson = null,
                            error = LOCAL_APP_CAPTURE_FRAME_FENCE_UNAVAILABLE_ERROR,
                        ),
                    )
                } else {
                    captureFrame(
                        webView = webView,
                        value = value,
                        expectedQaDocument = document,
                        onResult = onResult,
                    )
                }
            }
            return
        }
        if (expectedQaDocument != null) {
            val currentDocument = webView.url?.asUriOrNull()
            val current = currentDocument?.let { observedURL ->
                qaDocumentState.currentDocument(requireVisualFrame = true)
                    ?.takeIf { it.loadedRuntimeUrl == observedURL.toString() }
            }
            if (current == null || current != expectedQaDocument) {
                onResult(
                    LocalAppUiExecutionResult(
                        resultJson = null,
                        error = LOCAL_APP_CAPTURE_FRAME_FENCE_UNAVAILABLE_ERROR,
                    ),
                )
                return
            }
        }
        val width = webView.width
        val height = webView.height
        if (width <= 0 || height <= 0) {
            onResult(
                LocalAppUiExecutionResult(
                    resultJson = null,
                    error = LOCAL_APP_CAPTURE_UNAVAILABLE_ERROR,
                ),
            )
            return
        }
        val density = webView.resources.displayMetrics.density

        // `localCrop`, once computed, is THE clamped region for the rest of
        // this call: it drives the PixelCopy source rect below AND (in
        // `finishCapture`) the reported `capture_rect` — never two
        // independent derivations of the same rectangle.
        val requestedRect = parseRequestedCaptureRect(value)
        val localCrop = requestedRect?.let { cropSourceRect(it, density, width, height) }
        if (localCrop != null && localCrop.isEmpty()) {
            // Clamped to nothing: the requested rect was entirely outside the
            // viewport. Fail the same way an offscreen view does, rather than
            // silently returning the whole frame for a region nobody asked for.
            onResult(
                LocalAppUiExecutionResult(
                    resultJson = null,
                    error = LOCAL_APP_CAPTURE_UNAVAILABLE_ERROR,
                ),
            )
            return
        }

        // Cap the long edge before encoding: a 3x tablet view is several
        // megabytes of bitmap before the quality ladder ever runs. A crop is
        // capped on ITS OWN long edge via the same helper below, never the
        // view's — `cropTargetSize` is algebraically the same formula as the
        // inline one here, just parameterised on the region actually being
        // captured instead of always the whole view.
        val maxEdge = 1_024
        val scale: Float
        val targetWidth: Int
        val targetHeight: Int
        if (localCrop != null) {
            val (cropWidth, cropHeight) = cropTargetSize(localCrop, maxEdge)
            targetWidth = cropWidth
            targetHeight = cropHeight
            // Only consulted below for the whole-view software-draw path,
            // which a crop never reaches (see the two `localCrop != null`
            // branches after `hostActivityWindow`/`PixelCopy` below).
            scale = 1f
        } else {
            scale = if (maxOf(width, height) > maxEdge) {
                maxEdge.toFloat() / maxOf(width, height).toFloat()
            } else {
                1f
            }
            targetWidth = maxOf(1, (width * scale).toInt())
            targetHeight = maxOf(1, (height * scale).toInt())
        }

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

        // A crop that cannot go through PixelCopy (no window, or PixelCopy
        // itself refused the rect) fails outright rather than falling back to
        // a whole-view software draw: the software path draws the whole
        // `WebView` with no source-rect concept, so honouring a crop there
        // would need Canvas transform math this file has no way to verify —
        // and a wrong crop with a `capture_rect` that vouches for it is worse
        // than a clean failure.
        fun failCropUnavailable() {
            bitmap.recycle()
            onResult(LocalAppUiExecutionResult(resultJson = null, error = LOCAL_APP_CAPTURE_UNAVAILABLE_ERROR))
        }

        // `PixelCopy` scales the source rect into whatever bitmap it is handed,
        // so the long-edge cap above is applied by the copy itself rather than
        // by allocating a full-resolution frame first and shrinking it after.
        val window = webView.hostActivityWindow()
        if (window == null) {
            if (localCrop != null) {
                failCropUnavailable()
                return
            }
            // No Activity window to copy from — a detached or test host. The
            // software draw still captures DOM chrome, which is worth more than
            // an error, and `render_check` has the canvas count to tell the
            // agent the drawn surface is the part it cannot trust.
            finishCapture(webView, bitmap, scale, softwareDraw = true, width, height, targetWidth, targetHeight, null, density, onResult)
            return
        }
        val location = IntArray(2)
        webView.getLocationInWindow(location)
        // `localCrop` is in the WebView's OWN pixel space (0,0 = the view's
        // top-left, matching the CSS space `capture_rect`/`viewport` report);
        // `PixelCopy` on a `Window` needs WINDOW-relative pixels, hence the
        // `location` offset — the same offset the whole-view rect below has
        // always used.
        val source = if (localCrop != null) {
            android.graphics.Rect(
                location[0] + localCrop.left,
                location[1] + localCrop.top,
                location[0] + localCrop.right,
                location[1] + localCrop.bottom,
            )
        } else {
            android.graphics.Rect(
                location[0],
                location[1],
                location[0] + width,
                location[1] + height,
            )
        }
        try {
            android.view.PixelCopy.request(
                window,
                source,
                bitmap,
                { status ->
                    when {
                        status == android.view.PixelCopy.SUCCESS ->
                            finishCapture(webView, bitmap, scale, false, width, height, targetWidth, targetHeight, localCrop, density, onResult)
                        localCrop != null -> failCropUnavailable()
                        else ->
                            finishCapture(webView, bitmap, scale, true, width, height, targetWidth, targetHeight, null, density, onResult)
                    }
                },
                android.os.Handler(android.os.Looper.getMainLooper()),
            )
        } catch (error: IllegalArgumentException) {
            // The rect can leave the window between measuring and requesting —
            // a scroll or a rotation is enough. Fall back rather than fail,
            // unless a crop made that fallback unable to honour what was asked.
            if (localCrop != null) {
                failCropUnavailable()
            } else {
                finishCapture(webView, bitmap, scale, true, width, height, targetWidth, targetHeight, null, density, onResult)
            }
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
     * Always null-crop by construction: see the `failCropUnavailable` calls
     * in `captureFrame`, which keep a crop from ever reaching this path.
     *
     * [localCrop], when non-null, is read for `capture_rect` — the SAME
     * clamped-region value `captureFrame` already used to build the
     * `PixelCopy` source rect and to size [targetWidth]/[targetHeight], not a
     * second computation from the original request.
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
        localCrop: LocalAppPxRect?,
        density: Float,
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
        // Present if and only if the request carried a `rect` — omitted
        // entirely (not `null`) for a whole-view capture, so that JSON stays
        // byte-for-byte what it was before crops existed. `viewport` above is
        // unchanged either way: it always reports the WHOLE view, so an agent
        // must branch on `capture_rect`'s presence — not on `image` being
        // smaller than `viewport` implies — to know a crop happened; see
        // `skills/frontend-qa/SKILL.md`'s "Canvas and WebGL surfaces" section
        // for the inversion formula this enables, matching iOS's field name
        // and semantics exactly.
        if (localCrop != null) {
            payload.put(
                "capture_rect",
                org.json.JSONObject()
                    .put("x", localCrop.left / density.toDouble())
                    .put("y", localCrop.top / density.toDouble())
                    .put("width", localCrop.width() / density.toDouble())
                    .put("height", localCrop.height() / density.toDouble()),
            )
        }
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
        qaDocumentState.detach()
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
        beginNavigation(url)
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

/**
 * A CSS-pixel rect as parsed from `capture_view`'s optional crop request
 * (`AppUiRequestDto.value` = `{"rect":{"x","y","width","height"}}`), before
 * density scaling or viewport clamping.
 */
internal data class LocalAppCssRect(val x: Double, val y: Double, val width: Double, val height: Double)

/**
 * A rect in real surface/view pixels: the shape `PixelCopy`'s source rect and
 * (divided by density) the reported `capture_rect` are both read from — never
 * two independent derivations of one rectangle.
 *
 * Plain `Int` fields, not `android.graphics.Rect`/`RectF`: this module's JVM
 * unit tests run against the Android Gradle Plugin's mockable `android.jar`
 * (`testOptions.unitTests.isReturnDefaultValues = true` in
 * `app/build.gradle.kts`), which replaces EVERY `android.graphics.Rect`
 * constructor and method body with one that returns the return type's
 * default value — confirmed empirically before writing this: a throwaway
 * `@Test` constructing `Rect(10, 20, 130, 100)` and printing its fields read
 * back `left=0, top=0, right=0, bottom=0, width()=0, isEmpty=false` (the
 * constructor never set the fields, and `width()`/`isEmpty()` ignored field
 * state and returned their type defaults regardless). A real
 * `android.graphics.Rect` is built from this type only at the `PixelCopy`
 * call site in `captureFrame`, where no test ever inspects its fields — on a
 * real device (no mockable jar involved) `android.graphics.Rect` behaves
 * normally.
 */
internal data class LocalAppPxRect(val left: Int, val top: Int, val right: Int, val bottom: Int) {
    fun width(): Int = right - left
    fun height(): Int = bottom - top
    fun isEmpty(): Boolean = left >= right || top >= bottom
}

/**
 * CSS rect -> `PixelCopy` source rect, in real surface pixels, clamped to the
 * view's own pixel bounds.
 *
 * `× density` is the step this repo has already got wrong once (see the note
 * above `viewport` in `finishCapture`): a CSS pixel is not a surface pixel,
 * and `PixelCopy` samples the surface.
 *
 * A degenerate input (e.g. a negative `width`, which the engine host already
 * rejects server-side — this function does not re-check sign, only the
 * caller's finiteness parse) standardizes safely here rather than crashing:
 * `right`/`bottom`'s `coerceIn(left, ...)`/`coerceIn(top, ...)` cannot go
 * below `left`/`top`, so the worst case is an empty (zero-size) result,
 * caught by [LocalAppPxRect.isEmpty].
 */
internal fun cropSourceRect(cssRect: LocalAppCssRect, density: Float, viewWidthPx: Int, viewHeightPx: Int): LocalAppPxRect {
    val left = (cssRect.x * density).toInt().coerceIn(0, viewWidthPx)
    val top = (cssRect.y * density).toInt().coerceIn(0, viewHeightPx)
    val right = ((cssRect.x + cssRect.width) * density).toInt().coerceIn(left, viewWidthPx)
    val bottom = ((cssRect.y + cssRect.height) * density).toInt().coerceIn(top, viewHeightPx)
    return LocalAppPxRect(left, top, right, bottom)
}

/**
 * Cap the CROP's own long edge — never the whole view's long edge.
 * `coerceAtMost(1f)` forbids enlargement: a crop smaller than [capPx] is
 * returned at its real resolution, not blown up to fill the cap.
 *
 * Algebraically the same formula `captureFrame`'s whole-view branch computes
 * inline (`scale = min(1, capPx / longEdge)`, then `dimension * scale`) —
 * this is only ever called with the CROP's own dimensions, so the two never
 * disagree for the no-crop case without ever being the same code path.
 */
internal fun cropTargetSize(sourcePx: LocalAppPxRect, capPx: Int): Pair<Int, Int> {
    val longEdge = maxOf(sourcePx.width(), sourcePx.height())
    if (longEdge <= 0) return 1 to 1
    val scale = (capPx.toFloat() / longEdge).coerceAtMost(1f)
    return maxOf(1, (sourcePx.width() * scale).toInt()) to maxOf(1, (sourcePx.height() * scale).toInt())
}

/**
 * Parse `capture_view`'s optional crop request out of the opaque
 * `AppUiRequestDto.value` string. Shape: `{"rect":{"x","y","width","height"}}`,
 * each field EITHER a JSON integer or a JSON floating-point number — the
 * engine host preserves the caller's original numeric form rather than
 * re-deriving one from a parsed `f64`, so a fractional CSS pixel is routine,
 * not an edge case.
 *
 * Returns null for anything absent, malformed, non-numeric, or non-finite,
 * which collapses to the same "whole view" behaviour as no `value` at all —
 * mirroring iOS's `parseRequestedRect`.
 */
internal fun parseRequestedCaptureRect(value: String?): LocalAppCssRect? {
    if (value == null) return null
    return try {
        val rect = org.json.JSONObject(value).optJSONObject("rect") ?: return null
        val x = rect.opt("x").asFiniteCssNumber() ?: return null
        val y = rect.opt("y").asFiniteCssNumber() ?: return null
        val w = rect.opt("width").asFiniteCssNumber() ?: return null
        val h = rect.opt("height").asFiniteCssNumber() ?: return null
        LocalAppCssRect(x, y, w, h)
    } catch (error: org.json.JSONException) {
        null
    }
}

// `is Number`, not a cast to a specific numeric type: verified empirically
// (a throwaway test against this module's `org.json:json` test dependency)
// that a whole-number JSON literal like `10` parses to `java.lang.Integer`
// while a fractional one like `10.5` parses to `java.math.BigDecimal` — NOT
// `Double` — under this specific org.json build, and Android's own bundled
// org.json (a different fork, used at runtime) is free to choose yet another
// concrete type again. Reading through the common `Number` supertype's
// `.toDouble()` is correct for any of them without depending on which one a
// a given org.json build picks, and it correctly rejects a JSON string (e.g.
// `"10"`) or boolean, neither of which is a `Number` — matching iOS's
// `parseRequestedRect`, which similarly avoids a bridging-shape-sensitive
// cast.
private fun Any?.asFiniteCssNumber(): Double? =
    (this as? Number)?.toDouble()?.takeIf { it.isFinite() }

internal fun buildLocalAppUiExecutionRequest(action: LocalAppUiAutomationAction): String {
    return when (action) {
        is LocalAppUiAutomationAction.Qa -> buildLocalAppUiExecutionRequest(action.action)
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
        is LocalAppUiAutomationAction.CaptureView -> error("Structured script is not used for $action")
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
      // A password / hidden input's CONTENT must never leave the WebView, in any
      // field. `snapshot` already redacts `value`, but the accessible name falls
      // back to `element.value` for an input with no label, no placeholder and
      // no text — so an unlabelled `<input type="password">` used to ship the
      // typed password to the model as `name`. `deepQuery` widened the reach of
      // this walk into shadow roots, so the fallback now sees component-internal
      // inputs too.
      const isSensitive = element =>
        element instanceof HTMLInputElement && ['hidden', 'password'].includes(element.type);
      const nameOf = element => {
        // `deepQuery` below returns elements from INSIDE shadow roots, and a
        // shadow root is its own id scope. Resolving their labels against
        // `document` searches the wrong tree, so an element the walk just
        // surfaced comes back unnamed and role+name targeting misses it.
        const scope = element.getRootNode?.() || document;
        const byId = id => (scope.getElementById ? scope.getElementById(id) : document.getElementById(id));
        const labelledBy = clean(element.getAttribute('aria-labelledby'));
        const labelled = labelledBy
          ? labelledBy.split(/\s+/).map(id => byId(id)).find(Boolean)
          : null;
        const explicit = element.id
          ? scope.querySelector(`label[for="${'$'}{CSS.escape(element.id)}"]`)
          : null;
        return clean(
          element.getAttribute('aria-label')
            || labelled?.textContent
            || explicit?.textContent
            || element.placeholder
            || element.innerText
            || (isSensitive(element) ? '' : element.value)
        );
      };
      // Ionic renders a component's interactive internals inside a SHADOW ROOT, and
      // `document.querySelectorAll` does not cross that boundary. Without this walk
      // an app built from ion-* components reports `elements: []` — the same signal
      // a blank screen and a crashed app produce, which is exactly the ambiguity
      // `canvasCount` exists to resolve for a drawn surface.
      const SELECTOR = 'button,a[href],input,select,textarea,[role],[tabindex],[contenteditable="true"]';
      const deepQuery = (selector, limit) => {
        const found = [];
        const seen = new Set();
        const visit = (root, depth) => {
          // Bounded on BOTH axes: a component library nests a few roots deep, but a
          // page that nests further (or loops) must degrade to a partial list rather
          // than hang the WebView while the tool call waits on it.
          if (!root || depth > 8 || found.length >= limit) return;
          let matched = [];
          try { matched = Array.from(root.querySelectorAll(selector)); } catch (error) { return; }
          for (const element of matched) {
            if (found.length >= limit) return;
            if (!seen.has(element)) { seen.add(element); found.push(element); }
          }
          let hosts = [];
          try { hosts = Array.from(root.querySelectorAll('*')); } catch (error) { return; }
          for (const host of hosts) if (host.shadowRoot) visit(host.shadowRoot, depth + 1);
        };
        visit(document, 0);
        return found;
      };
      const candidates = () => deepQuery(SELECTOR, 400);
      // `ion-input` and its siblings keep the real <input> in their shadow root, so
      // a fill aimed at the host would write to a custom element that has no value
      // setter and no `input` event to dispatch.
      const nativeControl = element => {
        if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement || element instanceof HTMLSelectElement) return element;
        return element.shadowRoot ? (element.shadowRoot.querySelector('input,textarea,select') || element) : element;
      };
      const findTarget = target => {
        if (!target) return null;
        if (target.elementId) {
          // `getElementById` is document-scoped; a shadow root is a separate id
          // scope, and Ionic mints ids like `ion-input-0` inside one.
          const byId = document.getElementById(target.elementId)
            || deepQuery('[id="' + String(target.elementId).replace(/["\\]/g, '\\$&') + '"]', 1)[0];
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
          // Deliberately kept CHARACTER-IDENTICAL to the iOS source
          // (LocalAppWebView.swift's snapshot()) and to the plan's Step 3
          // text, so its indentation intentionally differs from its
          // neighbours below. Reformatting it would break the cross-platform
          // identity that LocalAppsStoreTests (iOS) and LocalAppWebViewTest
          // (Android) both pin.
          const vvOf = () => {
            const vv = window.visualViewport;
            return vv
              ? { width: Math.round(vv.width), height: Math.round(vv.height),
                  offsetLeft: Math.round(vv.offsetLeft), offsetTop: Math.round(vv.offsetTop),
                  scale: vv.scale }
              : { width: Math.round(window.innerWidth), height: Math.round(window.innerHeight),
                  offsetLeft: 0, offsetTop: 0, scale: 1 };
          };
          const rectOf = element => {
            const rect = element.getBoundingClientRect();
            return [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)];
          };
          const snapshot = () => {
            const out = {
              title: clean(document.title),
              url: location.href,
              documentState: document.readyState,
              viewport: vvOf(),
              runtimeErrors: (window.__lingxiRuntimeErrors || []).slice(0, 8),
              runtimeErrorsDropped: window.__lingxiRuntimeErrorsDropped || 0,
              // The host gate compares ONLY the pixels inside these rects, so a
              // DOM spinner cannot stand in for a frozen canvas. `canvasCount`
              // stays for compatibility with readers that only counted.
              canvases: deepQuery('canvas', 16).map(c => ({ rect: rectOf(c) })),
              canvasCount: deepQuery('canvas', 64).length,
              elements: candidates().slice(0, 200).map(element => {
                const rect = element.getBoundingClientRect();
                const sensitive = isSensitive(element);
                return {
                  elementId: clean(element.id) || null,
                  role: roleOf(element) || null,
                  name: nameOf(element) || null,
                  value: sensitive ? null : clean(element.value),
                  checked: typeof element.checked === 'boolean' ? element.checked : null,
                  disabled: !!element.disabled,
                  visible: rect.width > 0 && rect.height > 0,
                  rect: [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)],
                };
              }),
              truncated: [],
            };
            // 256 KiB is a HARD failure on the result channel on iOS, and
            // Android enforces no cap on `inspect` at all — so this ladder is
            // the only thing bounding the payload there either way. Leave
            // room for the JSON envelope and degrade in a fixed order rather
            // than dying (iOS) or growing unbounded (Android). Measured in
            // UTF-8 BYTES via TextEncoder, matching the native guard this
            // budget exists to stay under (`resultJSON.utf8.count` on iOS) —
            // `.length` counts UTF-16 code units, and CJK text is 1 unit but
            // 3 bytes per character, so a length-only check could call a
            // payload "safe" at roughly a third of its real size.
            // `window.TextEncoder` is page-controllable, so this uses the
            // reference the document-start bootstrap captured before page code
            // ran; falling back to the global only where no bootstrap ran.
            const BUDGET = 200 * 1024;
            const truncated = out.truncated;
            const size = () => new (window.__lingxiTextEncoder || TextEncoder)().encode(JSON.stringify(out)).length;
            for (const seg of ['elements', 'canvases', 'runtimeErrors']) {
              if (size() <= BUDGET) break;
              if (seg === 'elements') out.elements = out.elements.slice(0, 50);
              else if (seg === 'canvases') out.canvases = [];
              else out.runtimeErrors = [];
              truncated.push(seg);
            }
            return out;
          };
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
            : (deepQuery('canvas', 1)[0] || document.body);
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
          const field = nativeControl(element);
          if (!(field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement || field.isContentEditable)) {
            throw new Error('Target cannot be filled');
          }
          if (field.isContentEditable) {
            field.textContent = request.value || '';
          } else {
            setNativeValue(field, request.value || '');
          }
          dispatchValueChange(field);
        } else if (request.action === 'select') {
          const field = nativeControl(element);
          if (!(field instanceof HTMLSelectElement)) throw new Error('Target is not a select element');
          const option = Array.from(field.options).find(item =>
            item.value === request.value || clean(item.textContent) === clean(request.value)
          );
          if (!option) throw new Error('Select option was not found');
          setNativeValue(field, option.value);
          dispatchValueChange(field);
        } else if (request.action === 'toggle') {
          const desired = !!request.checked;
          // `ion-checkbox`/`ion-toggle` keep the real <input> — and the
          // `role`/`aria-checked` that describe it — inside their SHADOW ROOT,
          // exactly like `ion-input`. Reading the host alone answered "not
          // toggleable" for every checkbox and switch in every Ionic app, so
          // read the state from whichever node carries it. The CLICK still goes
          // to the host: that is what the component listens on.
          const stateOf = node => {
            if (node instanceof HTMLInputElement && (node.type === 'checkbox' || node.type === 'radio')) {
              return !!node.checked;
            }
            if (['checkbox', 'switch'].includes(roleOf(node).toLowerCase())) {
              return clean(node.getAttribute('aria-checked')).toLowerCase() === 'true';
            }
            return null;
          };
          const current = stateOf(nativeControl(element)) ?? stateOf(element);
          if (current === null) throw new Error('Target is not toggleable');
          if (current !== desired) element.click();
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
    // Bumped when the renderer dies: the dead WebView is torn down and rebuilt.
    var rendererGeneration by remember(appId) { mutableIntStateOf(0) }
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

    key(rendererGeneration) {
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
                            controller?.onPageStarted(url)
                            if (!Uri.parse(url).sameTrustedOrigin(trustedOrigin)) view.stopLoading()
                        }

                        override fun onPageFinished(view: WebView, url: String) {
                            controller?.onPageFinished(url)
                        }

                        override fun doUpdateVisitedHistory(view: WebView, url: String, isReload: Boolean) {
                            controller?.onVisitedHistoryUpdated(url, isReload)
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

                        override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
                            // Handled, so the renderer's death does not take the app
                            // down: detach and destroy the dead view, then rebuild.
                            if (webView !== view) return true
                            controller?.let { attached ->
                                LocalAppWebViewRegistry.unregister(appId, attached)
                                attached.detach()
                            }
                            runCatching { WebViewCompat.removeWebMessageListener(view, LINGXI_V1_MESSAGE_OBJECT) }
                            (view.parent as? ViewGroup)?.removeView(view)
                            view.destroy()
                            controller = null
                            webView = null
                            rendererGeneration++
                            return true
                        }
                    }
                    webViewClient = guardedClient
                    controller = LocalAppWebViewController(this, broker, guardedClient, url).also { attached ->
                        LocalAppWebViewRegistry.register(appId, attached)
                        currentControllerHandler(attached)
                        attached.beginNavigation(url)
                    }
                    tag = url
                    loadUrl(url)
                }
            },
            update = { view ->
                if (view.tag != url) {
                    view.tag = url
                    controller?.beginNavigation(url)
                    view.loadUrl(url)
                }
            },
        )
    }

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

/** Resolve page navigation like `new URL(value, location.href)` without
 * carrying the current page's ordinary query parameters across routes. */
internal fun resolveLocalAppNavigation(currentUrl: String, requested: String): String {
    val base = runCatching { URI(currentUrl) }.getOrNull() ?: return requested
    val destination = runCatching { URI(requested) }.getOrNull() ?: return requested
    val baseWithoutQueryOrFragment = runCatching {
        URI(base.toString().substringBefore('#').substringBefore('?'))
    }.getOrNull() ?: return requested
    // Preserve an explicit authority (including a scheme-relative URL) so the
    // caller's same-origin gate can reject it instead of silently rebasing it.
    val resolved = if (destination.isAbsolute || destination.rawAuthority != null) {
        destination
    } else if (destination.rawPath.isNullOrEmpty()) {
        // java.net.URI resolves `?query` against the containing directory and
        // resolves `#fragment` by inheriting the complete old query. Neither
        // matches the Local App contract: retain the document path, but let
        // the destination own its query/fragment so old business parameters
        // cannot leak into the next route.
        val query = destination.rawQuery?.let { "?$it" }.orEmpty()
        val fragment = destination.rawFragment?.let { "#$it" }.orEmpty()
        URI("$baseWithoutQueryOrFragment$query$fragment")
    } else {
        baseWithoutQueryOrFragment.resolve(destination)
    }
    val marker = rawRuntimeMarker(base.rawQuery)
    val target = if (marker != null) {
        replaceRuntimeMarker(resolved.toString(), resolved.rawQuery, marker)
    } else {
        resolved.toString()
    }
    return target
}

private fun rawRuntimeMarker(rawQuery: String?): String? =
    rawQuery.orEmpty().split('&').filter { item ->
        item.substringBefore('=') == "lingxi_runtime" && item.contains('=')
    }.singleOrNull()?.substringAfter('=')

private fun replaceRuntimeMarker(raw: String, rawQuery: String?, marker: String): String {
    val fragmentStart = raw.indexOf('#')
    val beforeFragment = if (fragmentStart >= 0) raw.substring(0, fragmentStart) else raw
    val fragment = if (fragmentStart >= 0) raw.substring(fragmentStart) else ""
    val withoutQuery = beforeFragment.substringBefore('?')
    val ordinaryItems = rawQuery.orEmpty().split('&').filter { item ->
        item.isNotEmpty() && !isRuntimeMarkerKey(item.substringBefore('='))
    }
    val query = (ordinaryItems + "lingxi_runtime=$marker").joinToString("&")
    return "$withoutQuery?$query$fragment"
}

/** Match URLSearchParams' percent-decoded key semantics for this ASCII-only
 * reserved name without pulling Android URI decoding into pure JVM tests. */
private fun isRuntimeMarkerKey(rawKey: String): Boolean {
    val expected = "lingxi_runtime"
    var rawIndex = 0
    var expectedIndex = 0
    while (rawIndex < rawKey.length && expectedIndex < expected.length) {
        val actual = if (rawKey[rawIndex] == '%' && rawIndex + 2 < rawKey.length) {
            val high = rawKey[rawIndex + 1].digitToIntOrNull(16) ?: return false
            val low = rawKey[rawIndex + 2].digitToIntOrNull(16) ?: return false
            rawIndex += 3
            (high * 16 + low).toChar()
        } else {
            rawKey[rawIndex++]
        }
        if (actual != expected[expectedIndex++]) return false
    }
    return rawIndex == rawKey.length && expectedIndex == expected.length
}

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
      // Deliberately kept CHARACTER-IDENTICAL to the iOS source
      // (LocalAppWebView.swift's bridgeSourceTemplate) and to the
      // plan's Step 3 text, so its indentation intentionally differs
      // from its neighbours here. Reformatting it would break the
      // cross-platform identity that LocalAppsStoreTests (iOS) and
      // LocalAppWebViewTest (Android) both pin.
      // Criterion 6's evidence. A React ErrorBoundary swallows a render crash
      // into console.error and never reaches window.onerror, so the console
      // hook is the one that actually catches the shipped templates' failure
      // mode. Bounded on count AND per-field length: `result_json` is capped
      // at 256 KiB and FAILS rather than truncating.
      if (!window.__lingxiRuntimeErrors) {
        window.__lingxiRuntimeErrors = [];
        window.__lingxiRuntimeErrorsDropped = 0;
        const cap = 8;
        const trim = (v, n) => String(v == null ? '' : v).replace(/\s+/g, ' ').slice(0, n);
        const push = entry => {
          if (window.__lingxiRuntimeErrors.length >= cap) { window.__lingxiRuntimeErrorsDropped += 1; return; }
          window.__lingxiRuntimeErrors.push(entry);
        };
        window.addEventListener('error', e => push({
          kind: 'error', message: trim(e.message, 200), source: trim(e.filename, 120),
          line: e.lineno | 0, column: e.colno | 0, at_ms: Date.now(),
        }));
        window.addEventListener('unhandledrejection', e => push({
          kind: 'rejection',
          // Never expand an arbitrary rejection value — take a safe string only.
          message: trim(e.reason && e.reason.message ? e.reason.message : e.reason, 200),
          source: '', line: 0, column: 0, at_ms: Date.now(),
        }));
        const nativeConsoleError = console.error.bind(console);
        console.error = function () {
          try {
            push({ kind: 'console', message: trim(Array.from(arguments).map(a => (a && a.message) ? a.message : a).join(' '), 200),
                   source: '', line: 0, column: 0, at_ms: Date.now() });
          } catch (ignored) { /* never let the ledger break the page */ }
          return nativeConsoleError.apply(console, arguments);
        };
      }
      // Same trick, same reason, for TextEncoder: the inspect snapshot's byte
      // budget measures with it, and `window.TextEncoder` is page-controllable,
      // so an app that shadows it made snapshot() THROW where the old
      // `.length` could not — killing inspect_ui outright. Capture the real
      // one here, at document-start, before any page code has run.
      // Non-writable and non-configurable (defineProperty's defaults), so the
      // page cannot take it back afterwards either.
      if (!window.__lingxiTextEncoder) {
        Object.defineProperty(window, '__lingxiTextEncoder', { value: window.TextEncoder });
      }
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
