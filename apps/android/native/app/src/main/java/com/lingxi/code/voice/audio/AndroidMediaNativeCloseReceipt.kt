package com.lingxi.code.voice.audio

import kotlinx.coroutines.CompletableDeferred
import java.util.concurrent.atomic.AtomicBoolean

/** A failed audio Open whose own dispatched work has an exact close receipt. */
internal class AndroidMediaAudioOpenClosed(val original: Throwable) : RuntimeException(original)

/** One stream and exact lease; absence of a callback remains unconfirmed. */
internal class AndroidMediaNativeCloseReceipt(val lease: AudioLease) {
    private val dispatched = AtomicBoolean(false)
    private val closed = CompletableDeferred<Unit>()

    fun markDispatched() {
        check(dispatched.compareAndSet(false, true))
    }

    /** Called only by the stream job's completion callback. */
    fun confirmIfNeverDispatched() {
        if (!dispatched.get()) closed.complete(Unit)
    }

    /** Only the native stream may confirm no allocation or its own release. */
    fun confirmNoResource() {
        closed.complete(Unit)
    }

    fun failNativeClose(error: Throwable) {
        closed.completeExceptionally(error)
    }

    suspend fun awaitClosed() = closed.await()
}
