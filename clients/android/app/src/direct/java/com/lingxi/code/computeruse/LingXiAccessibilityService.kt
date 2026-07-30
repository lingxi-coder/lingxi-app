package com.lingxi.code.computeruse

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.AccessibilityServiceInfo
import android.accessibilityservice.GestureDescription
import android.graphics.Bitmap
import android.graphics.Path
import android.hardware.HardwareBuffer
import android.os.Build
import android.view.Display
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import androidx.annotation.RequiresApi
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeout
import java.io.ByteArrayOutputStream
import java.util.concurrent.Executor

class LingXiAccessibilityService : AccessibilityService() {
    private val gestureGate = ComputerUseGestureGate()

    override fun onCreate() {
        super.onCreate()
        // Some vendor builds can recreate and bind the service without
        // delivering onServiceConnected() again to the new app lifecycle.
        // Registering the live instance here keeps UI and start checks aligned
        // with the service the system is actually running.
        ComputerUseFeatureProvider.onAccessibilityConnected(this)
    }

    override fun onServiceConnected() {
        serviceInfo = serviceInfo.apply {
            flags = flags or
                AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS or
                AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS or
                AccessibilityServiceInfo.FLAG_INCLUDE_NOT_IMPORTANT_VIEWS
        }
        ComputerUseFeatureProvider.onAccessibilityConnected(this)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event == null) return
        ComputerUseFeatureProvider.onAccessibilityEvent(
            service = this,
            packageName = event.packageName?.toString(),
            eventType = event.eventType,
        )
    }

    override fun onInterrupt() {
        // onInterrupt() asks an AccessibilityService to stop active feedback;
        // it does not mean the service was disabled or disconnected.
        cancelPendingGestures()
    }

    override fun onDestroy() {
        ComputerUseFeatureProvider.onAccessibilityDisconnected(
            this,
            "无障碍服务已断开",
        )
        super.onDestroy()
    }

    internal fun activeRoot(): AccessibilityNodeInfo? = rootInActiveWindow

    internal suspend fun gesture(
        strokes: List<GestureDescription.StrokeDescription>,
        timeoutMs: Long = 5_000,
    ): Boolean {
        val gestureToken = gestureGate.begin() ?: return false
        val deferred = CompletableDeferred<Boolean>()
        return try {
            val description = GestureDescription.Builder().also { builder ->
                strokes.forEach(builder::addStroke)
            }.build()
            val accepted = dispatchGesture(
                description,
                object : GestureResultCallback() {
                    override fun onCompleted(gestureDescription: GestureDescription?) {
                        deferred.complete(true)
                    }

                    override fun onCancelled(gestureDescription: GestureDescription?) {
                        deferred.complete(false)
                    }
                },
                null,
            )
            if (!accepted) return false
            withTimeout(timeoutMs) { deferred.await() }
        } finally {
            gestureGate.finish(gestureToken)
        }
    }

    internal suspend fun tap(x: Float, y: Float, durationMs: Long): Boolean {
        val path = Path().apply { moveTo(x, y) }
        return gesture(
            listOf(
                GestureDescription.StrokeDescription(
                    path,
                    0,
                    durationMs.coerceIn(1, 5_000),
                ),
            ),
        )
    }

    internal suspend fun swipe(
        startX: Float,
        startY: Float,
        endX: Float,
        endY: Float,
        durationMs: Long,
    ): Boolean {
        val path = Path().apply {
            moveTo(startX, startY)
            lineTo(endX, endY)
        }
        return gesture(
            listOf(
                GestureDescription.StrokeDescription(
                    path,
                    0,
                    durationMs.coerceIn(50, 5_000),
                ),
            ),
        )
    }

    internal suspend fun pinch(
        centerX: Float,
        centerY: Float,
        scale: Float,
        durationMs: Long,
    ): Boolean {
        val radius = 120f
        val endRadius = radius * scale.coerceIn(0.1f, 10f)
        val left = Path().apply {
            moveTo(centerX - radius, centerY)
            lineTo(centerX - endRadius, centerY)
        }
        val right = Path().apply {
            moveTo(centerX + radius, centerY)
            lineTo(centerX + endRadius, centerY)
        }
        val duration = durationMs.coerceIn(100, 5_000)
        return gesture(
            listOf(
                GestureDescription.StrokeDescription(left, 0, duration),
                GestureDescription.StrokeDescription(right, 0, duration),
            ),
        )
    }

    internal fun cancelPendingGestures() {
        // Android exposes no inert "cancel gesture" API. Dispatching another
        // gesture cancels the old one but also injects a real pointer event, so
        // a stop path must never use a fake tap as cancellation. Invalidate the
        // tracked action instead; the cancelled agent coroutine and service
        // teardown prevent any subsequent action from being accepted.
        gestureGate.claimCancellation()
    }

    @RequiresApi(Build.VERSION_CODES.R)
    internal suspend fun captureWithAccessibility(): CapturedScreen {
        val deferred = CompletableDeferred<CapturedScreen>()
        takeScreenshot(
            Display.DEFAULT_DISPLAY,
            Executor { command -> command.run() },
            object : TakeScreenshotCallback {
                override fun onSuccess(screenshot: ScreenshotResult) {
                    runCatching {
                        val hardwareBuffer: HardwareBuffer = screenshot.hardwareBuffer
                        try {
                            val wrapped = Bitmap.wrapHardwareBuffer(
                                hardwareBuffer,
                                screenshot.colorSpace,
                            ) ?: error("Unable to wrap accessibility screenshot")
                            val bitmap = try {
                                wrapped.copy(Bitmap.Config.ARGB_8888, false)
                            } finally {
                                wrapped.recycle()
                            }
                            try {
                                val bytes = ByteArrayOutputStream().use { output ->
                                    bitmap.compress(Bitmap.CompressFormat.PNG, 100, output)
                                    output.toByteArray()
                                }
                                CapturedScreen(bitmap.width, bitmap.height, bytes)
                            } finally {
                                bitmap.recycle()
                            }
                        } finally {
                            hardwareBuffer.close()
                        }
                    }.onSuccess(deferred::complete)
                        .onFailure(deferred::completeExceptionally)
                }

                override fun onFailure(errorCode: Int) {
                    deferred.completeExceptionally(
                        IllegalStateException("Accessibility screenshot failed: $errorCode"),
                    )
                }
            },
        )
        return withTimeout(5_000) { deferred.await() }
    }
}
