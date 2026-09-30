package com.lingxi.code.share

import com.lingxi.code.bindings.android.AndroidShare
import com.lingxi.code.bindings.android.ShareFfiException
import com.lingxi.code.bindings.android.ShareResultFfi

/**
 * Adapts the native [ShareController] to the generated UniFFI callback interface
 * [AndroidShare].
 *
 * The Rust seam (`apps/android-aar`) hands this foreign object to
 * `build_android_engine`, which bridges it onto `traits::SharingService` (via
 * `AndroidShareBridge`). We only map call shapes + result/error types here; the
 * real `Intent.ACTION_SEND` chooser lives in [ShareController].
 *
 * Mirrors [com.lingxi.code.vision.AndroidCameraAdapter]: a thin wrapper that
 * translates the local controller's success/failure surface onto the flat FFI
 * types Rust fans back out onto the richer `traits::ShareResult` /
 * `traits::ShareError`.
 */
class AndroidShareAdapter(
    private val controller: ShareController = ShareController,
) : AndroidShare {

    override suspend fun share(
        text: String?,
        url: String?,
        imageBytes: ByteArray?,
    ): ShareResultFfi {
        val outcome = try {
            controller.share(text = text, url = url, imageBytes = imageBytes)
        } catch (e: ShareException) {
            throw e.failure.toFfi()
        } catch (e: ShareFfiException) {
            throw e
        } catch (t: Throwable) {
            throw ShareFfiException.Other(t.message ?: "share error")
        }
        return when (outcome) {
            ShareOutcome.Success -> ShareResultFfi.SUCCESS
            ShareOutcome.Cancelled -> ShareResultFfi.CANCELLED
        }
    }
}

/** Map the local [ShareFailure] surface onto the flat FFI error enum. */
private fun ShareFailure.toFfi(): ShareFfiException = when (this) {
    is ShareFailure.Unsupported -> ShareFfiException.Unsupported()
    is ShareFailure.Other -> ShareFfiException.Other(message)
}
