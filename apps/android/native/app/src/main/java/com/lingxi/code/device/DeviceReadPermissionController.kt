package com.lingxi.code.device

import com.lingxi.code.bindings.android.DeviceControlFfiException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/**
 * One Activity's calendar/contacts permission UI. Only native execution after
 * engine authorization reaches this controller; attachment never prompts.
 * All state and ActivityResult callbacks are confined to the main dispatcher.
 */
internal class DeviceReadPermissionController(
    private val isGranted: (String) -> Boolean,
    private val isForeground: () -> Boolean,
    private val requestPermission: (String) -> Unit,
    private val dispatcher: CoroutineDispatcher = Dispatchers.Main.immediate,
    private val timeoutMs: Long = 60_000L,
) {
    private var attached = true
    private var pending: CompletableDeferred<Boolean>? = null

    suspend fun ensurePermission(permission: String, resource: String) = withContext(dispatcher) {
        if (!attached) throw DeviceControlFfiException.Unavailable()
        if (isGranted(permission)) return@withContext
        if (!isForeground()) throw DeviceControlFfiException.Unavailable()
        if (pending != null) {
            throw DeviceControlFfiException.Other("another device permission request is already in flight")
        }

        val result = CompletableDeferred<Boolean>()
        pending = result
        try {
            requestPermission(permission)
        } catch (_: Exception) {
            if (pending === result) pending = null
            throw DeviceControlFfiException.Unavailable()
        }

        // An OS permission dialog cannot be cancelled through ActivityResult.
        // Retain its slot on cancellation/timeout until its result arrives, so
        // a late result can never be mistaken for a subsequent request.
        val granted = withTimeoutOrNull(timeoutMs) { result.await() }
            ?: throw DeviceControlFfiException.Other("$resource permission request timed out")
        if (!attached) throw DeviceControlFfiException.Unavailable()
        if (!granted || !isGranted(permission)) {
            throw DeviceControlFfiException.Rejected("$resource permission denied")
        }
    }

    /** Called on the main thread by this Activity's own launcher callback. */
    fun onPermissionResult(granted: Boolean) {
        val result = pending ?: return
        pending = null
        result.complete(granted)
    }

    /** Called on the main thread when the owning Activity is destroyed/replaced. */
    fun detach() {
        attached = false
        val result = pending
        pending = null
        result?.completeExceptionally(DeviceControlFfiException.Unavailable())
    }
}
