package com.lingxi.code.computeruse

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.PixelFormat
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.ImageReader
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Handler
import android.os.HandlerThread
import android.util.DisplayMetrics
import android.view.WindowManager
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeout
import java.io.ByteArrayOutputStream

internal class MediaProjectionCapture(private val context: Context) {
    private val thread = HandlerThread("lingxi-projection").apply { start() }
    private val handler = Handler(thread.looper)
    private var projection: MediaProjection? = null
    private var reader: ImageReader? = null
    private var display: VirtualDisplay? = null
    private var width = 0
    private var height = 0
    private var density = 0

    fun start(resultCode: Int, resultData: Intent) {
        check(resultCode == Activity.RESULT_OK) { "MediaProjection permission was not granted" }
        stop()
        val manager = context.getSystemService(MediaProjectionManager::class.java)
        val projection = manager.getMediaProjection(resultCode, resultData)
            ?: error("MediaProjection token is unavailable")
        projection.registerCallback(
            object : MediaProjection.Callback() {
                override fun onStop() {
                    releaseDisplay()
                    ComputerUseFeatureProvider.stop(context, "media-projection-stopped")
                }
            },
            handler,
        )
        this.projection = projection
        configureDisplay()
    }

    suspend fun capturePng(): CapturedScreen {
        val projection = projection ?: error("MediaProjection is not active")
        if (reader == null) configureDisplay()
        val imageReader = checkNotNull(reader)
        val next = CompletableDeferred<CapturedScreen>()
        imageReader.setOnImageAvailableListener({ source ->
            val image = source.acquireLatestImage() ?: return@setOnImageAvailableListener
            runCatching {
                image.use {
                    val plane = it.planes[0]
                    val pixelStride = plane.pixelStride
                    val rowStride = plane.rowStride
                    val rowPadding = rowStride - pixelStride * width
                    val padded = Bitmap.createBitmap(
                        width + rowPadding / pixelStride,
                        height,
                        Bitmap.Config.ARGB_8888,
                    )
                    padded.copyPixelsFromBuffer(plane.buffer)
                    val cropped = Bitmap.createBitmap(padded, 0, 0, width, height)
                    if (cropped !== padded) padded.recycle()
                    val bytes = ByteArrayOutputStream().use { output ->
                        cropped.compress(Bitmap.CompressFormat.PNG, 100, output)
                        output.toByteArray()
                    }
                    cropped.recycle()
                    CapturedScreen(width, height, bytes)
                }
            }.onSuccess(next::complete)
                .onFailure(next::completeExceptionally)
            imageReader.setOnImageAvailableListener(null, null)
        }, handler)
        display?.release()
        display = projection.createVirtualDisplay(
            "LingXiComputerUse",
            width,
            height,
            density,
            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
            imageReader.surface,
            null,
            handler,
        )
        return try {
            withTimeout(5_000) { next.await() }
        } finally {
            // A timed-out request must not retain a listener that can capture a
            // later frame, and every per-capture VirtualDisplay is released even
            // when image conversion fails.
            imageReader.setOnImageAvailableListener(null, null)
            display?.release()
            display = null
        }
    }

    fun stop() {
        releaseDisplay()
        val current = projection
        projection = null
        current?.stop()
    }

    fun close() {
        stop()
        thread.quitSafely()
    }

    private fun configureDisplay() {
        releaseDisplay()
        val metrics = DisplayMetrics()
        @Suppress("DEPRECATION")
        context.getSystemService(WindowManager::class.java).defaultDisplay.getRealMetrics(metrics)
        width = metrics.widthPixels
        height = metrics.heightPixels
        density = metrics.densityDpi
        reader = ImageReader.newInstance(width, height, PixelFormat.RGBA_8888, 2)
    }

    private fun releaseDisplay() {
        display?.release()
        display = null
        reader?.close()
        reader = null
    }
}

internal data class CapturedScreen(
    val width: Int,
    val height: Int,
    val pngBytes: ByteArray,
)
