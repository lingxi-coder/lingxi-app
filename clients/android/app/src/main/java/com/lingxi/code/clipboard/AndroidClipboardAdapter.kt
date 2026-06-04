package com.lingxi.code.clipboard

import com.lingxi.code.bindings.AndroidClipboard
import com.lingxi.code.bindings.ClipboardFfiException

/**
 * Adapts the native [ClipboardController] to the generated UniFFI callback
 * interface [AndroidClipboard].
 *
 * The Rust seam (`apps/android-aar`) hands this foreign object to
 * `build_android_engine`, which bridges it onto `traits::Clipboard` (via
 * `AndroidClipboardBridge`). We only map the call shape + error types here; the
 * real `ClipboardManager` access lives in [ClipboardController].
 *
 * Mirrors [com.lingxi.code.notify.AndroidNotificationAdapter]: a thin wrapper
 * that translates the local controller's failure surface onto the flat FFI
 * error type Rust fans back out onto the richer `traits::ClipboardError`.
 */
class AndroidClipboardAdapter(
    private val controller: ClipboardController = ClipboardController,
) : AndroidClipboard {

    override suspend fun setText(text: String) {
        try {
            controller.setText(text)
        } catch (e: ClipException) {
            throw e.failure.toFfi()
        } catch (e: ClipboardFfiException) {
            throw e
        } catch (t: Throwable) {
            throw ClipboardFfiException.Other(t.message ?: "clipboard set error")
        }
    }

    override suspend fun getText(): String? {
        return try {
            controller.getText()
        } catch (e: ClipException) {
            throw e.failure.toFfi()
        } catch (e: ClipboardFfiException) {
            throw e
        } catch (t: Throwable) {
            throw ClipboardFfiException.Other(t.message ?: "clipboard get error")
        }
    }
}

/** Map the local [ClipFailure] surface onto the flat FFI error enum. */
private fun ClipFailure.toFfi(): ClipboardFfiException = when (this) {
    is ClipFailure.Unsupported -> ClipboardFfiException.Unsupported()
    is ClipFailure.Other -> ClipboardFfiException.Other(message)
}
