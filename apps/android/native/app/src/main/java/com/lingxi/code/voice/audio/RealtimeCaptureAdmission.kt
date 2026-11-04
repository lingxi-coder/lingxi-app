package com.lingxi.code.voice.audio

import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** Orders capture delivery against pause and lets a caller wait for an in-flight write. */
internal class RealtimeCaptureAdmission {
    // The low bit means paused; the remaining bits change on every pause transition.
    private val state = AtomicLong(0)
    private val delivery = Mutex()

    fun setPaused(paused: Boolean) {
        state.updateAndGet { previous -> ((previous and -2L) + 2L) or (if (paused) 1L else 0L) }
    }

    fun snapshot(): Long = state.get()

    fun isCurrent(token: Long): Boolean = (token and 1L) == 0L && state.get() == token

    suspend fun deliver(token: Long, send: suspend () -> Unit) {
        delivery.withLock {
            if (isCurrent(token)) send()
        }
    }

    suspend fun drain() { delivery.withLock { } }
}
