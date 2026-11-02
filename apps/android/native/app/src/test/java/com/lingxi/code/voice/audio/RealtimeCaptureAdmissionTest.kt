package com.lingxi.code.voice.audio

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class RealtimeCaptureAdmissionTest {
    @Test
    fun pauseDrainsAdmittedWriteAndDiscardsLateFramesEvenAfterResume() = runTest {
        val admission = RealtimeCaptureAdmission()
        val oldToken = admission.snapshot()
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        val writes = mutableListOf<String>()
        val first = launch {
            admission.deliver(oldToken) {
                entered.complete(Unit)
                release.await()
                writes += "first"
            }
        }
        entered.await()
        admission.setPaused(true)
        val drained = CompletableDeferred<Unit>()
        launch { admission.drain(); drained.complete(Unit) }
        runCurrent()
        assertFalse(drained.isCompleted)
        release.complete(Unit)
        first.join()
        drained.await()
        admission.deliver(oldToken) { writes += "late during pause" }
        admission.setPaused(false)
        admission.deliver(oldToken) { writes += "late after resume" }
        admission.deliver(admission.snapshot()) { writes += "new" }
        assertEquals(listOf("first", "new"), writes)
        assertTrue(drained.isCompleted)
    }
}
