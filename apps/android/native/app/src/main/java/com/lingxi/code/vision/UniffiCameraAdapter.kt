package com.lingxi.code.vision

import com.lingxi.code.bindings.android.AndroidCamera
import com.lingxi.code.bindings.android.CameraFfiException
import com.lingxi.code.bindings.android.CapturedImageFfi

/**
 * Adapts the native [CameraController] to the generated UniFFI callback
 * interface [AndroidCamera].
 *
 * The Rust seam (`apps/android-aar`) hands this foreign object to
 * `build_android_engine`, which bridges it onto `traits::CameraControl`
 * (via `AndroidCameraBridge`). We only map call shapes + result/error types
 * here; the real `ActivityResult`-driven capture lives in [CameraController].
 *
 * Mirrors `voice.audio.AndroidSttAdapter`: a thin wrapper that translates the
 * local provider's success/failure surface onto the flat FFI types Rust fans
 * back out onto the richer `traits::CameraError`.
 */
class AndroidCameraAdapter(
    private val controller: CameraController = CameraController,
) : AndroidCamera {

    override suspend fun capturePhoto(front: Boolean, allowEditing: Boolean): CapturedImageFfi =
        mapResult { controller.capturePhoto(front = front, allowEditing = allowEditing) }

    override suspend fun pickFromLibrary(): CapturedImageFfi =
        mapResult { controller.pickFromLibrary() }

    /** Run a controller call, mapping success → [CapturedImageFfi] and failure → [CameraFfiException]. */
    private suspend fun mapResult(block: suspend () -> CapturedImage): CapturedImageFfi {
        val image = try {
            block()
        } catch (e: CameraException) {
            throw e.failure.toFfi()
        } catch (e: CameraFfiException) {
            throw e
        } catch (t: Throwable) {
            throw CameraFfiException.Other(t.message ?: "camera error")
        }
        return CapturedImageFfi(
            jpegBytes = image.jpegBytes,
            width = image.width.toUInt(),
            height = image.height.toUInt(),
        )
    }
}

/** Map the local [CameraFailure] surface onto the flat FFI error enum. */
private fun CameraFailure.toFfi(): CameraFfiException = when (this) {
    is CameraFailure.PermissionDenied -> CameraFfiException.PermissionDenied()
    is CameraFailure.Cancelled -> CameraFfiException.Cancelled()
    is CameraFailure.DeviceUnavailable -> CameraFfiException.DeviceUnavailable()
    is CameraFailure.Other -> CameraFfiException.Other(message)
}
