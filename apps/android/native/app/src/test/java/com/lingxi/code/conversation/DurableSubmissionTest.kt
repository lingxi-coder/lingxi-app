package com.lingxi.code.conversation

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.async
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import com.lingxi.code.bindings.client.ClientException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DurableSubmissionTest {
    private val cancelledBeforeAdmission = "turn_cancelled_before_admission"

    @Test fun sourceSessionBindsOnceAtDispatchAndRejectsPlaceholderOrReplacement() = runTest {
        val owner = Any()
        var restored = 0
        val gate = NativeSubmissionGate { restored++ }.apply { assertTrue(enter()) }
        val submission = NativePromptSubmission(owner, 42L, gate)
        submission.record(Result.failure(ClientException.Rejected(cancelledBeforeAdmission)))
        assertTrue(!submission.bindIdentity(owner, NativePromptIdentity("new"), 42L))
        assertTrue(!submission.bindIdentity(Any(), NativePromptIdentity("session-a"), 42L))
        assertTrue(!submission.bindIdentity(owner, NativePromptIdentity("session-a"), 43L))
        assertTrue(!stopWithSubmissionReceipt({}, submission, owner, NativePromptIdentity("session-a"), 42L, 100) { true })
        assertTrue(submission.bindIdentity(owner, NativePromptIdentity("session-a"), 42L))
        assertTrue(!submission.bindIdentity(owner, NativePromptIdentity("session-b"), 42L))
        assertTrue(!stopWithSubmissionReceipt({}, submission, owner, NativePromptIdentity("session-b"), 42L, 100) { true })
        assertTrue(stopWithSubmissionReceipt({}, submission, owner, NativePromptIdentity("session-a"), 42L, 100) { true })
        assertEquals(1, restored)
    }

    @Test fun rejectedReceiptBeforeCancelAckWaitsForExactStop() = runTest {
        val owner = Any()
        val cancelAck = CompletableDeferred<Unit>()
        var restored = 0
        var cleared = 0
        val gate = NativeSubmissionGate { restored++ }.apply { assertTrue(enter()) }
        val submission = NativePromptSubmission(owner, 42L, gate).apply { assertTrue(bindIdentity(owner, NativePromptIdentity("session-a"), 42L)) }
        val stop = async(start = CoroutineStart.UNDISPATCHED) {
            stopWithSubmissionReceipt(
                stop = { cancelAck.await() }, submission = submission, owner = owner,
                identity = NativePromptIdentity("session-a"), turnId = 42L, timeoutMs = 100,
                clearCheckpoint = { cleared++; true },
            )
        }
        val result = runCatching {
            submitWithReceipt(
                submit = { throw ClientException.Rejected(cancelledBeforeAdmission) },
                record = submission::record,
            )
        }
        assertTrue(result.exceptionOrNull() is ClientException.Rejected)
        assertEquals(0, restored)
        assertEquals(0, cleared)
        cancelAck.complete(Unit)
        assertTrue(stop.await())
        assertEquals(1, restored)
        assertEquals(1, cleared)
        assertTrue(!stopWithSubmissionReceipt({}, submission, owner, NativePromptIdentity("session-a"), 42L, 100) { cleared++; true })
        assertEquals(1, restored)
        assertEquals(1, cleared)
    }

    @Test fun cancelAckBeforeRejectedReceiptWaitsForOriginalSend() = runTest {
        val owner = Any()
        var restored = 0
        var cleared = 0
        val gate = NativeSubmissionGate { restored++ }.apply { assertTrue(enter()) }
        val submission = NativePromptSubmission(owner, 42L, gate).apply { assertTrue(bindIdentity(owner, NativePromptIdentity("session-a"), 42L)) }
        val stop = async(start = CoroutineStart.UNDISPATCHED) {
            stopWithSubmissionReceipt(
                stop = {}, submission = submission, owner = owner,
                identity = NativePromptIdentity("session-a"), turnId = 42L, timeoutMs = 100,
                clearCheckpoint = { cleared++; true },
            )
        }
        assertEquals(0, restored)
        submission.record(Result.failure(ClientException.Rejected(cancelledBeforeAdmission)))
        assertTrue(stop.await())
        assertEquals(1, restored)
        assertEquals(1, cleared)
    }

    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun unknownTimeoutAndLateReceiptDoNotClearOrRestore() = runTest {
        val owner = Any()
        var restored = 0
        var cleared = 0
        val gate = NativeSubmissionGate { restored++ }.apply { assertTrue(enter()) }
        val submission = NativePromptSubmission(owner, 42L, gate).apply { assertTrue(bindIdentity(owner, NativePromptIdentity("session-a"), 42L)) }
        val stop = async(start = CoroutineStart.UNDISPATCHED) {
            stopWithSubmissionReceipt(
                stop = {}, submission = submission, owner = owner,
                identity = NativePromptIdentity("session-a"), turnId = 42L, timeoutMs = 100,
                clearCheckpoint = { cleared++; true },
            )
        }
        advanceTimeBy(101)
        runCurrent()
        assertTrue(!stop.await())
        submission.record(Result.failure(ClientException.Rejected(cancelledBeforeAdmission)))
        runCurrent()
        assertEquals(0, restored)
        assertEquals(0, cleared)
    }

    @Test fun failedCancelAndOtherSubmissionErrorsDoNotClaimNonAdmission() = runTest {
        for (failure in listOf(
            ClientException.Rejected("another_rejection"),
            IllegalStateException(cancelledBeforeAdmission),
        )) {
            val owner = Any()
            var restored = 0
            var cleared = 0
            val gate = NativeSubmissionGate { restored++ }.apply { assertTrue(enter()) }
            val submission = NativePromptSubmission(owner, 42L, gate).apply { assertTrue(bindIdentity(owner, NativePromptIdentity("session-a"), 42L)) }
            submission.record(Result.failure(failure))
            assertTrue(!stopWithSubmissionReceipt({}, submission, owner, NativePromptIdentity("session-a"), 42L, 100) { cleared++; true })
            assertEquals(0, restored)
            assertEquals(0, cleared)
        }
        val owner = Any()
        var restored = 0
        var cleared = 0
        val gate = NativeSubmissionGate { restored++ }.apply { assertTrue(enter()) }
        val submission = NativePromptSubmission(owner, 42L, gate).apply { assertTrue(bindIdentity(owner, NativePromptIdentity("session-a"), 42L)) }
        submission.record(Result.failure(ClientException.Rejected(cancelledBeforeAdmission)))
        val failure = runCatching {
            stopWithSubmissionReceipt({ error("cancel failed") }, submission, owner, NativePromptIdentity("session-a"), 42L, 100) { cleared++; true }
        }
        assertTrue(failure.isFailure)
        assertEquals(0, restored)
        assertEquals(0, cleared)
    }

    @Test fun acceptedSendAndFailedCheckpointClearDoNotClaimNonAdmission() = runTest {
        val owner = Any()
        var restored = 0
        var cleared = 0
        val acceptedGate = NativeSubmissionGate { restored++ }.apply { assertTrue(enter()) }
        val accepted = NativePromptSubmission(owner, 42L, acceptedGate).apply { assertTrue(bindIdentity(owner, NativePromptIdentity("session-a"), 42L)) }
        accepted.record(Result.success(Unit))
        assertTrue(!stopWithSubmissionReceipt({}, accepted, owner, NativePromptIdentity("session-a"), 42L, 100) { cleared++; true })
        assertEquals(0, restored)
        assertEquals(0, cleared)

        val rejectedGate = NativeSubmissionGate { restored++ }.apply { assertTrue(enter()) }
        val rejected = NativePromptSubmission(owner, 43L, rejectedGate).apply { assertTrue(bindIdentity(owner, NativePromptIdentity("session-a"), 43L)) }
        rejected.record(Result.failure(ClientException.Rejected(cancelledBeforeAdmission)))
        val failure = runCatching {
            stopWithSubmissionReceipt({}, rejected, owner, NativePromptIdentity("session-a"), 43L, 100) { false }
        }
        assertTrue(failure.isFailure)
        assertEquals(0, restored)
    }

    @Test fun receiptCannotSettleAnotherOwnerSessionOrTurn() = runTest {
        val owner = Any()
        var restored = 0
        var cleared = 0
        val gate = NativeSubmissionGate { restored++ }.apply { assertTrue(enter()) }
        val submission = NativePromptSubmission(owner, 42L, gate).apply { assertTrue(bindIdentity(owner, NativePromptIdentity("session-a"), 42L)) }
        submission.record(Result.failure(ClientException.Rejected(cancelledBeforeAdmission)))
        suspend fun settle(otherOwner: Any, session: String, turn: Long?) =
            stopWithSubmissionReceipt({}, submission, otherOwner, NativePromptIdentity(session), turn, 100) { cleared++; true }
        assertTrue(!settle(Any(), "session-a", 42L))
        assertTrue(!settle(owner, "session-b", 42L))
        assertTrue(!settle(owner, "session-a", 43L))
        assertTrue(!settle(owner, "session-a", null))
        assertEquals(0, restored)
        assertEquals(0, cleared)
        assertTrue(settle(owner, "session-a", 42L))
        assertEquals(1, restored)
        assertEquals(1, cleared)
    }

    @Test fun sideReceiptRequiresExactParentSideOwnerAndTurn() = runTest {
        val owner = Any()
        var restored = 0
        val gate = NativeSubmissionGate { restored++ }.apply { assertTrue(enter()) }
        val submission = NativePromptSubmission(owner, 42L, gate)
        val identity = NativePromptIdentity("parent-a", "side-a")
        assertTrue(!submission.bindIdentity(owner, NativePromptIdentity("parent-a", ""), 42L))
        assertTrue(submission.bindIdentity(owner, identity, 42L))
        submission.record(Result.failure(ClientException.Rejected(cancelledBeforeAdmission)))
        assertTrue(!stopWithSubmissionReceipt({}, submission, Any(), identity, 42L, 100) { true })
        assertTrue(!stopWithSubmissionReceipt({}, submission, owner, NativePromptIdentity("parent-b", "side-a"), 42L, 100) { true })
        assertTrue(!stopWithSubmissionReceipt({}, submission, owner, NativePromptIdentity("parent-a", "side-b"), 42L, 100) { true })
        assertTrue(!stopWithSubmissionReceipt({}, submission, owner, identity, 43L, 100) { true })
        assertEquals(0, restored)
        assertTrue(stopWithSubmissionReceipt({}, submission, owner, identity, 42L, 100) { true })
        assertEquals(1, restored)
    }

    @Test fun stoppedReceiptPreservesEditedOriginalDraftWithoutTouchingAnotherOwner() = runTest {
        val owner = Any()
        var originalRevision = 0
        var originalDraft = ""
        var selectedOwnerDraft = "other owner"
        val preserved = mutableListOf<String>()
        val composer = ComposerRestoreGate(
            revision = { originalRevision },
            preserve = { preserved += "original + photo" },
            restore = { originalDraft = "original + photo" },
        )
        val gate = NativeSubmissionGate(composer::rejected).apply { assertTrue(enter()) }
        val submission = NativePromptSubmission(owner, 42L, gate).apply { assertTrue(bindIdentity(owner, NativePromptIdentity("session-a"), 42L)) }
        composer.cleared()
        // The original owner was edited and cleared again while another owner was visible.
        originalDraft = "new edit"
        originalRevision++
        originalDraft = ""
        originalRevision++
        selectedOwnerDraft = "other owner edited"
        submission.record(Result.failure(ClientException.Rejected(cancelledBeforeAdmission)))
        assertTrue(stopWithSubmissionReceipt({}, submission, owner, NativePromptIdentity("session-a"), 42L, 100) { true })
        assertEquals("", originalDraft)
        assertEquals(listOf("original + photo"), preserved)
        assertEquals("other owner edited", selectedOwnerDraft)
    }

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
