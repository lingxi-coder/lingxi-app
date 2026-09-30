package com.lingxi.code.vision

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.util.Log
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import java.io.ByteArrayOutputStream
import kotlinx.coroutines.CancellableContinuation
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

private const val TAG = "CameraController"

/**
 * Result of one native capture/pick, already encoded to JPEG + measured.
 *
 * Mirrors the `CapturedImageFfi` carrier the [com.lingxi.code.bindings.AndroidCamera]
 * seam expects, but keeps the controller free of any bindings import so it can be
 * unit-tested / reused without the cdylib.
 */
data class CapturedImage(
    val jpegBytes: ByteArray,
    val width: Int,
    val height: Int,
)

/** Why a capture/pick failed; mapped onto `CameraFfiException` by the adapter. */
sealed interface CameraFailure {
    data object PermissionDenied : CameraFailure
    data object Cancelled : CameraFailure
    data object DeviceUnavailable : CameraFailure
    data class Other(val message: String) : CameraFailure
}

/** Raised by the controller; the adapter fans it onto `CameraFfiException`. */
class CameraException(val failure: CameraFailure) : Exception(
    when (failure) {
        is CameraFailure.Other -> failure.message
        else -> failure::class.simpleName ?: "camera error"
    },
)

/**
 * Process-global bridge between the (Rust-driven) [com.lingxi.code.bindings.AndroidCamera]
 * callback interface and the `ActivityResult` launchers, which can only be
 * registered against a live [androidx.activity.ComponentActivity].
 *
 * This mirrors how the device-audio mic is invoked: the engine calls a `suspend`
 * method that drives a real Android UI affordance and resumes when the user is
 * done. Because the engine has no `Activity` handle, the active Activity
 * registers its launchers here in `onCreate` ([attach]) and clears them in
 * `onDestroy` ([detach]); the suspend methods park a continuation that the
 * Activity's result callbacks resume.
 *
 * Only one capture/pick is in flight at a time (the native UI is modal), so a
 * single pending continuation is sufficient.
 */
object CameraController {

    /** Launchers wired up by the host Activity; null when no Activity is attached. */
    private class Launchers(
        val context: Context,
        val requestCameraPermission: ActivityResultLauncher<String>,
        val takePicture: ActivityResultLauncher<Void?>,
        val pickMedia: ActivityResultLauncher<PickVisualMediaRequest>,
    )

    @Volatile
    private var launchers: Launchers? = null

    /** The capture/pick awaiting a launcher result. */
    @Volatile
    private var pending: CancellableContinuation<CapturedImage>? = null

    /** True once the user holds CAMERA grant for a pending capture. */
    @Volatile
    private var awaitingCameraPermission: Boolean = false

    /**
     * Register the host Activity's launchers. The contracts are:
     *  - [ActivityResultContracts.RequestPermission] for runtime CAMERA grant,
     *  - [ActivityResultContracts.TakePicturePreview] for capture (returns a
     *    thumbnail [Bitmap]; no file provider needed),
     *  - [ActivityResultContracts.PickVisualMedia] for the library pick (returns
     *    a content [Uri]; READ_MEDIA_IMAGES is not required for the photo picker
     *    on modern Android, but we declare it for the legacy gallery fallback).
     */
    fun attach(launchers: Any) {
        // Typed via the concrete holder created in MainActivity to avoid leaking
        // androidx launcher generics across the public API.
        this.launchers = launchers as Launchers
    }

    fun detach() {
        launchers = null
        pending?.let { if (it.isActive) it.resumeWithException(CameraException(CameraFailure.Cancelled)) }
        pending = null
        awaitingCameraPermission = false
    }

    /**
     * Build the launcher holder. Called by [MainActivity] which owns the
     * `registerForActivityResult` results and routes them into [onCameraPermission],
     * [onPictureTaken] and [onMediaPicked].
     */
    fun makeLaunchers(
        context: Context,
        requestCameraPermission: ActivityResultLauncher<String>,
        takePicture: ActivityResultLauncher<Void?>,
        pickMedia: ActivityResultLauncher<PickVisualMediaRequest>,
    ): Any = Launchers(context, requestCameraPermission, takePicture, pickMedia)

    /** Capture a photo via the system camera UI, suspending for the result. */
    suspend fun capturePhoto(front: Boolean, allowEditing: Boolean): CapturedImage {
        val l = launchers ?: throw CameraException(CameraFailure.DeviceUnavailable)
        if (!hasCameraPermission(l.context)) {
            // Park, request CAMERA, and let onCameraPermission relaunch.
            return suspendCancellableCoroutine { cont ->
                bind(cont)
                awaitingCameraPermission = true
                cont.invokeOnCancellation { clearIfCurrent(cont) }
                runCatching { l.requestCameraPermission.launch(android.Manifest.permission.CAMERA) }
                    .onFailure { fail(cont, CameraFailure.DeviceUnavailable) }
            }
        }
        return suspendCancellableCoroutine { cont ->
            bind(cont)
            cont.invokeOnCancellation { clearIfCurrent(cont) }
            // TakePicturePreview ignores front/allowEditing (the system camera owns
            // its own lens + edit UI); we pass them through the seam for callers
            // that later swap in a CameraX impl honouring them.
            runCatching { l.takePicture.launch(null) }
                .onFailure { fail(cont, CameraFailure.DeviceUnavailable) }
        }
    }

    /** Pick an existing image from the system photo library. */
    suspend fun pickFromLibrary(): CapturedImage {
        val l = launchers ?: throw CameraException(CameraFailure.DeviceUnavailable)
        return suspendCancellableCoroutine { cont ->
            bind(cont)
            cont.invokeOnCancellation { clearIfCurrent(cont) }
            runCatching {
                l.pickMedia.launch(
                    PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly),
                )
            }.onFailure { fail(cont, CameraFailure.DeviceUnavailable) }
        }
    }

    // --- Activity result sinks (called from MainActivity callbacks) ---

    fun onCameraPermission(granted: Boolean) {
        val cont = pending ?: return
        if (!awaitingCameraPermission) return
        awaitingCameraPermission = false
        if (!granted) {
            fail(cont, CameraFailure.PermissionDenied)
            return
        }
        val l = launchers
        if (l == null) {
            fail(cont, CameraFailure.DeviceUnavailable)
            return
        }
        runCatching { l.takePicture.launch(null) }
            .onFailure { fail(cont, CameraFailure.DeviceUnavailable) }
    }

    fun onPictureTaken(bitmap: Bitmap?) {
        val cont = pending ?: return
        if (bitmap == null) {
            fail(cont, CameraFailure.Cancelled)
            return
        }
        val image = runCatching { encode(bitmap) }
            .getOrElse {
                fail(cont, CameraFailure.Other(it.message ?: "encode failed"))
                return
            }
        succeed(cont, image)
    }

    fun onMediaPicked(uri: Uri?) {
        val cont = pending ?: return
        if (uri == null) {
            fail(cont, CameraFailure.Cancelled)
            return
        }
        val l = launchers
        if (l == null) {
            fail(cont, CameraFailure.DeviceUnavailable)
            return
        }
        val image = runCatching { decodeUri(l.context, uri) }
            .getOrElse {
                fail(cont, CameraFailure.Other(it.message ?: "decode failed"))
                return
            }
        succeed(cont, image)
    }

    // --- internals ---

    private fun bind(cont: CancellableContinuation<CapturedImage>) {
        // Drop any stale pending op (the modal UI guarantees one at a time, but
        // be defensive if a prior launch never resolved).
        pending?.let { if (it.isActive) it.resumeWithException(CameraException(CameraFailure.Cancelled)) }
        pending = cont
    }

    private fun clearIfCurrent(cont: CancellableContinuation<CapturedImage>) {
        if (pending === cont) pending = null
    }

    private fun succeed(cont: CancellableContinuation<CapturedImage>, image: CapturedImage) {
        if (pending === cont) pending = null
        if (cont.isActive) cont.resume(image)
    }

    private fun fail(cont: CancellableContinuation<CapturedImage>, failure: CameraFailure) {
        if (pending === cont) pending = null
        if (cont.isActive) cont.resumeWithException(CameraException(failure))
        else Log.w(TAG, "camera failed after continuation closed: $failure")
    }

    private fun hasCameraPermission(context: Context): Boolean =
        androidx.core.content.ContextCompat.checkSelfPermission(
            context,
            android.Manifest.permission.CAMERA,
        ) == android.content.pm.PackageManager.PERMISSION_GRANTED

    private fun encode(bitmap: Bitmap): CapturedImage {
        val out = ByteArrayOutputStream()
        bitmap.compress(Bitmap.CompressFormat.JPEG, 90, out)
        return CapturedImage(jpegBytes = out.toByteArray(), width = bitmap.width, height = bitmap.height)
    }

    private fun decodeUri(context: Context, uri: Uri): CapturedImage {
        val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() }
            ?: throw IllegalStateException("could not open picked image")
        // Measure without fully decoding into a managed Bitmap.
        val opts = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, opts)
        val width = opts.outWidth.coerceAtLeast(0)
        val height = opts.outHeight.coerceAtLeast(0)
        // The picker can hand back HEIC/PNG/etc.; re-encode to JPEG so the FFI
        // carrier is always JPEG as documented, unless it's already JPEG.
        val isJpeg = opts.outMimeType == "image/jpeg"
        if (isJpeg) {
            return CapturedImage(jpegBytes = bytes, width = width, height = height)
        }
        val decoded = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
            ?: throw IllegalStateException("could not decode picked image")
        return encode(decoded)
    }
}
