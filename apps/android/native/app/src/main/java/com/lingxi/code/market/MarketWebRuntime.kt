package com.lingxi.code.market

import android.annotation.SuppressLint
import android.content.Context
import android.webkit.*
import androidx.webkit.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import java.io.ByteArrayInputStream
import java.io.File
import java.net.URI
import kotlin.coroutines.resume

/** Owns native handles only: UI controls where the returned WebView is attached. */
class MarketWebRuntime(private val context: Context, private val root: File): MarketRuntime {
    internal lateinit var repository: MarketRepository
    private val handles=mutableMapOf<String,WebView>()
    fun capabilityError(): String? = when {
        !WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE) -> "MULTI_PROFILE_UNSUPPORTED"
        !WebViewFeature.isFeatureSupported(WebViewFeature.DELETE_BROWSING_DATA) -> "DELETE_BROWSING_DATA_UNSUPPORTED"
        else -> null
    }
    private fun supported() { capabilityError()?.let { throw MarketException(it) } }
    private fun profile(id: String) = "market_$id"
    override suspend fun recoverProfiles(tombstones: Set<String>): Set<String> = withContext(Dispatchers.Main.immediate) {
        if(tombstones.isEmpty()) return@withContext tombstones
        supported(); val store=ProfileStore.getInstance()
        tombstones.filter { id -> store.getAllProfileNames().contains(profile(id)) && !runCatching { store.deleteProfile(profile(id)) }.getOrDefault(false) }.toSet()
    }
    override suspend fun clearData(installationId: String) = withContext(Dispatchers.Main.immediate) {
        supported(); val p=ProfileStore.getInstance().getProfile(profile(installationId)) ?: return@withContext
        suspendCancellableCoroutine<Unit> { continuation -> WebStorageCompat.deleteBrowsingData(p.webStorage) { if(continuation.isActive) continuation.resume(Unit) } }
    }
    @SuppressLint("SetJavaScriptEnabled")
    suspend fun open(appId: String, onExternalNavigation: (String)->Unit = {}): WebView {
        val item=repository.installed().find { it.appId==appId } ?: throw MarketException("NOT_INSTALLED")
        var result: WebView?=null
        repository.withAppLock(appId) {
            if(!repository.canOpen(item)) throw MarketException("UNINSTALL_PENDING")
            result=withContext(Dispatchers.Main.immediate) {
                supported(); handles[appId]?.let { return@withContext it }
                val directory=File(root,"versions/${item.directory}"); val domain="${item.installationId}.market.invalid"
                val assets=WebViewAssetLoader.Builder().setDomain(domain).setHttpAllowed(false).addPathHandler("/",WebViewAssetLoader.PathHandler { path -> serve(directory,path,null) }).build()
                WebView(context).apply {
                    WebViewCompat.setProfile(this,profile(item.installationId))
                    settings.javaScriptEnabled=true; settings.domStorageEnabled=true; settings.allowFileAccess=false; settings.allowContentAccess=false
                    settings.mixedContentMode=WebSettings.MIXED_CONTENT_NEVER_ALLOW; settings.javaScriptCanOpenWindowsAutomatically=false; settings.setSupportMultipleWindows(false)
                    webChromeClient=object: WebChromeClient() {
                        override fun onPermissionRequest(request: PermissionRequest) { request.deny() }
                        override fun onGeolocationPermissionsShowPrompt(origin: String, callback: GeolocationPermissions.Callback) { callback.invoke(origin,false,false) }
                    }
                    webViewClient=object: WebViewClient() {
                        override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                            if(request.url.scheme=="https" && request.url.host==domain) return false
                            if(request.isForMainFrame) onExternalNavigation(request.url.toString()); return true
                        }
                        override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                            if(request.url.host==domain && request.url.scheme=="https") {
                                if(request.method!="GET" && request.method!="HEAD") return failure(405)
                                val range=request.requestHeaders.entries.find { it.key.equals("Range",ignoreCase=true) }?.value
                                if(request.method=="HEAD" || range!=null) return serve(directory,request.url.path ?: "",range,request.method=="HEAD")
                                return assets.shouldInterceptRequest(request.url) ?: failure(404)
                            }
                            return if(request.url.scheme=="https") null else failure(403)
                        }
                        override fun onReceivedSslError(view: WebView, handler: android.webkit.SslErrorHandler, error: android.net.http.SslError) { handler.cancel() }
                    }
                    handles[appId]=this; loadUrl("https://$domain/index.html")
                }
            }
        }; return result!!
    }
    override suspend fun close(appId: String) = withContext(Dispatchers.Main.immediate) {
        handles.remove(appId)?.let { web -> (web.parent as? android.view.ViewGroup)?.removeView(web); web.stopLoading(); web.loadUrl("about:blank"); web.removeAllViews(); web.destroy() }; Unit
    }
    suspend fun suspend(appId: String) = withContext(Dispatchers.Main.immediate) { handles[appId]?.onPause(); Unit }
    suspend fun resume(appId: String) = withContext(Dispatchers.Main.immediate) { handles[appId]?.onResume(); Unit }
    companion object {
        const val CSP="sandbox allow-scripts allow-forms allow-modals allow-downloads allow-same-origin; default-src 'self' data: blob: https:; script-src 'self' 'unsafe-inline' 'unsafe-eval' blob: https:; style-src 'self' 'unsafe-inline' https:; connect-src 'self' https: wss:; worker-src 'none'; object-src 'none'; base-uri 'none'"
        private fun failure(code: Int)=WebResourceResponse("text/plain","UTF-8",code,if(code==404) "Not Found" else "Rejected",mapOf("Content-Security-Policy" to CSP),ByteArrayInputStream(ByteArray(0)))
        internal fun serve(root: File,path: String,range: String?,head: Boolean=false): WebResourceResponse {
            val file=File(root,path.removePrefix("/"))
            if(!file.canonicalPath.startsWith(root.canonicalPath+File.separator) || !file.isFile) return failure(404)
            val length=file.length(); var start=0L; var end=length-1
            if(range!=null) {
                val match=Regex("bytes=(\\d*)-(\\d*)").matchEntire(range) ?: return failure(416)
                val a=match.groupValues[1]; val b=match.groupValues[2]
                if(a.isEmpty()) { val suffix=b.toLongOrNull() ?: return failure(416); if(suffix<=0) return failure(416); start=(length-suffix).coerceAtLeast(0) }
                else { start=a.toLongOrNull() ?: return failure(416); if(b.isNotEmpty()) end=(b.toLongOrNull() ?: return failure(416)).coerceAtMost(length-1) }
                if(start>end || start>=length) return failure(416)
            }
            val stream=file.inputStream(); var skip=start; while(skip>0) { val n=stream.skip(skip); if(n<=0) { stream.close(); return failure(416) }; skip-=n }
            val count=(end-start+1).coerceAtLeast(0)
            val bounded=object: java.io.FilterInputStream(stream) { var left=count
                override fun read(): Int { if(left<=0) return -1; val x=super.read(); if(x>=0) left--; return x }
                override fun read(b: ByteArray,off: Int,len: Int): Int { if(left<=0) return -1; val n=`in`.read(b,off,minOf(len.toLong(),left).toInt()); if(n>0) left-=n; return n }
            }
            val mime=when(file.extension.lowercase()) { "html"->"text/html"; "js","mjs"->"text/javascript"; "css"->"text/css"; "json"->"application/json"; "svg"->"image/svg+xml"; "wasm"->"application/wasm"; else->MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase()) ?: "application/octet-stream" }
            val headers=mutableMapOf("Content-Security-Policy" to CSP,"X-Content-Type-Options" to "nosniff","Accept-Ranges" to "bytes","Content-Length" to count.toString(),"Cache-Control" to "no-cache")
            if(range!=null) headers["Content-Range"]="bytes $start-$end/$length"
            val body=if(head) { bounded.close(); ByteArrayInputStream(ByteArray(0)) } else bounded
            return WebResourceResponse(mime,"UTF-8",if(range==null) 200 else 206,if(range==null) "OK" else "Partial Content",headers,body)
        }
    }
}
