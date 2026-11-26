package com.lingxi.code.conversation

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ThumbnailInFlightTest {
    @Test
    fun visibleSubscriberKeepsThePrefetchAlive() = runTest {
        val requests = ThumbnailInFlight<String>(backgroundScope)
        val fetched = CompletableDeferred<String>()
        var loads = 0
        val load: suspend () -> String = { loads++; fetched.await() }
        val prefetch = async { requests.await("session:image:size", load) }
        runCurrent()
        val visible = async { requests.await("session:image:size", load) }
        runCurrent()

        prefetch.cancelAndJoin()
        fetched.complete("decoded")
        assertEquals("decoded", visible.await())
        assertEquals(1, loads)
    }

    @Test
    fun abandoningTheLastSubscriberCancelsWorkAndAllowsRetry() = runTest {
        val requests = ThumbnailInFlight<String>(backgroundScope)
        val fetched = CompletableDeferred<String>()
        var cancelled = false
        val prefetch = async {
            requests.await("key") {
                try {
                    fetched.await()
                } finally {
                    cancelled = true
                }
            }
        }
        runCurrent()

        prefetch.cancelAndJoin()
        runCurrent()
        assertTrue(cancelled)
        assertEquals("retried", requests.await("key") { "retried" })
    }
}
