package com.lingxi.code.conversation

import android.annotation.SuppressLint
import android.graphics.Color
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.ByteArrayInputStream

private const val MERMAID_ORIGIN = "https://mermaid.lingxi.invalid"
private object MermaidAsset {
    @Volatile var html: String? = null
}

/** A bundled document with a one-way size channel and no host capabilities. */
@SuppressLint("SetJavaScriptEnabled")
@Composable
internal fun MermaidDiagram(source: String) {
    val context = LocalContext.current.applicationContext
    val dark = LingXiTheme.palette.isDark
    val supported = remember { WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER) }
    val document by produceState<String?>(null, context) {
        value = withContext(Dispatchers.IO) {
            MermaidAsset.html ?: runCatching {
                context.assets.open("mermaid.html").bufferedReader().use { it.readText() }
            }.getOrNull()?.also { MermaidAsset.html = it }
        }
    }
    var height by remember { mutableStateOf(300) }
    var failed by remember(source) { mutableStateOf(false) }
    val html = document
    if (!supported || html == null || failed) {
        Text(source, color = LingXiTheme.palette.text2)
        return
    }
    val payload = JSONObject().put("source", source).put("dark", dark).toString()
    AndroidView(
        modifier = Modifier.fillMaxWidth().height(height.dp),
        factory = { viewContext ->
            MermaidWebView(viewContext).apply {
                setBackgroundColor(Color.TRANSPARENT)
                settings.javaScriptEnabled = true
                settings.domStorageEnabled = false
                settings.allowFileAccess = false
                settings.allowContentAccess = false
                settings.blockNetworkLoads = true
                settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
                settings.useWideViewPort = true
                settings.setSupportZoom(true)
                settings.builtInZoomControls = true
                settings.displayZoomControls = false
                settings.javaScriptCanOpenWindowsAutomatically = false
                WebViewCompat.addWebMessageListener(this, "mermaidHost", setOf(MERMAID_ORIGIN)) { _, message, origin, mainFrame, _ ->
                    if (mainFrame && origin.toString().trimEnd('/') == MERMAID_ORIGIN) {
                        val data = runCatching { JSONObject(message.data.orEmpty()) }.getOrNull()
                        if (data?.optString("channel") == "lingxi-mermaid" && data.optString("type") == "size") {
                            val next = data.optDouble("height", 300.0)
                            if (next.isFinite()) post { height = next.toInt().coerceIn(180, 640) }
                        }
                    }
                }
                webViewClient = object : WebViewClient() {
                    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest) = true
                    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest) =
                        WebResourceResponse("text/plain", "utf-8", ByteArrayInputStream(ByteArray(0)))
                    override fun onPageFinished(view: WebView, url: String) {
                        ready = true
                        deliver()
                    }
                    override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
                        failed = true
                        return true
                    }
                }
                loadDataWithBaseURL("$MERMAID_ORIGIN/", html, "text/html", "utf-8", null)
            }
        },
        update = { it.payload = payload; it.deliver() },
        onRelease = {
            it.stopLoading()
            WebViewCompat.removeWebMessageListener(it, "mermaidHost")
            it.destroy()
        },
    )
}

@SuppressLint("ViewConstructor")
private class MermaidWebView(context: android.content.Context) : WebView(context) {
    var payload: String? = null
    var ready = false
    private var delivered: String? = null

    fun deliver() {
        val next = payload ?: return
        if (!ready || delivered == next) return
        delivered = next
        evaluateJavascript("window.renderMermaid($next)", null)
    }
}
