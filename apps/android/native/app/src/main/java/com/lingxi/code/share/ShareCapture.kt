package com.lingxi.code.share

import android.util.Log
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import kotlinx.coroutines.launch

private const val TAG = "ShareCapture"

/**
 * L3-Wire — the device-share analog of `voice.VoiceCapture` / `vision.CameraCapture`.
 *
 * Where hold-to-talk surfaces a transcript and the composer's camera affordance
 * surfaces a [com.lingxi.code.vision.CapturedImage], a message bubble's share
 * affordance hands text/url to the native share sheet. Tapping it drives the same
 * process-global [ShareController] the engine bridges onto `traits::SharingService`
 * (via `AndroidShareBridge`) — so the UI affordance and `tool-share` share one
 * launch path. Unlike the camera there is no permission gate and no result to
 * await: the chooser is fire-and-forget.
 */

/** Result of one share affordance tap. */
sealed interface ShareCaptureResult {
    data object Launched : ShareCaptureResult
    data object NoChooser : ShareCaptureResult
    data class Failed(val message: String) : ShareCaptureResult
}

/** Drives a single share-sheet launch through the shared [ShareController]. */
class ShareCapture internal constructor(
    private val controller: ShareController = ShareController,
) {
    /**
     * Launch the native chooser for [text] (and optional [url]). Uses the same
     * [ShareController] the engine bridges onto its share seam, so a bubble share
     * and a `tool-share` invocation are the identical code path.
     */
    fun share(text: String?, url: String? = null): ShareCaptureResult {
        return try {
            when (controller.share(text = text, url = url, imageBytes = null)) {
                ShareOutcome.Success -> ShareCaptureResult.Launched
                ShareOutcome.Cancelled -> ShareCaptureResult.NoChooser
            }
        } catch (e: ShareException) {
            ShareCaptureResult.Failed(
                when (val f = e.failure) {
                    is ShareFailure.Other -> f.message
                    ShareFailure.Unsupported -> "nothing to share"
                },
            )
        } catch (t: Throwable) {
            ShareCaptureResult.Failed(t.message ?: "share error")
        }
    }
}

/**
 * Compose entry point: returns an `onShare(text)` handler wired to a live
 * [ShareCapture]. Mirrors [com.lingxi.code.vision.rememberCameraCapture] — the
 * caller passes the bubble's text and the native chooser is surfaced.
 */
@Composable
fun rememberShare(): (String) -> Unit {
    val scope = rememberCoroutineScope()
    val capture = remember { ShareCapture() }
    return { text ->
        scope.launch {
            when (val r = capture.share(text = text)) {
                ShareCaptureResult.Launched -> Unit
                ShareCaptureResult.NoChooser -> Log.w(TAG, "no context attached for share chooser")
                is ShareCaptureResult.Failed -> Log.w(TAG, "share failed: ${r.message}")
            }
        }
    }
}
