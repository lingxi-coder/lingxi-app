package com.lingxi.code.apps

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.ByteArrayOutputStream

@RunWith(AndroidJUnit4::class)
class LocalAppPhotoEncodingTest {
    @Test fun transparentLargePNGBecomesBoundedWhiteJPEG() {
        val image = Bitmap.createBitmap(4096, 32, Bitmap.Config.ARGB_8888)
        val bytes = try {
            image.eraseColor(Color.TRANSPARENT)
            ByteArrayOutputStream().use { output -> image.compress(Bitmap.CompressFormat.PNG, 100, output); output.toByteArray() }
        } finally { image.recycle() }
        val jpeg = LocalAppPhotoEncoding.jpeg(bytes)
        assertEquals(0xff, jpeg[0].toInt() and 0xff)
        assertEquals(0xd8, jpeg[1].toInt() and 0xff)
        val decoded = BitmapFactory.decodeByteArray(jpeg, 0, jpeg.size)
        try {
            assertEquals(2048, decoded.width)
            assertEquals(16, decoded.height)
            assertTrue(Color.red(decoded.getPixel(0, 0)) >= 250)
        } finally { decoded.recycle() }
    }
    @Test fun malformedAndOversizedPhotoIsRejected() {
        assertTrue(runCatching { LocalAppPhotoEncoding.jpeg(byteArrayOf(0, 1, 2)) }.isFailure)
        assertTrue(runCatching { LocalAppPhotoEncoding.jpeg(ByteArray(16 * 1024 * 1024 + 1)) }.isFailure)
    }
}
