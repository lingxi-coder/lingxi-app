package com.lingxi.code.remote

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.net.Uri
import androidx.compose.ui.graphics.asImageBitmap
import com.lingxi.code.conversation.ComposerAttachment
import com.lingxi.code.conversation.REMOTE_MAX_PROMPT_IMAGE_BYTES
import java.io.ByteArrayOutputStream
import java.util.Base64
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** The longest edge of an uploaded image; larger pictures are scaled down. */
internal const val REMOTE_MAX_IMAGE_EDGE = 1568

/** Why a picked image could not be attached. */
class RemoteImageRejected(val reason: Reason) : Exception(reason.name) {
    enum class Reason { Type, Decode, Size }
}

/** Dimensions that fit [width] x [height] inside a square of [maxEdge], never enlarging. */
internal fun fitWithin(width: Int, height: Int, maxEdge: Int): Pair<Int, Int> {
    val scale = minOf(1.0, maxEdge.toDouble() / maxOf(width, height))
    return maxOf(1, Math.round(width * scale).toInt()) to maxOf(1, Math.round(height * scale).toInt())
}

/** What to try next when an encoded image is still too big: lower the quality, then shrink and start again. */
internal fun nextShrinkAttempt(quality: Int, scale: Double): Pair<Int, Double> =
    if (quality > 55) (quality - 15) to scale else 80 to scale * 0.75

/**
 * A picked picture as an attachment of at most 2 MiB, scaled to fit
 * [REMOTE_MAX_IMAGE_EDGE] and encoded as JPEG (the desktop accepts PNG, JPEG,
 * GIF and WebP; JPEG is the only one that can be tuned to a size). Runs off the
 * main thread.
 */
suspend fun shrinkPickedImage(context: Context, uri: Uri): ComposerAttachment = withContext(Dispatchers.IO) {
    val resolver = context.contentResolver
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    val boundsStream = resolver.openInputStream(uri)
        ?: throw RemoteImageRejected(RemoteImageRejected.Reason.Decode)
    boundsStream.use { BitmapFactory.decodeStream(it, null, bounds) }
    if (bounds.outWidth <= 0 || bounds.outHeight <= 0) throw RemoteImageRejected(RemoteImageRejected.Reason.Decode)
    val (width, height) = fitWithin(bounds.outWidth, bounds.outHeight, REMOTE_MAX_IMAGE_EDGE)
    // Let the decoder do the coarse part of the scaling so a 50 MP photo never sits in memory whole.
    var sample = 1
    while (bounds.outWidth / (sample * 2) >= width && bounds.outHeight / (sample * 2) >= height) sample *= 2
    val decoded = resolver.openInputStream(uri)?.use {
        BitmapFactory.decodeStream(it, null, BitmapFactory.Options().apply { inSampleSize = sample })
    } ?: throw RemoteImageRejected(RemoteImageRejected.Reason.Decode)
    try {
        var quality = 90
        var scale = 1.0
        repeat(12) {
            val w = maxOf(1, Math.round(width * scale).toInt())
            val h = maxOf(1, Math.round(height * scale).toInt())
            val jpeg = encodeJpeg(decoded, w, h, quality)
            if (jpeg.size <= REMOTE_MAX_PROMPT_IMAGE_BYTES) {
                val thumb = BitmapFactory.decodeByteArray(jpeg, 0, jpeg.size, BitmapFactory.Options().apply { inSampleSize = 4 })
                    ?: throw RemoteImageRejected(RemoteImageRejected.Reason.Decode)
                return@withContext ComposerAttachment(
                    thumb = thumb.asImageBitmap(),
                    width = w,
                    height = h,
                    mediaType = "image/jpeg",
                    base64 = Base64.getEncoder().encodeToString(jpeg),
                )
            }
            nextShrinkAttempt(quality, scale).let { quality = it.first; scale = it.second }
        }
        throw RemoteImageRejected(RemoteImageRejected.Reason.Size)
    } finally {
        decoded.recycle()
    }
}

private fun encodeJpeg(source: Bitmap, width: Int, height: Int, quality: Int): ByteArray {
    val canvasBitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
    try {
        val canvas = Canvas(canvasBitmap)
        canvas.drawColor(Color.WHITE) // JPEG has no transparency
        canvas.drawBitmap(source, null, android.graphics.Rect(0, 0, width, height), null)
        return ByteArrayOutputStream().also { canvasBitmap.compress(Bitmap.CompressFormat.JPEG, quality, it) }.toByteArray()
    } finally {
        canvasBitmap.recycle()
    }
}
