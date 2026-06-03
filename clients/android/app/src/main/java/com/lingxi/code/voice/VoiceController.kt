package com.lingxi.code.voice

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.util.Log
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.core.content.ContextCompat
import com.lingxi.code.bindings.AndroidEventListener
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.bindings.SpeechFfiException
import com.lingxi.code.bindings.buildAndroidEngine
import com.lingxi.code.voice.audio.AndroidSttAdapter
import com.lingxi.code.voice.audio.AndroidTtsAdapter
import com.lingxi.code.voice.audio.SystemSpeechRecognizerStt
import com.lingxi.code.voice.audio.SystemTextToSpeechTts
import com.lingxi.code.vision.AndroidCameraAdapter
import kotlinx.coroutines.launch

private const val TAG = "VoiceController"

/**
 * T3.3 — wire the device-audio path into the engine + the Compose hold-to-talk
 * surface.
 *
 * Two concerns live here:
 *  1. [buildVoiceEngine] constructs the real [MobileEngineHandle] through the
 *     generated UniFFI `buildAndroidEngine(...)`, handing it the [AndroidStt] /
 *     [AndroidTts] adapters (T3.2) over the device's system recognizer /
 *     synthesizer plus a foreground event listener and config. This is the
 *     end-to-end FFI seam: the engine's `tool-speech` now routes through the
 *     device's native STT/TTS. On a non-Android host (and if the cdylib fails to
 *     load) it returns `null` rather than crashing the shell.
 *  2. [rememberVoiceCapture] turns the hold-to-talk release into a real
 *     transcription. Releasing the held mic checks the `RECORD_AUDIO` runtime
 *     permission (requesting it on first use) and, when granted, drives the same
 *     [SystemSpeechRecognizerStt] adapter to capture one utterance; the
 *     recognized text is routed back into the composer draft via [onTranscript].
 */

/**
 * Build the engine over the device's native speech stack. The STT/TTS adapters
 * are the exact objects the engine bridges onto `traits::SpeechToText` /
 * `traits::TextToSpeech`, so the speech tool runs on-device.
 *
 * Returns `null` when the engine cannot be built — on a JVM/unit host the
 * `buildAndroidEngine` export returns `PlatformUnavailable`, and a missing
 * cdylib throws on class init; either way the chat shell stays usable.
 */
fun buildVoiceEngine(
    context: Context,
    apiBase: String = "",
    apiKey: String = "",
    model: String = "",
): MobileEngineHandle? {
    val appContext = context.applicationContext
    val stt = AndroidSttAdapter(SystemSpeechRecognizerStt(appContext))
    val tts = AndroidTtsAdapter(SystemTextToSpeechTts(appContext))
    // Device-vision: the camera adapter drives the process-global CameraController,
    // whose ActivityResult launchers are registered by MainActivity. The engine
    // bridges this onto `traits::CameraControl`, lighting up `tool-camera` on-device.
    val camera = AndroidCameraAdapter()
    val listener = object : AndroidEventListener {
        override suspend fun onEvent(event: ClientEvent) {
            // Minimal sink: the chat surface still streams through the mock
            // ConversationSource for now (T3.3 wires the audio path, not the full
            // event loop). Logging proves the listener bridge is live without
            // blocking the engine turn loop.
            Log.d(TAG, "engine event: ${event::class.simpleName}")
        }
    }
    return try {
        buildAndroidEngine(
            apiBase = apiBase,
            apiKey = apiKey,
            model = model,
            appFilesRoot = appContext.filesDir.absolutePath,
            listener = listener,
            stt = stt,
            tts = tts,
            camera = camera,
        )
    } catch (t: Throwable) {
        // PlatformUnavailable on a host build, or UnsatisfiedLinkError when the
        // native lib for this ABI is absent — degrade to the UI shell.
        Log.w(TAG, "buildAndroidEngine unavailable: ${t.message}")
        null
    }
}

/** Result of one hold-to-talk capture. */
sealed interface VoiceCaptureResult {
    data class Transcript(val text: String) : VoiceCaptureResult
    data object PermissionDenied : VoiceCaptureResult
    data class Failed(val message: String) : VoiceCaptureResult
    data object Empty : VoiceCaptureResult
}

/** Drives a single hold-to-talk transcription, gated on RECORD_AUDIO. */
class VoiceCapture internal constructor(
    private val context: Context,
    private val requestPermission: () -> Unit,
    private val hasPermission: () -> Boolean,
) {
    private val stt = SystemSpeechRecognizerStt(context.applicationContext)

    /** True once RECORD_AUDIO has been granted. */
    fun isPermitted(): Boolean = hasPermission()

    /** Ask for RECORD_AUDIO (no-op if already granted). */
    fun ensurePermission() {
        if (!hasPermission()) requestPermission()
    }

    /**
     * Capture one utterance from the live mic and return the transcript. Uses the
     * same `SystemSpeechRecognizerStt` the engine bridges onto its STT seam.
     */
    suspend fun transcribe(language: String? = null): VoiceCaptureResult {
        if (!hasPermission()) return VoiceCaptureResult.PermissionDenied
        val adapter = AndroidSttAdapter(stt)
        return try {
            val text = adapter.transcribe(language).trim()
            if (text.isEmpty()) VoiceCaptureResult.Empty
            else VoiceCaptureResult.Transcript(text)
        } catch (e: SpeechFfiException.PermissionDenied) {
            VoiceCaptureResult.PermissionDenied
        } catch (e: SpeechFfiException.NoSpeech) {
            VoiceCaptureResult.Empty
        } catch (e: SpeechFfiException) {
            VoiceCaptureResult.Failed(e.message ?: "speech error")
        } catch (t: Throwable) {
            VoiceCaptureResult.Failed(t.message ?: "speech error")
        }
    }
}

/**
 * Compose entry point: returns the hold-to-talk handlers wired to a live
 * [VoiceCapture]. [onTranscript] receives the recognized text on a successful
 * release; the mic is gated on the RECORD_AUDIO runtime permission, which is
 * requested the first time the user holds the mic without it.
 *
 * @return Pair of (onHoldStart, onHoldRelease) for [VoiceFlowOverlay]'s gesture.
 */
@Composable
fun rememberVoiceCapture(
    onTranscript: (String) -> Unit,
): Pair<() -> Unit, () -> Unit> {
    val context = androidx.compose.ui.platform.LocalContext.current
    val scope = rememberCoroutineScope()

    val permLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { /* result observed on the next hold via hasPermission() */ }

    val capture = remember(context) {
        VoiceCapture(
            context = context,
            requestPermission = { permLauncher.launch(Manifest.permission.RECORD_AUDIO) },
            hasPermission = {
                ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.RECORD_AUDIO,
                ) == PackageManager.PERMISSION_GRANTED
            },
        )
    }

    val onHoldStart: () -> Unit = { capture.ensurePermission() }
    val onHoldRelease: () -> Unit = {
        scope.launch {
            when (val r = capture.transcribe()) {
                is VoiceCaptureResult.Transcript -> onTranscript(r.text)
                is VoiceCaptureResult.PermissionDenied -> capture.ensurePermission()
                is VoiceCaptureResult.Empty -> Unit
                is VoiceCaptureResult.Failed -> Log.w(TAG, "transcription failed: ${r.message}")
            }
        }
    }

    return onHoldStart to onHoldRelease
}
