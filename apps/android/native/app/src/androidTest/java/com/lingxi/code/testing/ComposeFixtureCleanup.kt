package com.lingxi.code.testing

import android.app.Activity
import android.os.SystemClock
import android.graphics.Bitmap
import android.graphics.Rect
import android.view.WindowInsets
import android.view.PixelCopy
import android.os.Handler
import android.os.Looper
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.json.JSONObject

/**
 * Finish the fixture through its normal lifecycle before ActivityScenario.close().
 * Test Core 1.7 otherwise launches an EmptyActivity and waits for its resume broadcast
 * even on modern Android. On the Samsung test phone that helper intermittently stalls
 * after the assertions, leaving the tested Activity paused but never destroyed.
 * This keeps destruction checked, without the API 15–19 workaround in that helper.
 */
fun finishComposeFixture(activity: Activity) {
    val instrumentation = InstrumentationRegistry.getInstrumentation()
    if (InstrumentationRegistry.getArguments().getString("captureFixture") == "true") {
        val info = JSONObject()
        var focused = false
        instrumentation.runOnMainSync {
            focused = activity.window.decorView.hasWindowFocus()
            val bounds = Rect().also { activity.window.decorView.getWindowVisibleDisplayFrame(it) }
            info.put("focused", focused).put("visibleBounds", bounds.toShortString())
                .put("viewWidth", activity.window.decorView.width).put("viewHeight", activity.window.decorView.height)
                .put("density", activity.resources.displayMetrics.density)
                .put("orientation", activity.resources.configuration.orientation)
                .put("imeVisible", activity.window.decorView.rootWindowInsets?.isVisible(WindowInsets.Type.ime()))
        }
        val directory = activity.getExternalFilesDir("mobile-port-acceptance")!!
        directory.mkdirs()
        runCatching {
            if (focused) {
                val bitmap = Bitmap.createBitmap(activity.window.decorView.width, activity.window.decorView.height, Bitmap.Config.ARGB_8888)
                try {
                    val copied = CountDownLatch(1)
                    var result = PixelCopy.ERROR_UNKNOWN
                    PixelCopy.request(activity.window, bitmap, { result = it; copied.countDown() }, Handler(Looper.getMainLooper()))
                    check(copied.await(3, TimeUnit.SECONDS) && result == PixelCopy.SUCCESS) { "Window PixelCopy result=$result" }
                    File(directory, "fixture-last.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
                } finally { bitmap.recycle() }
            }
        }.onFailure { info.put("captureError", "${it.javaClass.simpleName}: ${it.message}") }
        File(directory, "fixture-last.json").writeText(info.toString(2))
    }
    instrumentation.runOnMainSync { activity.finish() }
    val deadline = SystemClock.uptimeMillis() + 10_000
    var destroyed = false
    while (!destroyed && SystemClock.uptimeMillis() < deadline) {
        instrumentation.runOnMainSync { destroyed = activity.isDestroyed }
        if (!destroyed) SystemClock.sleep(16)
    }
    check(destroyed) { "Compose fixture did not reach DESTROYED after finish()" }
}
