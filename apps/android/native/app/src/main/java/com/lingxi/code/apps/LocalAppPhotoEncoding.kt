package com.lingxi.code.apps

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.ImageDecoder
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import kotlin.math.roundToInt

/** Photo codecs and EXIF orientation stay native; the app receives a bounded JPEG. */
internal object LocalAppPhotoEncoding {
    fun jpeg(bytes: ByteArray): ByteArray {
        require(bytes.size <= 16 * 1024 * 1024) { "请选择小于 16 MB 的照片。" }
        val decoded = ImageDecoder.decodeBitmap(ImageDecoder.createSource(ByteBuffer.wrap(bytes))) { decoder, info, _ ->
            val width = info.size.width
            val height = info.size.height
            require(width > 0 && height > 0 && width <= 32768 && height <= 32768 && width.toLong() * height <= 64_000_000) {
                "请选择不超过 6400 万像素的照片。"
            }
            val scale = minOf(1.0, 2048.0 / maxOf(width, height))
            decoder.setTargetSize((width * scale).roundToInt().coerceAtLeast(1), (height * scale).roundToInt().coerceAtLeast(1))
            decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
        }
        try {
            val flattened = Bitmap.createBitmap(decoded.width, decoded.height, Bitmap.Config.ARGB_8888)
            try {
                Canvas(flattened).apply { drawColor(Color.WHITE); drawBitmap(decoded, 0f, 0f, null) }
                return ByteArrayOutputStream().use { output ->
                    check(flattened.compress(Bitmap.CompressFormat.JPEG, 85, output)) { "照片无法转换，请重新选择。" }
                    output.toByteArray()
                }
            } finally { flattened.recycle() }
        } finally { decoded.recycle() }
    }
}
