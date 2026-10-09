package com.lingxi.code.conversation

import android.annotation.SuppressLint
import android.net.Uri
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.GeolocationPermissions
import android.webkit.PermissionRequest
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.webkit.JavaScriptReplyProxy
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import androidx.webkit.WebViewRenderProcess
import androidx.webkit.WebViewRenderProcessClient
import com.lingxi.code.BuildConfig
import com.lingxi.code.R
import com.lingxi.code.bindings.runtime.VisualizationHost
import com.lingxi.code.bindings.runtime.VisualizationMountDto
import com.lingxi.code.bindings.runtime.VisualizationThemeDto
import com.lingxi.code.components.LXIcon
import com.lingxi.code.components.LXIconName
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import org.json.JSONTokener
import java.io.ByteArrayInputStream
import java.util.Locale

/** The Android visualization WebView's dedicated, never-resolvable origin. */
const val VISUALIZATION_ORIGIN = "https://lingxi-visualization.invalid"
private const val VISUALIZATION_HOST_NAME = "lingxi-visualization.invalid"
private const val VISUALIZATION_MESSAGE_OBJECT = "lingxiVisualization"
private const val MAX_MESSAGE_CHARACTERS = 64 * 1024
private const val MIN_HEIGHT_DP = 32
private const val MAX_INLINE_HEIGHT_DP = 640

/** What a transcript widget needs from its screen. */
data class VisualizationScreenContext(
    val host: VisualizationHost?,
    val sessionId: String,
    val dark: Boolean,
    val onFollowup: (VisualizationFollowup) -> Unit,
)

val LocalVisualizationContext = staticCompositionLocalOf<VisualizationScreenContext?> { null }

/**
 * The request path [VisualizationHost.serve] answers, or null for anything
 * off the visualization origin (blocked) or not a plain GET.
 */
internal fun visualizationRequestPath(url: Uri, method: String): String? {
    if (!method.equals("GET", ignoreCase = true)) return null
    if (url.scheme != "https" || url.host != VISUALIZATION_HOST_NAME || url.port != -1) return null
    if (url.encodedQuery != null || url.encodedFragment != null || url.encodedUserInfo != null) return null
    return url.encodedPath?.takeIf { it.startsWith("/") }
}

private fun blockedResponse(): WebResourceResponse =
    WebResourceResponse("text/plain", "utf-8", 404, "Not Found", emptyMap(), ByteArrayInputStream(ByteArray(0)))

/** One inline widget in the transcript: a placeholder, a live card or a note. */
@Composable
fun VisualizationCard(
    status: VisualizationSlotStatus,
    reference: VisualizationRef?,
    modifier: Modifier = Modifier,
) {
    val context = LocalVisualizationContext.current
    Box(modifier = modifier.fillMaxWidth().padding(bottom = 18.dp)) {
        when {
            status == VisualizationSlotStatus.Pending ->
                VisualizationNote(stringResource(R.string.visualization_preparing), busy = true)
            status == VisualizationSlotStatus.Ready && reference != null && context?.host != null &&
                context.sessionId.isNotEmpty() && webMessagingSupported() ->
                key(context.sessionId, reference) {
                    VisualizationLiveCard(context, reference)
                }
            else -> VisualizationNote(stringResource(R.string.visualization_unavailable), busy = false)
        }
    }
}

private fun webMessagingSupported(): Boolean =
    WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)

private enum class WidgetPhase { Loading, Ready, Unavailable }

/** Mutable relay state for one mounted widget, owned by its composition. */
private class WidgetSession(
    val host: VisualizationHost,
    val sessionId: String,
    val reference: VisualizationRef,
    val scope: CoroutineScope,
) {
    var webView: WebView? = null
    var reply: JavaScriptReplyProxy? = null
    var mount: VisualizationMountDto? = null

    fun post(message: JSONObject) {
        reply?.postMessage(message.toString())
    }

    /** Retire the current mount; its late replies and writes are refused. */
    fun retire() {
        val token = mount?.token ?: return
        mount = null
        scope.launch(Dispatchers.IO) { runCatching { host.unmount(token) } }
    }

    fun destroy() {
        retire()
        reply = null
        webView?.let { view ->
            runCatching { WebViewCompat.removeWebMessageListener(view, VISUALIZATION_MESSAGE_OBJECT) }
            (view.parent as? ViewGroup)?.removeView(view)
            view.destroy()
        }
        webView = null
    }
}

@SuppressLint("SetJavaScriptEnabled")
@Composable
private fun VisualizationLiveCard(context: VisualizationScreenContext, reference: VisualizationRef) {
    val scope = rememberCoroutineScope()
    val session = remember { WidgetSession(context.host!!, context.sessionId, reference, scope) }
    var phase by remember { mutableStateOf(WidgetPhase.Loading) }
    var heightDp by remember { mutableIntStateOf(160) }
    var title by remember { mutableStateOf("") }
    // Bumped when the renderer dies: the old WebView is unusable and is rebuilt.
    var generation by remember { mutableIntStateOf(0) }
    val onFollowup by rememberUpdatedState(context.onFollowup)
    val dark by rememberUpdatedState(context.dark)
    var mountedDark by remember { mutableStateOf(context.dark) }
    val locale = remember { Locale.getDefault().toLanguageTag() }

    fun mountWidget() {
        session.retire()
        phase = WidgetPhase.Loading
        scope.launch {
            val ticket = withContext(Dispatchers.IO) {
                runCatching {
                    session.host.mount(
                        sessionId = session.sessionId,
                        id = reference.id,
                        revision = reference.revision,
                        themeDto = VisualizationThemeDto(dark = dark, tokens = emptyMap()),
                        locale = locale,
                        expanded = false,
                    )
                }.getOrNull()
            }
            if (ticket == null) {
                phase = WidgetPhase.Unavailable
                return@launch
            }
            session.mount = ticket
            title = ticket.title
            session.post(
                JSONObject()
                    .put("type", "mount")
                    .put("generation", ticket.generation.toLong())
                    .put("docUrl", ticket.docUrl)
                    .put("title", ticket.title)
                    .put("maxHeight", MAX_INLINE_HEIGHT_DP)
                    .put("expanded", false),
            )
        }
    }

    fun saveState(body: JSONObject, mount: VisualizationMountDto) {
        val requestId = body.opt("requestId") as? Number ?: return
        val baseVersion = (body.opt("baseVersion") as? Number)?.toLong()?.takeIf { it >= 0 } ?: return
        val modelContent = body.opt("modelContent") as? String ?: return
        val privateContent = body.opt("privateContent") as? String ?: return
        scope.launch {
            val result = withContext(Dispatchers.IO) {
                runCatching {
                    session.host.writeState(
                        token = mount.token,
                        generation = mount.generation,
                        baseVersion = baseVersion.toULong(),
                        modelContentJson = modelContent,
                        privateContentJson = privateContent,
                    )
                }.getOrNull()
            }
            if (session.mount?.token != mount.token) return@launch
            val reply = JSONObject()
                .put("generation", mount.generation.toLong())
                .put("requestId", requestId)
            if (result?.saved == true) {
                session.post(reply.put("type", "state.saved").put("version", result.version.toLong()))
            } else {
                val state = result?.currentStateJson
                    ?.let { runCatching { JSONTokener(it).nextValue() }.getOrNull() }
                    ?: JSONObject.NULL
                session.post(
                    reply.put("type", "state.rejected")
                        .put("reason", result?.reason ?: "rejected")
                        .put("state", state),
                )
            }
        }
    }

    fun receive(raw: String) {
        if (raw.length > MAX_MESSAGE_CHARACTERS) return
        val body = runCatching { JSONObject(raw) }.getOrNull() ?: return
        val type = body.optString("type")
        if (type == "shell.ready") {
            mountWidget()
            return
        }
        val mount = session.mount ?: return
        if ((body.opt("generation") as? Number)?.toLong() != mount.generation.toLong()) return
        when (type) {
            "ready" -> phase = WidgetPhase.Ready
            "resize" -> (body.opt("height") as? Number)?.toDouble()
                ?.takeIf { it.isFinite() }
                ?.let { heightDp = kotlin.math.ceil(it).toInt().coerceIn(MIN_HEIGHT_DP, MAX_INLINE_HEIGHT_DP) }
            "state.save" -> saveState(body, mount)
            "followup.draft" -> body.optString("text").takeIf { it.isNotBlank() }?.let { text ->
                onFollowup(
                    VisualizationFollowup(
                        text = text,
                        chip = VisualizationContextChip(reference.id, reference.revision, mount.title),
                    ),
                )
            }
            "crashed" -> {
                session.retire()
                session.webView?.loadUrl(session.host.shellUrl())
            }
        }
    }

    // A theme change remounts: documents are single-use and carry their theme.
    LaunchedEffect(context.dark) {
        if (context.dark == mountedDark) return@LaunchedEffect
        mountedDark = context.dark
        session.retire()
        session.webView?.loadUrl(session.host.shellUrl())
    }

    val label = title.ifEmpty { stringResource(R.string.visualization_label) }
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .semantics { contentDescription = label },
    ) {
        if (phase == WidgetPhase.Unavailable) {
            VisualizationNote(stringResource(R.string.visualization_unavailable), busy = false)
        } else {
            key(generation) {
                AndroidView(
                    modifier = Modifier.fillMaxWidth().height(heightDp.dp).testTag("conversation.visualization"),
                    factory = { viewContext ->
                        WebView(viewContext).apply view@{
                            session.webView = this
                            WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)
                            setBackgroundColor(android.graphics.Color.TRANSPARENT)
                            isVerticalScrollBarEnabled = false
                            isHorizontalScrollBarEnabled = false
                            overScrollMode = WebView.OVER_SCROLL_NEVER
                            settings.javaScriptEnabled = true
                            settings.domStorageEnabled = false
                            settings.databaseEnabled = false
                            settings.allowFileAccess = false
                            settings.allowContentAccess = false
                            @Suppress("DEPRECATION")
                            settings.allowFileAccessFromFileURLs = false
                            @Suppress("DEPRECATION")
                            settings.allowUniversalAccessFromFileURLs = false
                            settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
                            settings.setGeolocationEnabled(false)
                            settings.setSupportMultipleWindows(false)
                            settings.javaScriptCanOpenWindowsAutomatically = false
                            settings.mediaPlaybackRequiresUserGesture = true
                            settings.cacheMode = WebSettings.LOAD_NO_CACHE
                            // Every document and asset is answered in-process;
                            // nothing may reach the network.
                            settings.blockNetworkLoads = true
                            CookieManager.getInstance().setAcceptThirdPartyCookies(this, false)
                            WebViewCompat.addWebMessageListener(
                                this,
                                VISUALIZATION_MESSAGE_OBJECT,
                                setOf(VISUALIZATION_ORIGIN),
                            ) { _, message, sourceOrigin, isMainFrame, replyProxy ->
                                if (!isMainFrame || sourceOrigin.toString().trimEnd('/') != VISUALIZATION_ORIGIN) {
                                    return@addWebMessageListener
                                }
                                session.reply = replyProxy
                                receive(message.data.orEmpty())
                            }
                            webChromeClient = object : WebChromeClient() {
                                override fun onPermissionRequest(request: PermissionRequest) = request.deny()

                                override fun onGeolocationPermissionsShowPrompt(
                                    origin: String?,
                                    callback: GeolocationPermissions.Callback?,
                                ) {
                                    callback?.invoke(origin, false, false)
                                }
                            }
                            // A hung widget would stall every WebView sharing its
                            // renderer; terminating it lands in onRenderProcessGone.
                            if (WebViewFeature.isFeatureSupported(WebViewFeature.WEB_VIEW_RENDERER_CLIENT_BASIC_USAGE)) {
                                WebViewCompat.setWebViewRenderProcessClient(
                                    this,
                                    object : WebViewRenderProcessClient() {
                                        override fun onRenderProcessUnresponsive(
                                            view: WebView,
                                            renderer: WebViewRenderProcess?,
                                        ) {
                                            renderer?.terminate()
                                        }

                                        override fun onRenderProcessResponsive(
                                            view: WebView,
                                            renderer: WebViewRenderProcess?,
                                        ) = Unit
                                    },
                                )
                            }
                            webViewClient = object : WebViewClient() {
                                override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean =
                                    visualizationRequestPath(request.url, "GET") == null

                                override fun shouldInterceptRequest(
                                    view: WebView,
                                    request: WebResourceRequest,
                                ): WebResourceResponse {
                                    val path = visualizationRequestPath(request.url, request.method)
                                        ?: return blockedResponse()
                                    val response = runCatching { session.host.serve(path) }.getOrNull()
                                        ?: return blockedResponse()
                                    val headers = response.headers
                                        .filterNot { it.name.equals("Content-Type", ignoreCase = true) }
                                        .associate { it.name to it.value }
                                    val textual = response.mimeType.startsWith("text/") ||
                                        response.mimeType == "application/javascript"
                                    return WebResourceResponse(
                                        response.mimeType,
                                        if (textual) "utf-8" else null,
                                        response.status.toInt(),
                                        if (response.status.toInt() == 200) "OK" else "Not Found",
                                        headers,
                                        ByteArrayInputStream(response.body),
                                    )
                                }

                                override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
                                    // Handled: the app survives; this widget is rebuilt.
                                    if (session.webView === view) {
                                        session.destroy()
                                        phase = WidgetPhase.Loading
                                        generation++
                                    }
                                    return true
                                }
                            }
                            loadUrl(session.host.shellUrl())
                        }
                    },
                )
            }
            if (phase != WidgetPhase.Ready) {
                VisualizationNote(stringResource(R.string.visualization_loading), busy = true)
            }
        }
    }

    DisposableEffect(Unit) {
        onDispose { session.destroy() }
    }
}

@Composable
private fun VisualizationNote(text: String, busy: Boolean) {
    val t = LingXiTheme.palette
    val shape = RoundedCornerShape(12.dp)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = if (busy) 96.dp else 0.dp)
            .clip(shape)
            .background(t.surface)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.Top,
    ) {
        if (busy) CircularProgressIndicator(modifier = Modifier.size(14.dp), strokeWidth = 1.5.dp, color = t.text3)
        Text(text = text, color = t.text3, fontSize = 13.sp)
    }
}

/** The widget a user message (or the composer draft) follows up on. */
@Composable
fun VisualizationContextChipView(
    chip: VisualizationContextChip,
    modifier: Modifier = Modifier,
    onDismiss: (() -> Unit)? = null,
) {
    val t = LingXiTheme.palette
    val shape = RoundedCornerShape(10.dp)
    Row(
        modifier = modifier
            .widthIn(max = 280.dp)
            .clip(shape)
            .background(t.surface)
            .border(0.5.dp, t.border, shape)
            .padding(horizontal = 10.dp, vertical = 6.dp)
            .testTag("conversation.visualization.chip"),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = chip.title.ifEmpty { stringResource(R.string.visualization_label) },
            color = t.text,
            fontSize = 12.sp,
            maxLines = 1,
            overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false),
        )
        if (onDismiss != null) {
            val description = stringResource(R.string.visualization_remove_context)
            Box(
                modifier = Modifier
                    .size(18.dp)
                    .clip(RoundedCornerShape(9.dp))
                    .clickable(onClick = onDismiss)
                    .semantics { contentDescription = description },
                contentAlignment = Alignment.Center,
            ) {
                LXIcon(name = LXIconName.X, size = 10.dp, color = t.text3, stroke = 2f, contentDescription = null)
            }
        }
    }
}
