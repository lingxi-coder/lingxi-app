package com.lingxi.code.share

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.util.Log
import androidx.core.content.FileProvider
import java.io.File

private const val TAG = "ShareController"

/** Outcome of one native share; mapped onto `ShareResultFfi` by the adapter. */
enum class ShareOutcome {
    /** The system chooser was launched. */
    Success,

    /** No way to present the chooser (no attached context). */
    Cancelled,
}

/** Why a share failed; mapped onto `ShareFfiException` by the adapter. */
sealed interface ShareFailure {
    data object Unsupported : ShareFailure
    data class Other(val message: String) : ShareFailure
}

/** Raised by the controller; the adapter fans it onto `ShareFfiException`. */
class ShareException(val failure: ShareFailure) : Exception(
    when (failure) {
        is ShareFailure.Other -> failure.message
        else -> failure::class.simpleName ?: "share error"
    },
)

/**
 * Process-global bridge between the (Rust-driven) `com.lingxi.code.bindings.android.AndroidShare`
 * callback interface and the Android share sheet, which is launched off a live
 * [Context].
 *
 * This mirrors [com.lingxi.code.vision.CameraController]: because the engine has
 * no [Context] handle, the host Activity attaches one in `onCreate` ([attach])
 * and clears it in `onDestroy` ([detach]); the share method builds an
 * `Intent.ACTION_SEND` chooser from the flat payload and launches it.
 *
 * Unlike the camera there is no `ActivityResult` to await — the share sheet does
 * not report the chosen target back to us — so the share method returns as soon
 * as the chooser is launched ([ShareOutcome.Success]) and only reports
 * [ShareOutcome.Cancelled] when there is no attached context to launch from.
 */
object ShareController {

    /** Context wired up by the host Activity; null when nothing is attached. */
    @Volatile
    private var context: Context? = null

    /** Register the host Activity's (application) context. */
    fun attach(context: Context) {
        this.context = context.applicationContext
    }

    fun detach() {
        context = null
    }

    /**
     * Build and launch an `Intent.ACTION_SEND` chooser for the payload. Image
     * bytes (if any) are written to a cache file exposed through a
     * [FileProvider] content [Uri]; `text`/`url` ride along as `EXTRA_TEXT`.
     *
     * Returns [ShareOutcome.Success] once the chooser is launched, or
     * [ShareOutcome.Cancelled] if no context is attached. Throws
     * [ShareException] on a malformed payload or launch failure.
     */
    fun share(text: String?, url: String?, imageBytes: ByteArray?): ShareOutcome {
        val ctx = context ?: return ShareOutcome.Cancelled

        // Combine text + url into the single EXTRA_TEXT body (both optional).
        val body = listOfNotNull(text?.takeIf { it.isNotEmpty() }, url?.takeIf { it.isNotEmpty() })
            .joinToString(separator = "\n")
        val hasImage = imageBytes != null && imageBytes.isNotEmpty()
        if (body.isEmpty() && !hasImage) {
            throw ShareException(ShareFailure.Unsupported)
        }

        val send = Intent(Intent.ACTION_SEND).apply {
            if (hasImage) {
                val uri = writeImageToCache(ctx, imageBytes!!)
                type = "image/jpeg"
                putExtra(Intent.EXTRA_STREAM, uri)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            } else {
                type = "text/plain"
            }
            if (body.isNotEmpty()) {
                putExtra(Intent.EXTRA_TEXT, body)
            }
        }

        val chooser = Intent.createChooser(send, null).apply {
            // Launching from a non-Activity (application) context requires its own task.
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            if (hasImage) addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }

        return try {
            ctx.startActivity(chooser)
            ShareOutcome.Success
        } catch (t: Throwable) {
            Log.w(TAG, "share chooser failed to launch: ${t.message}")
            throw ShareException(ShareFailure.Other(t.message ?: "share launch failed"))
        }
    }

    /**
     * Persist JPEG [bytes] to a cache file and expose it via the app's
     * [FileProvider], whose authority is `${packageName}.fileprovider` (matches
     * the manifest `<provider>` declaration, debug-suffix aware).
     */
    private fun writeImageToCache(context: Context, bytes: ByteArray): Uri {
        val dir = File(context.cacheDir, "shared_images").apply { mkdirs() }
        val file = File(dir, "share_${System.currentTimeMillis()}.jpg")
        file.outputStream().use { it.write(bytes) }
        val authority = "${context.packageName}.fileprovider"
        return FileProvider.getUriForFile(context, authority, file)
    }
}
