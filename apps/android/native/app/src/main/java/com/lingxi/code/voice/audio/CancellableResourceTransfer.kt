package com.lingxi.code.voice.audio

import kotlinx.coroutines.CancellableContinuation
import kotlin.coroutines.resume

/** Transfers a native handle only if cancellation does not discard the result. */
internal fun <T> CancellableContinuation<T>.resumeResource(
    resource: T,
    closeIfCancelled: (T) -> Unit,
) {
    resume(resource) { _, undelivered, _ -> closeIfCancelled(undelivered) }
}
