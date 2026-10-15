package com.lingxi.code.apps

import android.annotation.SuppressLint
import android.content.Context
import android.content.res.Configuration
import android.view.ContextThemeWrapper
import android.webkit.GeolocationPermissions
import android.webkit.PermissionRequest
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.webkit.ProfileStore
import androidx.webkit.WebStorageCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import com.lingxi.code.bindings.runtime.LocalAppHost
import com.lingxi.code.market.MarketException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import java.io.ByteArrayInputStream
import kotlin.coroutines.resume

/**
 * Runs chat apps the way [com.lingxi.code.market.MarketWebRuntime] runs market
 * apps: each app at its own origin (`https://<id>.lingxi-app.invalid`) in its
 * own WebView profile, so its storage survives new revisions and goes with the
 * app. Every request for that origin is answered by the engine's
 * [LocalAppHost.serve], which owns the document and its policies.
 */
class ChatAppRuntime(private val context: Context, private val host: () -> LocalAppHost?) {
    private val handles = mutableMapOf<String, WebView>()

    fun capabilityError(): String? = when {
        !WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE) -> "MULTI_PROFILE_UNSUPPORTED"
        !WebViewFeature.isFeatureSupported(WebViewFeature.DELETE_BROWSING_DATA) -> "DELETE_BROWSING_DATA_UNSUPPORTED"
        else -> null
    }

    private fun profile(id: String) = "app_$id"

    @SuppressLint("SetJavaScriptEnabled")
    suspend fun open(id: String, onExternalNavigation: (String) -> Unit = {}): WebView =
        withContext(Dispatchers.Main.immediate) {
            capabilityError()?.let { throw MarketException(it) }
            handles[id]?.let { return@withContext it }
            val domain = "$id.lingxi-app.invalid"
            // WebView derives prefers-color-scheme from its Context theme's
            // isLightTheme attribute, not Compose or the system setting alone.
            val night = context.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK ==
                Configuration.UI_MODE_NIGHT_YES
            val webContext = ContextThemeWrapper(
                context,
                if (night) android.R.style.Theme_Material_NoActionBar
                else android.R.style.Theme_Material_Light_NoActionBar,
            )
            WebView(webContext).apply {
                WebViewCompat.setProfile(this, profile(id))
                settings.javaScriptEnabled = true
                settings.domStorageEnabled = true
                settings.allowFileAccess = false
                settings.allowContentAccess = false
                settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
                settings.javaScriptCanOpenWindowsAutomatically = false
                settings.setSupportMultipleWindows(false)
                webChromeClient = object : WebChromeClient() {
                    override fun onPermissionRequest(request: PermissionRequest) {
                        request.deny()
                    }

                    override fun onGeolocationPermissionsShowPrompt(origin: String, callback: GeolocationPermissions.Callback) {
                        callback.invoke(origin, false, false)
                    }
                }
                webViewClient = object : WebViewClient() {
                    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                        if (request.url.scheme == "https" && request.url.host == domain) return false
                        if (request.isForMainFrame) onExternalNavigation(request.url.toString())
                        return true
                    }

                    // Runs on a WebView worker thread, which may block on the engine.
                    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                        if (request.url.scheme != "https") return failure(403)
                        if (request.url.host != domain) return null
                        if (request.method != "GET" && request.method != "HEAD") return failure(405)
                        val engine = host() ?: return failure(503)
                        val response = runCatching { engine.serve(id, request.url.path ?: "/") }.getOrNull()
                            ?: return failure(500)
                        val headers = response.headers
                            .filterNot { it.name.equals("Content-Type", ignoreCase = true) }
                            .associate { it.name to it.value }
                        val body = if (request.method == "HEAD") ByteArray(0) else response.body
                        return WebResourceResponse(
                            response.mimeType,
                            "UTF-8",
                            response.status.toInt(),
                            if (response.status.toInt() == 200) "OK" else "Not Found",
                            headers,
                            ByteArrayInputStream(body),
                        )
                    }

                    override fun onReceivedSslError(
                        view: WebView,
                        handler: android.webkit.SslErrorHandler,
                        error: android.net.http.SslError,
                    ) {
                        handler.cancel()
                    }
                }
                handles[id] = this
                loadUrl("https://$domain/")
            }
        }

    suspend fun close(id: String) = withContext(Dispatchers.Main.immediate) {
        handles.remove(id)?.let { web ->
            (web.parent as? android.view.ViewGroup)?.removeView(web)
            web.stopLoading()
            web.loadUrl("about:blank")
            web.removeAllViews()
            web.destroy()
        }
        Unit
    }

    suspend fun suspend(id: String) = withContext(Dispatchers.Main.immediate) { handles[id]?.onPause(); Unit }

    suspend fun resume(id: String) = withContext(Dispatchers.Main.immediate) { handles[id]?.onResume(); Unit }

    /** Removes the app's WebView profile, or at least its browsing data. Close the app first. */
    suspend fun clearData(id: String) = withContext(Dispatchers.Main.immediate) {
        if (capabilityError() != null) return@withContext
        val store = ProfileStore.getInstance()
        val name = profile(id)
        if (!store.getAllProfileNames().contains(name)) return@withContext
        if (runCatching { store.deleteProfile(name) }.getOrDefault(false)) return@withContext
        val profile = store.getProfile(name) ?: return@withContext
        suspendCancellableCoroutine<Unit> { continuation ->
            WebStorageCompat.deleteBrowsingData(profile.webStorage) { if (continuation.isActive) continuation.resume(Unit) }
        }
    }

    private companion object {
        fun failure(code: Int) = WebResourceResponse(
            "text/plain",
            "UTF-8",
            code,
            "Rejected",
            mapOf("Content-Security-Policy" to "default-src 'none'"),
            ByteArrayInputStream(ByteArray(0)),
        )
    }
}
