package com.lingxi.code.voice.audio

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withContext

internal fun interface AndroidMediaOpenCleanup {
    suspend fun close()
}

internal class AndroidMediaCloseUnconfirmedException : IllegalStateException("Android media close remains unconfirmed")

/** One background close operation shared by bounded waiters; timeout never cancels the close. */
internal class AndroidMediaCloseFlight(
    private val closeTimeoutMs: Long,
    private val canRetryAfterFailure: () -> Boolean = { false },
    private val closeResource: suspend () -> Unit,
) {
    private val lock = Any()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    @Volatile private var completion: CompletableDeferred<Unit>? = null
    private var terminalFailure: Throwable? = null
    private var terminalFailureRetryable = false

    init {
        require(closeTimeoutMs > 0)
    }

    fun start() {
        synchronized(lock) {
            if (completion == null) startLocked()
        }
    }

    private fun startLocked() {
        val finished = CompletableDeferred<Unit>()
        completion = finished
        terminalFailure = null
        terminalFailureRetryable = false
        scope.launch {
            try {
                closeResource()
                finished.complete(Unit)
            } catch (error: Throwable) {
                // A retry is safe only when the resource-specific close protocol says the
                // previous attempt reached a terminal receipt. A bounded waiter timeout does
                // not complete this flight and therefore never starts an overlapping retry.
                val retryable = error !is CancellationException &&
                    runCatching(canRetryAfterFailure).getOrDefault(false)
                synchronized(lock) {
                    if (completion === finished) {
                        terminalFailure = error
                        terminalFailureRetryable = retryable
                    }
                }
                finished.completeExceptionally(error)
            }
        }
    }

    suspend fun awaitClosed() = withContext(NonCancellable) {
        val finished = synchronized(lock) {
            val previous = completion
            if (previous == null ||
                (previous.isCompleted && terminalFailure != null && terminalFailureRetryable)
            ) {
                startLocked()
            }
            checkNotNull(completion)
        }
        try {
            withTimeout(closeTimeoutMs) { finished.await() }
        } catch (_: TimeoutCancellationException) {
            throw AndroidMediaCloseUnconfirmedException()
        }
    }
}

/** Keeps partial-open ownership until a later close attempt is acknowledged. */
internal class AndroidMediaOpenCleanupRegistry {
    private val mutex = Mutex()
    private val pending = mutableListOf<AndroidMediaOpenCleanup>()

    suspend fun retainAndRetry(cleanup: AndroidMediaOpenCleanup) = withContext(NonCancellable) {
        mutex.withLock {
            pending += cleanup
            retryLocked()
        }
    }

    suspend fun retryPending() = withContext(NonCancellable) {
        mutex.withLock { retryLocked() }
    }

    suspend fun pendingCount(): Int = mutex.withLock { pending.size }

    private suspend fun retryLocked() {
        while (pending.isNotEmpty()) {
            val cleanup = pending.first()
            cleanup.close()
            pending.removeAt(0)
        }
    }
}
