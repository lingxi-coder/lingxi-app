package com.lingxi.code.clipboard

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.util.Log

private const val TAG = "ClipboardController"

/** A user-visible label for the clip the engine writes. */
private const val CLIP_LABEL = "lingxi_agent"

/** Why a clipboard op failed; mapped onto `ClipboardFfiException` by the adapter. */
sealed interface ClipFailure {
    /**
     * The platform does not permit this operation (e.g. no attached context, or
     * an Android 10+ read restriction the controller chose to surface as an
     * error rather than an empty read).
     */
    data object Unsupported : ClipFailure
    data class Other(val message: String) : ClipFailure
}

/** Raised by the controller; the adapter fans it onto `ClipboardFfiException`. */
class ClipException(val failure: ClipFailure) : Exception(
    when (failure) {
        is ClipFailure.Other -> failure.message
        else -> failure::class.simpleName ?: "clipboard error"
    },
)

/**
 * Process-global bridge between the (Rust-driven)
 * `com.lingxi.code.bindings.android.AndroidClipboard` callback interface and the system
 * [ClipboardManager].
 *
 * Mirrors [com.lingxi.code.notify.NotificationController]: because the engine
 * has no [Context] handle, the host Activity attaches the application context in
 * `onCreate` ([attach]) and clears it in `onDestroy` ([detach]).
 *
 * Both ops are engine-driven (`tool-clipboard`), so there is no user-facing
 * affordance — the model writes/reads the clipboard by calling [setText] /
 * [getText].
 *
 * No manifest permission is needed for clipboard access. Note that Android 10+
 * (API 29) restricts clipboard *reads* to the focused app / default IME; when a
 * background read is not permitted the system simply returns no primary clip, so
 * [getText] returns `null` rather than crashing.
 */
object ClipboardController {

    /** Context wired up by the host Activity; null when nothing is attached. */
    @Volatile
    private var context: Context? = null

    /** Register the host Activity's (application) context. */
    fun attach(context: Context) {
        this.context = context.applicationContext
    }

    fun detach() {
        context = null
    }

    /**
     * Write plain [text] to the system clipboard via
     * [ClipData.newPlainText] + [ClipboardManager.setPrimaryClip].
     *
     * Throws [ClipException] with [ClipFailure.Unsupported] when no context is
     * attached; with [ClipFailure.Other] on any other native failure.
     */
    fun setText(text: String) {
        val ctx = context ?: throw ClipException(ClipFailure.Unsupported)
        try {
            val manager = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            manager.setPrimaryClip(ClipData.newPlainText(CLIP_LABEL, text))
        } catch (e: ClipException) {
            throw e
        } catch (t: Throwable) {
            Log.w(TAG, "clipboard set failed: ${t.message}")
            throw ClipException(ClipFailure.Other(t.message ?: "clipboard set failed"))
        }
    }

    /**
     * Read plain text from the system clipboard via
     * `primaryClip?.getItemAt(0)?.coerceToText(context)?.toString()`.
     *
     * Returns `null` when the clipboard is empty or when a background read is
     * not permitted (Android 10+ restricts reads to the focused app / default
     * IME — the system surfaces this as no primary clip, not a crash).
     */
    fun getText(): String? {
        val ctx = context ?: return null
        return try {
            val manager = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            val clip = manager.primaryClip ?: return null
            if (clip.itemCount == 0) return null
            clip.getItemAt(0)?.coerceToText(ctx)?.toString()
        } catch (t: Throwable) {
            // Defensive: a denied read can surface as a SecurityException on some
            // OEM builds. Degrade to "nothing readable" rather than crash.
            Log.w(TAG, "clipboard get failed: ${t.message}")
            null
        }
    }
}
