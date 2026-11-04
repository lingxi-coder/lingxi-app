package com.lingxi.code.conversation

import kotlin.math.max
import kotlin.math.min

/** Bound software decode to 16 MiB and 4096 pixels per edge. Export keeps original bytes. */
internal fun imagePreviewDecodeSize(width: Int, height: Int): Pair<Int, Int> {
    require(width > 0 && height > 0)
    val ratio = minOf(1.0, 4096.0 / maxOf(width, height), kotlin.math.sqrt(4_194_304.0 / (width.toDouble() * height)))
    return maxOf(1, (width * ratio).toInt()) to maxOf(1, (height * ratio).toInt())
}

internal data class ImagePreviewTransform(val scale: Float? = null, val x: Float = 0f, val y: Float = 0f)

/** All values are source/window pixels, so the percentage refers to the original image. */
internal data class ImagePreviewGeometry(val imageWidth: Float, val imageHeight: Float, val width: Float, val height: Float) {
    val ready get() = imageWidth > 0 && imageHeight > 0 && width > 0 && height > 0
    val fit get() = if (ready) min(1f, min(width / imageWidth, height / imageHeight)) else 1f
    val minimum get() = min(.1f, fit)

    fun zoom(current: ImagePreviewTransform, requested: Float, anchorX: Float = 0f, anchorY: Float = 0f, panX: Float = 0f, panY: Float = 0f): ImagePreviewTransform {
        if (!ready) return current
        val old = current.scale ?: fit
        val next = requested.coerceIn(minimum, 8f)
        val x = current.x.coerceIn(-limitX(old), limitX(old))
        val y = current.y.coerceIn(-limitY(old), limitY(old))
        return ImagePreviewTransform(next,
            (anchorX - (anchorX - x) * next / old + panX).coerceIn(-limitX(next), limitX(next)),
            (anchorY - (anchorY - y) * next / old + panY).coerceIn(-limitY(next), limitY(next)))
    }

    fun step(current: ImagePreviewTransform, increase: Boolean): ImagePreviewTransform {
        val scale = current.scale ?: fit
        val steps = listOf(.1f, .125f, .25f, .5f, .75f, 1f, 1.5f, 2f, 3f, 4f, 6f, 8f)
        val next = if (increase) steps.firstOrNull { it > scale + .001f } ?: 8f
            else steps.lastOrNull { it < scale - .001f } ?: minimum
        return zoom(current, next)
    }

    private fun limitX(scale: Float) = max(0f, (imageWidth * scale - width) / 2)
    private fun limitY(scale: Float) = max(0f, (imageHeight * scale - height) / 2)
}
