package com.lingxi.code.voice.audio

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.CancellableContinuation
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.coroutines.CoroutineContext

class CancellableResourceTransferTest {
    private class QueuedDispatcher : CoroutineDispatcher() {
        private val queued = ArrayDeque<Runnable>()

        override fun dispatch(context: CoroutineContext, block: Runnable) {
            queued.addLast(block)
        }

        fun drain() {
            while (queued.isNotEmpty()) queued.removeFirst().run()
        }
    }

    @Test
    fun lateCameraOpenResultClosesHandleAfterCancellation() {
        val dispatcher = QueuedDispatcher()
        val scope = CoroutineScope(Job() + dispatcher)
        var closeCount = 0
        lateinit var continuation: CancellableContinuation<Unit>
        val job = scope.launch(start = CoroutineStart.UNDISPATCHED) {
            suspendCancellableCoroutine<Unit> { continuation = it }
        }

        job.cancel()
        continuation.resumeResource(Unit) { closeCount++ }
        dispatcher.drain()

        assertEquals(1, closeCount)
        scope.coroutineContext[Job]?.cancel()
    }

    @Test
    fun successfulCameraHandleDeliveryKeepsTheHandleOpen() {
        val dispatcher = QueuedDispatcher()
        val scope = CoroutineScope(Job() + dispatcher)
        var closeCount = 0
        lateinit var continuation: CancellableContinuation<Unit>
        val job = scope.launch(start = CoroutineStart.UNDISPATCHED) {
            suspendCancellableCoroutine<Unit> { continuation = it }
        }

        continuation.resumeResource(Unit) { closeCount++ }
        dispatcher.drain()

        assertTrue(job.isCompleted)
        assertEquals(0, closeCount)
        scope.coroutineContext[Job]?.cancel()
    }
}
