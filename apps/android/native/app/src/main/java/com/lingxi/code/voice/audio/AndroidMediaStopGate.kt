package com.lingxi.code.voice.audio

import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

internal enum class AndroidMediaStopState { ACTIVE, STOPPING, STOPPED }

/** Serializes teardown, retaining a retryable STOPPING state until close is acknowledged. */
internal class AndroidMediaStopGate {
    private val mutex = Mutex()

    @Volatile
    var state: AndroidMediaStopState = AndroidMediaStopState.ACTIVE
        private set

    suspend fun stop(close: suspend () -> Unit, onStopped: () -> Unit = {}) {
        mutex.withLock {
            if (state == AndroidMediaStopState.STOPPED) return@withLock
            state = AndroidMediaStopState.STOPPING
            close()
            onStopped()
            state = AndroidMediaStopState.STOPPED
        }
    }
}
