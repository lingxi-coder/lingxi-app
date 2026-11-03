package com.lingxi.code.conversation

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DurableSubmissionTest {
    @Test fun stopWhileCheckpointIsSuspendedPreventsLaterNativeAdmission() = runBlocking {
        val checkpoint = CompletableDeferred<Unit>()
        var restored = 0
        var calls = 0
        val admission = NativeSubmissionGate { restored++ }
        val job = launch(start = CoroutineStart.UNDISPATCHED) {
            submitDurableTurn(
                prepare = { checkpoint.await() },
                submit = { if (admission.enter()) calls++ },
                onNotAdmitted = { admission.reject() },
            )
        }
        assertTrue(admission.reject())
        checkpoint.complete(Unit)
        job.join()
        assertEquals(1, restored)
        assertEquals(0, calls)
    }

    @Test fun stopAfterNativeAdmissionDoesNotRestoreOrDuplicateTheTurn() {
        var restored = 0
        val admission = NativeSubmissionGate { restored++ }
        assertTrue(admission.enter())
        assertTrue(!admission.reject())
        assertTrue(!admission.enter())
        assertEquals(0, restored)
        assertTrue(admission.rejectKnown())
        assertTrue(!admission.rejectKnown())
        assertEquals(1, restored)
    }
    @Test fun checkpointFailureRestoresBeforeAnyNativeCall() = runBlocking {
        val checkpoint = CompletableDeferred<Unit>()
        var restored = 0
        var calls = 0
        var failed = false
        val job = launch(start = CoroutineStart.UNDISPATCHED) {
            failed = runCatching {
                submitDurableTurn(
                    prepare = { checkpoint.await(); error("checkpoint failed") },
                    submit = { calls++ },
                    onNotAdmitted = { restored++ },
                )
            }.isFailure
        }
        assertEquals(0, calls)
        checkpoint.complete(Unit)
        job.join()
        assertTrue(failed)
        assertEquals(1, restored)
        assertEquals(0, calls)
    }

    @Test fun nativeCallFailureDoesNotClaimTheTurnWasRejected() = runBlocking {
        var restored = 0
        var calls = 0
        val result = runCatching {
            submitDurableTurn(
                prepare = {},
                submit = { calls++; error("unknown native outcome") },
                onNotAdmitted = { restored++ },
            )
        }
        assertTrue(result.isFailure)
        assertEquals(1, calls)
        assertEquals(0, restored)
    }
}
