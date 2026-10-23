package com.lingxi.code.remote

import android.graphics.Bitmap
import android.net.Uri
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.util.Base64
import java.util.Random
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Bounds-only BitmapFactory decoding returns null even for a valid image. */
@RunWith(AndroidJUnit4::class)
class RemoteImageShrinkInstrumentedTest {
    @Test fun aValidPickedImageSurvivesBoundsDecodeAndIsCompressed() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val file = File.createTempFile("remote-image-", ".png", context.cacheDir)
        val bitmap = Bitmap.createBitmap(2000, 1000, Bitmap.Config.ARGB_8888)
        try {
            val random = Random(73)
            val pixels = IntArray(bitmap.width * bitmap.height) { random.nextInt() or (0xff shl 24) }
            bitmap.setPixels(pixels, 0, bitmap.width, 0, 0, bitmap.width, bitmap.height)
            file.outputStream().use { assertTrue(bitmap.compress(Bitmap.CompressFormat.PNG, 100, it)) }
            assertTrue("fixture must exceed the remote upload limit", file.length() > 2 * 1024 * 1024)
            val attachment = shrinkPickedImage(context, Uri.fromFile(file))
            assertEquals(1568, attachment.width)
            assertEquals(784, attachment.height)
            assertEquals("image/jpeg", attachment.mediaType)
            assertTrue(Base64.getDecoder().decode(attachment.base64).size <= 2 * 1024 * 1024)
        } finally {
            bitmap.recycle()
            file.delete()
        }
    }
}
