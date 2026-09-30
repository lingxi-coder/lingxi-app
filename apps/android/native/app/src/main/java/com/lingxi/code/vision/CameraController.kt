package com.lingxi.code.vision

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Handler
import android.os.Looper
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import java.io.ByteArrayOutputStream
import kotlinx.coroutines.CancellableContinuation
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/**
 * Result of one native capture/pick, already encoded to JPEG + measured.
 *
 * Mirrors the `CapturedImageFfi` carrier the [com.lingxi.code.bindings.android.AndroidCamera]
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
 * Process-global bridge between the (Rust-driven) [com.lingxi.code.bindings.android.AndroidCamera]
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
internal class CameraResultHost(
    val owner: Any,
    val hasPermission: () -> Boolean,
    val requestPermission: () -> Unit,
    val takePicture: () -> Unit,
    val pickMedia: () -> Unit,
)

internal class CameraResultCoordinator(private val post: (() -> Unit) -> Unit) {

    internal enum class ResultKind { Permission, Picture, Media }
    private class Pending(
        val host: CameraResultHost,
        val continuation: CancellableContinuation<CapturedImage>,
        var kind: ResultKind,
        var launched: Boolean = false,
    )

    private val lock = Any()
    private var launchers: CameraResultHost? = null
    // A cancelled launched operation remains here until its OS result drains.
    private var pending: Pending? = null

    fun attach(next: CameraResultHost) {
        val retired = synchronized(lock) {
            if (launchers?.owner === next.owner) return@synchronized null
            val old = pending.takeIf { it?.host?.owner !== next.owner }
            if (old != null) pending = null
            launchers = next
            old
        }
        retired?.let { fail(it, CameraFailure.Cancelled) }
    }

    fun detach(owner: Any) {
        val retired = synchronized(lock) {
            if (launchers?.owner === owner) launchers = null
            pending?.takeIf { it.host.owner === owner }?.also { pending = null }
        }
        retired?.let { fail(it, CameraFailure.Cancelled) }
    }

    suspend fun capturePhoto(): CapturedImage {
        val host = synchronized(lock) { launchers } ?: throw CameraException(CameraFailure.DeviceUnavailable)
        val kind = if (host.hasPermission()) ResultKind.Picture else ResultKind.Permission
        return awaitResult(host, kind)
    }

    suspend fun pickFromLibrary(): CapturedImage {
        val host = synchronized(lock) { launchers } ?: throw CameraException(CameraFailure.DeviceUnavailable)
        return awaitResult(host, ResultKind.Media)
    }

    private suspend fun awaitResult(host: CameraResultHost, kind: ResultKind): CapturedImage = suspendCancellableCoroutine { continuation ->
        val request = Pending(host, continuation, kind)
        val accepted = synchronized(lock) {
            if (launchers !== host || pending != null) false else {
                pending = request
                true
            }
        }
        if (!accepted) {
            continuation.resumeWithException(CameraException(CameraFailure.Other("another camera request is already in flight")))
            return@suspendCancellableCoroutine
        }
        continuation.invokeOnCancellation {
            synchronized(lock) {
                // Before launch there can be no result. After launch keep the
                // dead owner's slot; Android cannot cancel the external UI.
                if (pending === request && !request.launched) pending = null
            }
        }
        launch(request)
    }

    private fun launch(request: Pending) {
        post {
            val admitted = synchronized(lock) {
                if (pending !== request || launchers !== request.host) false
                else if (!request.continuation.isActive) {
                    pending = null
                    false
                } else {
                    request.launched = true
                    true
                }
            }
            if (!admitted) return@post
            runCatching {
                when (request.kind) {
                    ResultKind.Permission -> request.host.requestPermission()
                    ResultKind.Picture -> request.host.takePicture()
                    ResultKind.Media -> request.host.pickMedia()
                }
            }.onFailure { fail(request, CameraFailure.DeviceUnavailable) }
        }
    }

    fun onCameraPermission(owner: Any, granted: Boolean) {
        val request = synchronized(lock) {
            pending?.takeIf { it.host.owner === owner && it.kind == ResultKind.Permission && it.launched }?.also {
                // This OS result is consumed before possibly launching a photo.
                it.launched = false
                if (!it.continuation.isActive || !granted) pending = null
                else it.kind = ResultKind.Picture
            }
        } ?: return
        if (!request.continuation.isActive) return
        if (!granted) fail(request, CameraFailure.PermissionDenied) else launch(request)
    }

    fun onResult(owner: Any, kind: ResultKind, image: () -> CapturedImage) {
        val request = takeResult(owner, kind) ?: return
        if (!request.continuation.isActive) return
        runCatching(image).onSuccess { succeed(request, it) }.onFailure {
            fail(request, if (it is CameraException) it.failure else CameraFailure.Other(it.message ?: "image read failed"))
        }
    }

    private fun takeResult(owner: Any, kind: ResultKind): Pending? = synchronized(lock) {
        pending?.takeIf { it.host.owner === owner && it.kind == kind && it.launched }?.also { pending = null }
    }

    private fun succeed(request: Pending, image: CapturedImage) {
        if (request.continuation.isActive) request.continuation.resume(image)
    }

    private fun fail(request: Pending, failure: CameraFailure) {
        synchronized(lock) { if (pending === request) pending = null }
        if (request.continuation.isActive) request.continuation.resumeWithException(CameraException(failure))
    }

}

object CameraController {
    private class Launchers(
        val owner: Any, val context: Context,
        val requestPermission: ActivityResultLauncher<String>,
        val takePicture: ActivityResultLauncher<Void?>,
        val pickMedia: ActivityResultLauncher<PickVisualMediaRequest>,
    )
    private val mainHandler = Handler(Looper.getMainLooper())
    private val coordinator = CameraResultCoordinator { operation -> mainHandler.post { operation() } }

    fun attach(value: Any) {
        val host = value as Launchers
        activeContexts.clear()
        activeContexts[host.owner] = host.context
        coordinator.attach(CameraResultHost(host.owner, { hasCameraPermission(host.context) },
            { host.requestPermission.launch(android.Manifest.permission.CAMERA) },
            { host.takePicture.launch(null) },
            { host.pickMedia.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) }))
    }

    fun detach(owner: Any) {
        coordinator.detach(owner)
        activeContexts.remove(owner)
    }

    fun makeLaunchers(owner: Any, context: Context, requestCameraPermission: ActivityResultLauncher<String>,
        takePicture: ActivityResultLauncher<Void?>, pickMedia: ActivityResultLauncher<PickVisualMediaRequest>): Any =
        Launchers(owner, context, requestCameraPermission, takePicture, pickMedia)

    suspend fun capturePhoto(front: Boolean, allowEditing: Boolean): CapturedImage = coordinator.capturePhoto()
    suspend fun pickFromLibrary(): CapturedImage = coordinator.pickFromLibrary()
    fun onCameraPermission(owner: Any, granted: Boolean) = coordinator.onCameraPermission(owner, granted)
    fun onPictureTaken(owner: Any, bitmap: Bitmap?) = coordinator.onResult(owner, CameraResultCoordinator.ResultKind.Picture) {
        if (bitmap == null) throw CameraException(CameraFailure.Cancelled)
        encode(bitmap)
    }
    fun onMediaPicked(owner: Any, uri: Uri?) = coordinator.onResult(owner, CameraResultCoordinator.ResultKind.Media) {
        if (uri == null) throw CameraException(CameraFailure.Cancelled)
        // The launch owner controls the decoder context, including after a rebind.
        val host = activeContexts[owner] ?: throw CameraException(CameraFailure.DeviceUnavailable)
        decodeUri(host, uri)
    }

    private val activeContexts = java.util.IdentityHashMap<Any, Context>()
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
