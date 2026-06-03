package com.lingxi.code.notify

import com.lingxi.code.bindings.AndroidNotification
import com.lingxi.code.bindings.NotificationFfiException

/**
 * Adapts the native [NotificationController] to the generated UniFFI callback
 * interface [AndroidNotification].
 *
 * The Rust seam (`apps/android-aar`) hands this foreign object to
 * `build_android_engine`, which bridges it onto `traits::NotificationService`
 * (via `AndroidNotificationBridge`). We only map the call shape + error types
 * here; the real `NotificationManager` post lives in [NotificationController].
 *
 * Mirrors [com.lingxi.code.share.AndroidShareAdapter]: a thin wrapper that
 * translates the local controller's failure surface onto the flat FFI error
 * type Rust fans back out onto the richer `traits::NotificationError`.
 */
class AndroidNotificationAdapter(
    private val controller: NotificationController = NotificationController,
) : AndroidNotification {

    override suspend fun notify(
        title: String,
        body: String,
        tag: String?,
    ) {
        try {
            controller.notify(title = title, body = body, tag = tag)
        } catch (e: NotifyException) {
            throw e.failure.toFfi()
        } catch (e: NotificationFfiException) {
            throw e
        } catch (t: Throwable) {
            throw NotificationFfiException.Other(t.message ?: "notify error")
        }
    }
}

/** Map the local [NotifyFailure] surface onto the flat FFI error enum. */
private fun NotifyFailure.toFfi(): NotificationFfiException = when (this) {
    is NotifyFailure.PermissionDenied -> NotificationFfiException.PermissionDenied()
    is NotifyFailure.Other -> NotificationFfiException.Other(message)
}
