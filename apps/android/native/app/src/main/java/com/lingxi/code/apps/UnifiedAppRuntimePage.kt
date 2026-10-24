package com.lingxi.code.apps

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.util.Base64
import android.webkit.WebView
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.lingxi.code.components.UiTags
import com.lingxi.code.components.blockTouchesBehind
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.util.UUID

@Composable
fun UnifiedAppRuntimePage(apps: UnifiedLocalApps, appId: String, onClose: () -> Unit, onShowDetails: (String) -> Unit) {
    val context = LocalContext.current
    val lifecycle = LocalLifecycleOwner.current
    val activity = remember(context) {
        var owner: android.content.Context? = context
        while (owner is android.content.ContextWrapper && owner !is android.app.Activity) owner = owner.baseContext
        owner as? android.app.Activity
    }
    val scope = rememberCoroutineScope()
    var view by remember(appId) { mutableStateOf<WebView?>(null) }
    var error by remember(appId) { mutableStateOf<String?>(null) }
    var release by remember(appId) { mutableStateOf<AppRelease?>(null) }
    var needsConsent by remember(appId) { mutableStateOf(false) }
    var paused by remember(appId) { mutableStateOf(false) }
    var starting by remember(appId) { mutableStateOf(false) }
    var request by remember(appId) { mutableStateOf(apps.runtime.pendingDeviceRequest(appId)) }
    var cameraUri by rememberSaveable(appId) { mutableStateOf<String?>(null) }
    var cameraFile by rememberSaveable(appId) { mutableStateOf<String?>(null) }
    var external by remember { mutableStateOf<String?>(null) }
    var menuOpen by remember { mutableStateOf(false) }
    suspend fun open() {
        if (starting) return
        starting = true; error = null
        try {
            val record = apps.get(appId)
            val current = apps.release(appId, record.currentRevision); release = current
            if (current.executesCode && current.digest !in record.executionGrants) { needsConsent = true; return }
            val operation = if (paused) "resume" else "start"
            paused = false
            val launch = apps.start(appId, operation)
            view = apps.runtime.open(appId, launch)
            if (lifecycle.lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) apps.runtime.resume(appId)
            else apps.runtime.pause(appId)
            apps.markOpened(appId); paused = false
        } catch (e: Exception) { error = e.message }
        finally { starting = false }
    }
    fun closePage() {
        scope.launch {
            apps.runtime.launch(appId)?.let { launch -> runCatching { apps.agentClosed(launch) } }
            onClose()
        }
    }
    fun finishPicker(uri: Uri?) {
        val pending = request ?: return
        val temporaryPhoto = cameraFile.takeIf { pending.kind == "camera.capture" }
        request = null
        scope.launch {
            try {
                val value = uri?.let { selected -> withContext(Dispatchers.IO) {
                    val mime = context.contentResolver.getType(selected) ?: "application/octet-stream"
                    val photo = pending.kind == "photos.pick" || pending.kind == "camera.capture"
                    val maximumBytes = (if (photo) 16 else 8) * 1024 * 1024
                    val bytes = context.contentResolver.openInputStream(selected)?.use { input ->
                        val output = java.io.ByteArrayOutputStream(); val buffer = ByteArray(32768)
                        while (true) { val count = input.read(buffer); if (count < 0) break
                            require(output.size() + count <= maximumBytes) { "所选文件过大，请选择较小文件。" }
                            output.write(buffer, 0, count)
                        }; output.toByteArray()
                    } ?: kotlin.error("无法读取所选文件")
                    val normalized = if (photo) LocalAppPhotoEncoding.jpeg(bytes) else bytes
                    JSONObject().put("mimeType", if (photo) "image/jpeg" else mime)
                        .put("base64", Base64.encodeToString(normalized, Base64.NO_WRAP)).toString()
                } }
                pending.reply.complete(value)
            } catch (e: Exception) { pending.reply.completeExceptionally(e); error = e.message }
            finally {
                temporaryPhoto?.let { path -> withContext(Dispatchers.IO) { File(path).delete() }; cameraFile = null; cameraUri = null }
            }
        }
    }
    val documentPicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { finishPicker(it) }
    val sharePicker = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        request?.reply?.complete(null); request = null
    }
    val photoCapture = rememberLauncherForActivityResult(ActivityResultContracts.TakePicture()) { accepted -> finishPicker(if (accepted) cameraUri?.let(Uri::parse) else null) }
    fun capture() {
        val pending = request ?: return
        scope.launch {
            try {
                apps.runtime.authorizeDeviceRequest(pending)
                val file = File(context.cacheDir, "shared_images/local-app-${UUID.randomUUID()}.jpg")
                file.parentFile!!.mkdirs()
                cameraFile = file.absolutePath
                cameraUri = FileProvider.getUriForFile(context, context.packageName + ".fileprovider", file).toString()
                photoCapture.launch(Uri.parse(cameraUri))
            } catch (failure: Exception) {
                pending.reply.completeExceptionally(failure); request = null; error = failure.message
            }
        }
    }
    val cameraPermission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { allowed ->
        if (allowed) capture() else { request?.reply?.completeExceptionally(IllegalStateException("相机权限未获允许")); request = null }
    }
    LaunchedEffect(appId) { open() }
    LaunchedEffect(appId) {
        apps.runtime.deviceRequests.collect { next ->
            if (next.appId != appId) { next.reply.completeExceptionally(IllegalStateException("应用窗口不可用")); return@collect }
            request = next
            if (next.kind == "share") {
                sharePicker.launch(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, next.text), null))
            } else if (next.kind == "camera.capture") {
                if (ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) capture()
                else cameraPermission.launch(Manifest.permission.CAMERA)
            } else if (next.kind != "microphone.record") documentPicker.launch(arrayOf(if (next.kind == "photos.pick") "image/*" else "*/*"))
        }
    }
    LaunchedEffect(paused, starting) {
        if (paused && !starting && lifecycle.lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) open()
    }
    DisposableEffect(appId, lifecycle) {
        apps.runtime.externalNavigation = { external = it }
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_RESUME -> apps.runtime.resume(appId)
                Lifecycle.Event.ON_PAUSE -> apps.runtime.pause(appId)
                Lifecycle.Event.ON_STOP -> {
                    apps.runtime.pause(appId)
                    if (activity?.isChangingConfigurations != true && !apps.runtime.deviceRequestActive && !apps.keepRunning(appId, release?.digest)) scope.launch {
                        try { apps.suspend(appId); view = null; paused = true } catch (e: Exception) { error = e.message }
                    }
                }
                Lifecycle.Event.ON_START -> if (paused) scope.launch { open() }
                else -> Unit
            }
        }
        lifecycle.lifecycle.addObserver(observer)
        onDispose {
            lifecycle.lifecycle.removeObserver(observer)
            apps.runtime.externalNavigation = null
            if (activity?.isChangingConfigurations != true) {
                request?.reply?.completeExceptionally(IllegalStateException("应用窗口已关闭"))
                val closingView = view
                if (closingView != null) apps.scope.launch {
                    try { apps.runtime.close(appId, closingView) }
                    catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
                    catch (failure: Exception) { apps.recordRuntimeFailure(failure) }
                }
            }
        }
    }
    BackHandler { if (view?.canGoBack() == true) view?.goBack() else closePage() }
    Box(Modifier.fillMaxSize().background(LingXiTheme.palette.windowBg).blockTouchesBehind().windowInsetsPadding(WindowInsets.systemBars).testTag(UiTags.APP_RUNTIME_PAGE)) {
        view?.let { web -> AndroidView(factory = { web }, modifier = Modifier.fillMaxSize(), onRelease = { (it.parent as? android.view.ViewGroup)?.removeView(it) }) }
        if (view == null && !needsConsent) Column(Modifier.align(Alignment.Center).padding(24.dp)) {
            if (starting) CircularProgressIndicator()
            error?.let { Text(it, color = MaterialTheme.colorScheme.error); TextButton(onClick = { scope.launch { open() } }) { Text(stringResource(R.string.local_app_v2_retry)) } }
            if (paused) Text(stringResource(R.string.local_app_v2_paused))
        }
        Surface(Modifier.align(Alignment.TopEnd).padding(8.dp), shape = MaterialTheme.shapes.large, tonalElevation = 4.dp) {
            Row {
                Box {
                    TextButton(onClick = { menuOpen = true }) { Text(stringResource(R.string.local_app_v2_manage)) }
                    DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                        DropdownMenuItem(text = { Text(stringResource(R.string.local_app_v2_manage)) }, onClick = { menuOpen = false; onShowDetails(appId) })
                        DropdownMenuItem(text = { Text(stringResource(R.string.local_app_v2_refresh)) }, onClick = { menuOpen = false; view?.reload() })
                        DropdownMenuItem(text = { Text(stringResource(R.string.local_app_v2_stop)) }, onClick = {
                            menuOpen = false
                            scope.launch { try { apps.stop(appId); view = null; onClose() } catch (failure: Exception) { error = failure.message } }
                        })
                    }
                }
                TextButton(onClick = ::closePage) { Text(stringResource(R.string.local_app_v2_close)) }
            }
        }
    }
    val selected = release
    request?.takeIf { it.kind == "microphone.record" }?.let { pending ->
        LocalAppRecordingDialog(apps, pending) { if (request === pending) request = null }
    }
    if (release?.capabilities?.contains("agent") == true) LocalAppAgentInteractions(apps, appId)
    if (needsConsent && selected != null) AlertDialog(onDismissRequest = ::closePage, title = { Text(stringResource(R.string.local_app_v2_allow_title)) }, text = {
        Text(stringResource(R.string.local_app_v2_execution_note, selected.revision, selected.digest.take(16)))
    }, confirmButton = { TextButton(onClick = { scope.launch { try { apps.grant(appId, selected.digest); needsConsent = false; open() } catch (e: Exception) { error = e.message } } }) { Text(stringResource(R.string.local_app_v2_allow_release)) } }, dismissButton = { TextButton(onClick = ::closePage) { Text(stringResource(R.string.local_app_v2_cancel)) } })
    external?.let { url -> AlertDialog(onDismissRequest = { external = null }, title = { Text(stringResource(R.string.local_app_v2_external_title)) }, text = { Text(url) }, confirmButton = { TextButton(onClick = { external = null; runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url))) } }) { Text(stringResource(R.string.local_app_v2_open)) } }, dismissButton = { TextButton(onClick = { external = null }) { Text(stringResource(R.string.local_app_v2_cancel)) } }) }
}
