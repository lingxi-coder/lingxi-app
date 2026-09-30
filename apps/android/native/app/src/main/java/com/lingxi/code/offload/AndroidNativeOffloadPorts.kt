package com.lingxi.code.offload

import android.os.Build
import android.content.Context
import com.lingxi.code.clipboard.ClipboardController
import com.lingxi.code.notify.NotificationController
import com.lingxi.code.notify.NotifyException
import com.lingxi.code.notify.NotifyFailure

interface NativeClipboardPort {
    fun getText(): String?
    fun setText(text: String)
}

fun interface NativeNotificationPort {
    fun post(title: String, body: String, tag: String?)
}

fun interface NativeDevicePort {
    fun snapshot(): Map<String, String>
}

fun interface NativeCommandPort {
    suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult

    fun close() = Unit
}

data class NativeOffloadPorts(
    val clipboard: NativeClipboardPort,
    val notifications: NativeNotificationPort,
    val device: NativeDevicePort,
    val commands: Map<String, NativeCommandPort> = emptyMap(),
) {
    fun close() {
        commands.values.distinct().forEach { port -> runCatching { port.close() } }
    }
}

object AndroidNativeOffloadPorts {
    @Volatile
    private var applicationContext: Context? = null

    fun attach(context: Context) {
        applicationContext = context.applicationContext
    }

    fun detach() {
        applicationContext = null
    }

    internal fun currentContext(): Context? = applicationContext

    fun create(context: Context): NativeOffloadPorts {
        attach(context)
        return create()
    }

    fun create(): NativeOffloadPorts = NativeOffloadPorts(
        clipboard = object : NativeClipboardPort {
            override fun getText(): String? = ClipboardController.getText()
            override fun setText(text: String) = ClipboardController.setText(text)
        },
        notifications = object : NativeNotificationPort {
            override fun post(title: String, body: String, tag: String?) {
                try {
                    NotificationController.notify(title, body, tag)
                } catch (error: NotifyException) {
                    if (error.failure is NotifyFailure.PermissionDenied) {
                        throw NativeOffloadPermissionException(
                            NativeOffloadPermissionError(
                                code = "ANDROID_PERMISSION_DENIED",
                                tool = "notification",
                                message = "Android notification permission is not granted",
                                recoverable = true,
                            ),
                        )
                    }
                    throw error
                }
            }
        },
        device = object : NativeDevicePort {
            override fun snapshot(): Map<String, String> = linkedMapOf(
                "manufacturer" to Build.MANUFACTURER,
                "model" to Build.MODEL,
                "device" to Build.DEVICE,
                "androidRelease" to Build.VERSION.RELEASE,
                "sdk" to Build.VERSION.SDK_INT.toString(),
                "supportedAbis" to Build.SUPPORTED_ABIS.joinToString(","),
            )
        },
        commands = applicationContext
            ?.let(AndroidSystemOffloadPorts::create)
            .orEmpty(),
    )
}
