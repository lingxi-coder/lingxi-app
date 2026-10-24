package com.lingxi.code.apps

import android.Manifest
import android.content.pm.PackageManager
import android.util.Base64
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.lingxi.code.R
import com.lingxi.code.voice.audio.AndroidAudioServiceProvider
import com.lingxi.code.voice.audio.AudioOwnerKey
import com.lingxi.code.voice.audio.DeviceAudioOperation
import com.lingxi.code.voice.audio.DeviceAudioResult
import kotlinx.coroutines.*
import org.json.JSONObject
import java.util.UUID

/** Recording uses the same device lease as chat audio, with a unique app-call owner. */
@Composable
internal fun LocalAppRecordingDialog(
    apps: UnifiedLocalApps,
    request: UnifiedAppWebRuntime.DeviceRequest,
    onFinished: () -> Unit,
) {
    val context = LocalContext.current.applicationContext
    val scope = rememberCoroutineScope()
    val lifecycle = LocalLifecycleOwner.current
    val owner = remember(request) { AudioOwnerKey.ui("local-app:${request.appId}:${request.runId}:${UUID.randomUUID()}") }
    val stop = remember(request) { CompletableDeferred<Unit>() }
    var recording by remember(request) { mutableStateOf(false) }
    var job by remember(request) { mutableStateOf<Job?>(null) }
    fun cancel() {
        job?.cancel()
        request.reply.completeExceptionally(IllegalStateException("Recording cancelled"))
        onFinished()
    }
    fun start() {
        if (job != null || request.reply.isCompleted) return
        job = scope.launch {
            var value: String? = null
            var failure: Exception? = null
            try {
                apps.runtime.authorizeDeviceRequest(request)
                val started = withContext(Dispatchers.IO) {
                    AndroidAudioServiceProvider.perform(context, owner, DeviceAudioOperation.StartRecording(44_100, "audio/m4a"))
                }
                val handle = when (started) {
                    is DeviceAudioResult.RecordingStarted -> started.handle
                    is DeviceAudioResult.Failed -> error(started.error.message)
                    else -> error("Microphone did not start")
                }
                recording = true
                // Revocation or a replaced run stops capture promptly, including
                // when no further message is sent by the page.
                withTimeoutOrNull(30_000) {
                    while (!stop.isCompleted) {
                        delay(200)
                        apps.runtime.authorizeDeviceRequest(request)
                    }
                }
                val result = withContext(Dispatchers.IO) {
                    AndroidAudioServiceProvider.perform(context, owner, DeviceAudioOperation.StopRecording(handle))
                }
                apps.runtime.authorizeDeviceRequest(request)
                val audio = when (result) {
                    is DeviceAudioResult.Recording -> result
                    is DeviceAudioResult.Failed -> error(result.error.message)
                    else -> error("Recording did not finish")
                }
                check(audio.bytes.size <= 8 * 1024 * 1024) { "Recording exceeds 8 MB" }
                value = JSONObject().put("mimeType", audio.mimeType)
                    .put("base64", Base64.encodeToString(audio.bytes, Base64.NO_WRAP)).toString()
            } catch (error: Exception) {
                failure = error
            } finally {
                try {
                    val cleaned = withContext(Dispatchers.IO + NonCancellable) {
                        AndroidAudioServiceProvider.perform(context, owner, DeviceAudioOperation.EndOwner)
                    }
                    if (cleaned is DeviceAudioResult.Failed) failure = IllegalStateException(cleaned.error.message, failure)
                } catch (error: Exception) {
                    failure = error
                }
                val error = failure
                if (error != null) request.reply.completeExceptionally(error) else request.reply.complete(value)
                recording = false
                onFinished()
            }
        }
    }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) start() else cancel()
    }
    DisposableEffect(request, lifecycle) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_STOP && job != null) cancel() }
        lifecycle.lifecycle.addObserver(observer)
        onDispose {
            lifecycle.lifecycle.removeObserver(observer)
            job?.cancel()
            request.reply.completeExceptionally(IllegalStateException("Recording window closed"))
        }
    }
    AlertDialog(onDismissRequest = ::cancel, title = { Text(stringResource(R.string.local_app_v2_record_title)) },
        text = { Text(stringResource(if (recording) R.string.local_app_v2_record_active else R.string.local_app_v2_record_note)) },
        confirmButton = {
            TextButton(onClick = {
                if (recording) stop.complete(Unit)
                else if (job == null) {
                    if (ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) start()
                    else permission.launch(Manifest.permission.RECORD_AUDIO)
                }
            }, enabled = recording || job == null) {
                Text(stringResource(if (recording) R.string.local_app_v2_record_finish else R.string.local_app_v2_record_start))
            }
        }, dismissButton = { TextButton(onClick = ::cancel) { Text(stringResource(R.string.local_app_v2_cancel)) } })
}
