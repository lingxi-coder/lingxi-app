package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.MessageImageDto
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ThumbnailInFlightTest {
    @Test
    fun readOnlyHistoryReservesTheSameImageSlotBeforeAttachmentsAreReady() {
        val message = Message(Role.User, "photo", remoteImages = listOf(MessageImageDto("image/png", "image-ref")))
        val placeholder = chatImageThumbnails(message, "session", null).single()
        val ready = chatImageThumbnails(message, "session") { _, _ -> byteArrayOf() }.single()
        assertEquals(placeholder.key, ready.key)
        assertNull(placeholder.preview)
        assertNotNull(ready.preview)
    }

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
