package com.lingxi.code.apps

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.os.Build
import android.content.pm.PackageManager
import androidx.core.content.ContextCompat
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import android.os.VibrationEffect
import android.os.Vibrator
import android.webkit.PermissionRequest
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.webkit.ProfileStore
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import androidx.webkit.WebStorageCompat
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.json.JSONObject

/** One profile per installation, one current run per view; never receives a chat engine. */
class UnifiedAppWebRuntime(private val context: Context, private val apps: UnifiedLocalApps) {
    private data class ViewRun(val view: WebView, val launch: AppLaunch, val capabilities: Set<String>, var foreground: Boolean = false)
    private val views = mutableMapOf<String, ViewRun>()
    data class DeviceRequest(val appId: String, val runId: String, val kind: String, val text: String?, val reply: CompletableDeferred<String?>)
    private val requests = MutableSharedFlow<DeviceRequest>(extraBufferCapacity = 1)
    val deviceRequests = requests.asSharedFlow()
    var deviceRequestActive = false
        private set
    private var activeDeviceRequest: DeviceRequest? = null
    fun pendingDeviceRequest(appId: String): DeviceRequest? = activeDeviceRequest?.takeIf { it.appId == appId }
    var externalNavigation: ((String) -> Unit)? = null

    fun launch(id: String): AppLaunch? = views[id]?.launch

    @SuppressLint("SetJavaScriptEnabled")
    suspend fun open(id: String, launch: AppLaunch): WebView = withContext(Dispatchers.Main.immediate) {
        require(launch.run.appId == id)
        require(WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE)) { "MULTI_PROFILE_UNSUPPORTED" }
        require(WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) { "WEB_MESSAGE_LISTENER_UNSUPPORTED" }
        require(WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) { "DOCUMENT_START_SCRIPT_UNSUPPORTED" }
        views[id]?.takeIf { it.launch.run.runId == launch.run.runId }?.let { return@withContext it.view }
        val declared = apps.release(id, launch.run.revision).capabilities.toSet()
        close(id)
        val view = WebView(context)
        WebViewCompat.setProfile(view, profile(id))
        view.settings.javaScriptEnabled = true
        view.settings.domStorageEnabled = true
        view.settings.allowFileAccess = false
        view.settings.allowContentAccess = false
        view.settings.javaScriptCanOpenWindowsAutomatically = false
        view.settings.setSupportMultipleWindows(false)
        view.webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                if (isCurrentAppFrame(launch, launch.origin, request.url.toString(), true)) return false
                // The gateway performs a Referer-verified 307 into this epoch for
                // same-origin root links, preserving POST bodies and query strings.
                if (request.isForMainFrame && isCurrentAppFrame(launch, launch.origin, view.url, true) &&
                    request.url.scheme == "http" && request.url.host == "127.0.0.1" &&
                    request.url.port == Uri.parse(launch.origin).port && request.url.userInfo == null &&
                    !request.url.path.orEmpty().startsWith("/__run/")) return false
                if (request.isForMainFrame && request.url.scheme in setOf("https", "http", "mailto", "tel")) externalNavigation?.invoke(request.url.toString())
                return true
            }
            override fun onReceivedSslError(view: WebView, handler: android.webkit.SslErrorHandler, error: android.net.http.SslError) = handler.cancel()
        }
        view.webChromeClient = object : WebChromeClient() {
            override fun onPermissionRequest(request: PermissionRequest) {
                val capabilities = request.resources.map { resource -> when (resource) {
                    PermissionRequest.RESOURCE_VIDEO_CAPTURE -> "camera"
                    PermissionRequest.RESOURCE_AUDIO_CAPTURE -> "microphone"
                    else -> "unsupported"
                } }
                if (!current(id, launch) || !isCurrentAppFrame(launch, request.origin.toString(), view.url, true) ||
                    capabilities.any { !apps.permissions.allowed(id, launch.run.digest, it) }) { request.deny(); return }
                // WebChromeClient does not expose the requesting frame. A media bridge
                // request is therefore denied here; only explicit native picker flows run.
                request.deny()
            }
            override fun onShowFileChooser(view: WebView, callback: ValueCallback<Array<Uri>?>, params: FileChooserParams): Boolean {
                // Only the main-frame message bridge can identify its caller.
                // File/camera dialogs use files.pick/camera.capture below.
                callback.onReceiveValue(null)
                return true
            }
            override fun onGeolocationPermissionsShowPrompt(origin: String, callback: android.webkit.GeolocationPermissions.Callback) = callback.invoke(origin, false, false)
        }
        WebViewCompat.addWebMessageListener(view, "lingxiNative", setOf(launch.origin)) { sourceView, message, sourceOrigin, mainFrame, reply ->
            val raw = message.data ?: return@addWebMessageListener
            if (raw.length > 64 * 1024) return@addWebMessageListener
            val input = runCatching { JSONObject(raw) }.getOrNull() ?: return@addWebMessageListener
            val requestId = input.optionalString("id") ?: return@addWebMessageListener
            apps.scope.launch {
                val result = JSONObject().put("id", requestId)
                try {
                    check(current(id, launch) && sourceView === view && isCurrentAppFrame(launch, sourceOrigin.toString(), view.url, mainFrame)) { "Application caller is no longer active" }
                    val live = runIsCurrent(id, launch)
                    check(live && current(id, launch)) { "Application run has stopped" }
                    val method = input.getString("method")
                    val args = input.optJSONObject("args") ?: JSONObject()
                    val value = native(id, launch, method, args)
                    check(current(id, launch) && runIsCurrent(id, launch)) { "Application run changed" }
                    result.put("ok", true).put("value", value ?: JSONObject.NULL)
                } catch (error: Exception) {
                    result.put("ok", false).put("error", error.message ?: "Native operation failed")
                }
                if (current(id, launch)) reply.postMessage(result.toString())
            }
        }
        WebViewCompat.addDocumentStartJavaScript(view, appSdkScript(launch), setOf(launch.origin))
        views[id] = ViewRun(view, launch, declared)
        view.loadUrl(launch.url)
        view
    }
    private suspend fun native(id: String, launch: AppLaunch, method: String, args: JSONObject): Any? {
        fun need(capability: String) {
            val page = views[id]
            check(devicePageActive(id)) { "Open this application page before using a device capability" }
            check(page != null && capability in page.capabilities && apps.permissions.allowed(id, launch.run.digest, capability)) { "Native capability required: $capability" }
        }
        when (method) {
            "info" -> {
                val app = apps.get(id)
                val layout = apps.request("layout", id) as JSONObject
                return JSONObject().put("id", id).put("name", app.name).put("platform", "android").put("runId", launch.run.runId)
                    .put("revision", launch.run.revision).put("dataDirectory", withContext(Dispatchers.IO) { apps.guestPath(layout.getString("dataPath")) })
            }
            "serviceSocketUrl" -> return appServiceUrl(launch, args.getString("name"), args.optString("path", "/"), true)
            "camera.capture", "photos.pick", "files.pick", "microphone.record", "share" -> {
                val capability = deviceCapability(method)
                need(capability)
                check(!deviceRequestActive) { "A device picker is already open" }
                val pending = CompletableDeferred<String?>()
                deviceRequestActive = true
                try {
                    val request = DeviceRequest(id, launch.run.runId, method, if (method == "share") args.getString("text") else null, pending)
                    activeDeviceRequest = request
                    requests.emit(request)
                    val value = pending.await()
                    awaitDevicePage(id, launch)
                    check(current(id, launch)) { "Application changed while selecting a file" }
                    need(capability)
                    return value?.let(::JSONObject)
                } finally { activeDeviceRequest = null; deviceRequestActive = false }
            }
            "clipboard.read" -> { need("clipboard"); return (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).primaryClip?.getItemAt(0)?.coerceToText(context)?.toString() }
            "clipboard.write" -> { need("clipboard"); (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText("Local app", args.getString("text"))); return null }

            "notify" -> {
                need("notifications")
                check(Build.VERSION.SDK_INT < 33 || ContextCompat.checkSelfPermission(context, android.Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) { "System notification permission is not granted" }
                val manager = context.getSystemService(NotificationManager::class.java)
                check(manager.areNotificationsEnabled()) { "System notifications are disabled" }
                val channel = "local-app-v2-events"
                manager.createNotificationChannel(NotificationChannel(channel, "Local app notifications", NotificationManager.IMPORTANCE_DEFAULT))
                val notification = Notification.Builder(context, channel).setSmallIcon(android.R.drawable.ic_dialog_info)
                    .setContentTitle(args.optString("title", "Local app").take(100)).setContentText(args.getString("body").take(1000)).setAutoCancel(true).build()
                manager.notify(id, 1, notification)
                return null
            }
            "haptics" -> { need("haptics"); (context.getSystemService(Context.VIBRATOR_SERVICE) as Vibrator).vibrate(VibrationEffect.createOneShot(30, VibrationEffect.DEFAULT_AMPLITUDE)); return null }
            else -> error("Unsupported native method: $method")
        }
    }
    private fun current(id: String, launch: AppLaunch) = views[id]?.launch?.run?.runId == launch.run.runId
    private fun devicePageActive(id: String): Boolean = views[id]?.let { page ->
        page.foreground && page.view.isAttachedToWindow && page.view.windowVisibility == android.view.View.VISIBLE
    } == true
    private suspend fun awaitDevicePage(id: String, launch: AppLaunch) = withTimeout(3_000) {
        // ActivityResult may arrive before onResume and before a rotated
        // WebView is attached. Grant no capability until all gates agree.
        while (!devicePageActive(id)) {
            check(current(id, launch)) { "Application window closed" }
            kotlinx.coroutines.delay(50)
        }
    }
    internal suspend fun authorizeDeviceRequest(request: DeviceRequest) {
        val active = views[request.appId] ?: error("Application window closed")
        awaitDevicePage(request.appId, active.launch)
        val capability = deviceCapability(request.kind)
        check(active.launch.run.runId == request.runId && runIsCurrent(request.appId, active.launch) &&
            current(request.appId, active.launch) && capability in active.capabilities &&
            devicePageActive(request.appId) &&
            apps.permissions.allowed(request.appId, active.launch.run.digest, capability)) { "Application device permission, page or run changed" }
    }
    private suspend fun runIsCurrent(id: String, launch: AppLaunch): Boolean {
        val run = (apps.request("status", id) as JSONObject).optJSONObject("run")
        return run?.optString("runId") == launch.run.runId && run.optString("phase") == "running"
    }
    suspend fun close(id: String) = withContext(Dispatchers.Main.immediate) {
        activeDeviceRequest?.takeIf { it.appId == id }?.reply?.completeExceptionally(IllegalStateException("Application run closed"))
        views.remove(id)?.view?.let { view ->
            (view.parent as? android.view.ViewGroup)?.removeView(view)
            WebViewCompat.removeWebMessageListener(view, "lingxiNative")
            view.stopLoading(); view.loadUrl("about:blank"); view.destroy()
        }
        Unit
    }
    suspend fun clearData(id: String) = withContext(Dispatchers.Main.immediate) {
        check(WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE)) { "MULTI_PROFILE_UNSUPPORTED" }
        check(WebViewFeature.isFeatureSupported(WebViewFeature.DELETE_BROWSING_DATA)) { "Update Android System WebView to clear this application's browser data" }
        close(id)
        val installed = ProfileStore.getInstance().getProfile(profile(id)) ?: return@withContext
        // deleteProfile rejects profiles loaded anywhere in this process, even
        // after every associated WebView has been destroyed. Clear this exact
        // profile's browsing data and await the provider's completion instead.
        val cleared = CompletableDeferred<Unit>()
        WebStorageCompat.deleteBrowsingData(installed.webStorage) { cleared.complete(Unit) }
        withTimeout(30_000) { cleared.await() }
    }
    fun pause(id: String) { views[id]?.let { it.foreground = false; it.view.onPause() } }
    fun resume(id: String) { views[id]?.let { it.foreground = true; it.view.onResume() } }
    companion object {
        private fun profile(id: String) = "local_app_v2_$id"
        private fun deviceCapability(method: String) = when (method) {
            "camera.capture" -> "camera"
            "photos.pick" -> "photos"
            "files.pick" -> "files"
            "microphone.record" -> "microphone"
            "share" -> "share"
            else -> error("Unsupported device request")
        }
    }
}
