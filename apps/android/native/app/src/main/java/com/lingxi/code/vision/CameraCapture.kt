package com.lingxi.code.vision

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
import kotlinx.coroutines.launch

private const val TAG = "CameraCapture"

/**
 * L3-Wire — the device-vision analog of `voice.VoiceCapture`.
 *
 * Where hold-to-talk surfaces a transcript ([com.lingxi.code.voice.rememberVoiceCapture] →
 * `onTranscript`), the composer's camera affordance surfaces a [CapturedImage] →
 * `onCaptured`. Tapping the camera icon checks the runtime CAMERA permission
 * (requesting it on first use) and, when granted, drives the same
 * [CameraController] the engine bridges onto `traits::CameraControl` — so the
 * UI affordance and `tool-camera` share one capture path.
 *
 * The capture runs through [CameraController.capturePhoto] (the real system
 * camera UI, not a stub), and the resulting JPEG is handed back to the caller to
 * render as a composer thumbnail / attachment chip.
 */

/** Result of one composer camera tap. */
sealed interface CameraCaptureResult {
    data class Captured(val image: CapturedImage) : CameraCaptureResult
    data object PermissionDenied : CameraCaptureResult
    data object Cancelled : CameraCaptureResult
    data class Failed(val message: String) : CameraCaptureResult
}

/** Drives a single composer photo capture, gated on CAMERA. */
class CameraCapture internal constructor(
    private val requestPermission: () -> Unit,
    private val hasPermission: () -> Boolean,
    private val controller: CameraController = CameraController,
) {
    fun isPermitted(): Boolean = hasPermission()

    /** Ask for CAMERA (no-op if already granted). */
    fun ensurePermission() {
        if (!hasPermission()) requestPermission()
    }

    /**
     * Capture one photo via the system camera, returning the encoded JPEG. Uses
     * the same [CameraController] the engine bridges onto its camera seam, so a
     * composer capture and a `tool-camera` capture are the identical code path.
     */
    suspend fun capture(): CameraCaptureResult {
        return try {
            val image = controller.capturePhoto(front = false, allowEditing = false)
            CameraCaptureResult.Captured(image)
        } catch (e: CameraException) {
            when (e.failure) {
                is CameraFailure.PermissionDenied -> CameraCaptureResult.PermissionDenied
                is CameraFailure.Cancelled -> CameraCaptureResult.Cancelled
                is CameraFailure.DeviceUnavailable ->
                    CameraCaptureResult.Failed("camera unavailable")
                is CameraFailure.Other ->
                    CameraCaptureResult.Failed((e.failure as CameraFailure.Other).message)
            }
        } catch (t: Throwable) {
            CameraCaptureResult.Failed(t.message ?: "camera error")
        }
    }
}

/**
 * Compose entry point: returns an `onCameraClick` handler wired to a live
 * [CameraCapture]. [onCaptured] receives the photo on a successful capture; the
 * camera is gated on the CAMERA runtime permission, requested the first time the
 * user taps without it. Mirrors [com.lingxi.code.voice.rememberVoiceCapture].
 */
@Composable
fun rememberCameraCapture(
    onCaptured: (CapturedImage) -> Unit,
): () -> Unit {
    val context = androidx.compose.ui.platform.LocalContext.current
    val scope = rememberCoroutineScope()

    val permLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { /* result observed on the next tap via hasPermission() */ }

    val capture = remember(context) {
        CameraCapture(
            requestPermission = { permLauncher.launch(Manifest.permission.CAMERA) },
            hasPermission = {
                ContextCompat.checkSelfPermission(
                    context as Context,
                    Manifest.permission.CAMERA,
                ) == PackageManager.PERMISSION_GRANTED
            },
        )
    }

    return {
        if (!capture.isPermitted()) {
            capture.ensurePermission()
        } else {
            scope.launch {
                when (val r = capture.capture()) {
                    is CameraCaptureResult.Captured -> onCaptured(r.image)
                    is CameraCaptureResult.PermissionDenied -> capture.ensurePermission()
                    is CameraCaptureResult.Cancelled -> Unit
                    is CameraCaptureResult.Failed -> Log.w(TAG, "capture failed: ${r.message}")
                }
            }
        }
    }
}
