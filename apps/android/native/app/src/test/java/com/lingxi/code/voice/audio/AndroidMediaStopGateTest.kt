package com.lingxi.code.voice.audio

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

class AndroidMediaStopGateTest {
    @Test
    fun partialOpenCleanupFailureBlocksLaterOpenUntilRetryAcknowledgesClose() = runBlocking {
        val registry = AndroidMediaOpenCleanupRegistry()
        val closeAttempts = AtomicInteger()
        var laterOpenSucceeded = false
        val cleanup = AndroidMediaOpenCleanup {
            if (closeAttempts.incrementAndGet() == 1) {
                throw IllegalStateException("partial-open close failed")
            }
        }

        assertTrue(runCatching { registry.retainAndRetry(cleanup) }.exceptionOrNull() is IllegalStateException)
        assertEquals(1, registry.pendingCount())
        assertTrue(runCatching { registry.retryPending() }.isSuccess)
        laterOpenSucceeded = true

        assertTrue(laterOpenSucceeded)
        assertEquals(2, closeAttempts.get())
        assertEquals(0, registry.pendingCount())
    }

    @Test
    fun cancelledPartialOpenKeepsCloseFlightUntilAcknowledged() = runBlocking {
        val registry = AndroidMediaOpenCleanupRegistry()
        val closeStarted = CompletableDeferred<Unit>()
        val allowClose = CompletableDeferred<Unit>()
        var closeCompleted = false
        val opener = launch {
            registry.retainAndRetry(AndroidMediaOpenCleanup {
                closeStarted.complete(Unit)
                allowClose.await()
                closeCompleted = true
            })
        }

        closeStarted.await()
        opener.cancel()
        assertFalse(opener.isCompleted)
        allowClose.complete(Unit)
        opener.cancelAndJoin()

        assertTrue(closeCompleted)
        assertEquals(0, registry.pendingCount())
    }

    @Test
    fun cameraCloseTimeoutKeepsOneBackgroundFlightAndLeaseUnknown() = runBlocking {
        val closeStarted = CompletableDeferred<Unit>()
        val allowClose = CompletableDeferred<Unit>()
        val closeAttempts = AtomicInteger()
        val flight = AndroidMediaCloseFlight(closeTimeoutMs = 25) {
            closeAttempts.incrementAndGet()
            closeStarted.complete(Unit)
            allowClose.await()
        }

        flight.start()
        closeStarted.await()
        assertTrue(runCatching { flight.awaitClosed() }.exceptionOrNull() is AndroidMediaCloseUnconfirmedException)
        assertEquals(1, closeAttempts.get())

        allowClose.complete(Unit)
        flight.awaitClosed()
        flight.start()
        assertEquals(1, closeAttempts.get())
    }

    @Test
    fun cameraCloseFlightRetriesOnlyAfterTerminalCallbacksAndNeverOverlapsAttempts() = runBlocking {
        val deviceClosed = AtomicBoolean(false)
        val sessionClosed = AtomicBoolean(false)
        val closeAttempts = AtomicInteger()
        val activeAttempts = AtomicInteger()
        val maxConcurrentAttempts = AtomicInteger()
        val flight = AndroidMediaCloseFlight(
            closeTimeoutMs = 1_000,
            canRetryAfterFailure = { deviceClosed.get() && sessionClosed.get() },
        ) {
            val active = activeAttempts.incrementAndGet()
            maxConcurrentAttempts.updateAndGet { maxOf(it, active) }
            try {
                if (closeAttempts.incrementAndGet() == 1) {
                    // Model a cleanup exception after Camera2 supplied both close receipts.
                    deviceClosed.set(true)
                    sessionClosed.set(true)
                    throw IllegalStateException("post-receipt reader cleanup failed")
                }
            } finally {
                activeAttempts.decrementAndGet()
            }
        }

        assertTrue(runCatching { flight.awaitClosed() }.exceptionOrNull() is IllegalStateException)
        assertEquals(1, closeAttempts.get())
        flight.start()
        assertEquals("fire-and-forget callback starts do not race a close retry", 1, closeAttempts.get())
        flight.awaitClosed()

        assertEquals(2, closeAttempts.get())
        assertEquals(1, maxConcurrentAttempts.get())
    }

    @Test
    fun cameraCloseFlightDoesNotRetryAnExceptionalAttemptWithoutTerminalReceipt() = runBlocking {
        val deviceClosed = AtomicBoolean(false)
        val closeAttempts = AtomicInteger()
        val flight = AndroidMediaCloseFlight(
            closeTimeoutMs = 1_000,
            canRetryAfterFailure = { deviceClosed.get() },
        ) {
            closeAttempts.incrementAndGet()
            throw IllegalStateException("close failed before Camera2 onClosed")
        }

        assertTrue(runCatching { flight.awaitClosed() }.exceptionOrNull() is IllegalStateException)
        assertTrue(runCatching { flight.awaitClosed() }.exceptionOrNull() is IllegalStateException)
        assertEquals("no close receipt means the old Camera2 request is still unknown", 1, closeAttempts.get())
    }

    @Test
    fun failedCloseRetainsLeaseForRetryAndCommitsOnlyOnce() = runBlocking {
        val gate = AndroidMediaStopGate()
        val closeAttempts = AtomicInteger()
        val leases = mutableMapOf("lease" to "open")

        val firstFailure = runCatching {
            gate.stop(
                close = {
                    closeAttempts.incrementAndGet()
                    throw IllegalStateException("native close failed")
                },
                onStopped = { leases.remove("lease") },
            )
        }.exceptionOrNull()

        assertTrue(firstFailure is IllegalStateException)
        assertEquals(AndroidMediaStopState.STOPPING, gate.state)
        assertEquals(1, closeAttempts.get())
        assertEquals("open", leases["lease"])

        gate.stop(
            close = { closeAttempts.incrementAndGet() },
            onStopped = { leases.remove("lease") },
        )
        gate.stop(
            close = { closeAttempts.incrementAndGet() },
            onStopped = { leases.remove("lease") },
        )

        assertEquals(AndroidMediaStopState.STOPPED, gate.state)
        assertEquals(2, closeAttempts.get())
        assertFalse(leases.containsKey("lease"))
    }

    @Test
    fun concurrentStopRetriesFailureWithoutPrematureSuccess() = runBlocking {
        val gate = AndroidMediaStopGate()
        val closeStarted = CompletableDeferred<Unit>()
        val allowFirstFailure = CompletableDeferred<Unit>()
        val closeAttempts = AtomicInteger()
        val commitAttempts = AtomicInteger()

        val first = async(Dispatchers.Default) {
            runCatching {
                gate.stop(
                    close = {
                        if (closeAttempts.incrementAndGet() == 1) {
                            closeStarted.complete(Unit)
                            allowFirstFailure.await()
                            throw IllegalStateException("first native close failed")
                        }
                    },
                    onStopped = { commitAttempts.incrementAndGet() },
                )
            }.exceptionOrNull()
        }
        closeStarted.await()
        val second = async(Dispatchers.Default) {
            gate.stop(
                close = { closeAttempts.incrementAndGet() },
                onStopped = { commitAttempts.incrementAndGet() },
            )
        }

        assertFalse(second.isCompleted)
        allowFirstFailure.complete(Unit)
        assertTrue(first.await() is IllegalStateException)
        second.await()
        gate.stop(
            close = { closeAttempts.incrementAndGet() },
            onStopped = { commitAttempts.incrementAndGet() },
        )

        assertEquals(AndroidMediaStopState.STOPPED, gate.state)
        assertEquals(2, closeAttempts.get())
        assertEquals(1, commitAttempts.get())
    }
}
