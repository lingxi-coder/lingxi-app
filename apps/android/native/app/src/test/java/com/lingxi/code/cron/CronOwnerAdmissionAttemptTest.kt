package com.lingxi.code.cron

import com.lingxi.code.conversation.SessionAdmissionGate
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class CronOwnerAdmissionAttemptTest {
    private val identity = CronOwnerIdentity("19587a33-0725-48db-abca-8a2aed345f6b", "/guest/exact.jsonl", "chat", "scheduled")
    private val candidate = CronOwnerCandidate(identity.sessionId, null, identity.mode, identity.scopeKey)
    private val otherId = "29587a33-0725-48db-abca-8a2aed345f6b"

    private fun clearGate() = SessionAdmissionGate.completeOwnerScan(SessionAdmissionGate.scanEpoch())

    @Test fun `only the selected UUID is reserved until confirmation`() = runTest {
        var writes = 0
        val attempt = CronOwnerAdmissionAttempt { _, _ -> writes++; true }
        try {
            assertTrue(SessionAdmissionGate.permitsNewOwner(identity.sessionId))
            assertTrue(attempt.begin(candidate))
            assertTrue(attempt.begin(candidate))
            assertFalse(SessionAdmissionGate.permitsNewOwner(identity.sessionId))
            assertTrue(SessionAdmissionGate.permitsNewOwner(otherId))
            assertTrue(attempt.confirm(identity))
            assertTrue(SessionAdmissionGate.permitsNewOwner(identity.sessionId))
            assertTrue(attempt.confirm(identity))
            assertEquals(1, writes)
        } finally { attempt.finishCaller(); attempt.finishNative(); clearGate() }
    }

    @Test fun `caller cancellation waits for callback and fences all new claims until strict scan`() = runTest {
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        val attempt = CronOwnerAdmissionAttempt { _, _ -> entered.complete(Unit); release.await(); true }
        try {
            assertTrue(attempt.begin(candidate))
            val result = async { attempt.confirm(identity) }
            entered.await()
            val stale = SessionAdmissionGate.scanEpoch()
            attempt.finishCaller()
            assertFalse(SessionAdmissionGate.permitsNewOwner(identity.sessionId))
            assertTrue(SessionAdmissionGate.permitsNewOwner(otherId))
            SessionAdmissionGate.completeOwnerScan(stale)
            release.complete(Unit)
            assertFalse(result.await())
            assertFalse(SessionAdmissionGate.needsOwnerScan())
            assertFalse(SessionAdmissionGate.permitsNewOwner(identity.sessionId))
            assertTrue(SessionAdmissionGate.permitsNewOwner(otherId))
            attempt.finishNative()
            assertTrue(SessionAdmissionGate.needsOwnerScan())
            assertFalse(SessionAdmissionGate.permitsNewOwner(identity.sessionId))
            assertFalse(SessionAdmissionGate.permitsNewOwner(otherId))
            clearGate()
            assertTrue(SessionAdmissionGate.permitsNewOwner(identity.sessionId))
            assertTrue(SessionAdmissionGate.permitsNewOwner(otherId))
        } finally { release.complete(Unit); attempt.finishCaller(); attempt.finishNative(); clearGate() }
    }

    @Test fun `duplicate confirmations share one durable admission and a late callback is rejected`() = runTest {
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        var writes = 0
        val attempt = CronOwnerAdmissionAttempt { _, _ -> writes++; entered.complete(Unit); release.await(); true }
        try {
            assertTrue(attempt.begin(candidate))
            val first = async { attempt.confirm(identity) }
            entered.await()
            val second = async { attempt.confirm(identity) }
            runCurrent()
            release.complete(Unit)
            assertTrue(first.await())
            assertTrue(second.await())
            assertEquals(1, writes)
            attempt.finishCaller()
            assertFalse(attempt.confirm(identity))
        } finally { release.complete(Unit); attempt.finishCaller(); attempt.finishNative(); clearGate() }
    }

    @Test fun `cancellation before begin rejects late begin and does not block unrelated owners`() = runTest {
        val attempt = CronOwnerAdmissionAttempt { _, _ -> error("late callback must not persist") }
        try {
            attempt.finishCaller()
            assertFalse(attempt.begin(candidate))
            assertTrue(SessionAdmissionGate.permitsNewOwner(identity.sessionId))
            assertTrue(SessionAdmissionGate.permitsNewOwner(otherId))
            attempt.finishNative()
            assertFalse(SessionAdmissionGate.needsOwnerScan())
        } finally { attempt.finishNative(); clearGate() }
    }

    @Test fun `mismatched callback never writes and requires recovery`() = runTest {
        val attempt = CronOwnerAdmissionAttempt { _, _ -> error("wrong UUID must not persist") }
        try {
            assertTrue(attempt.begin(candidate))
            assertFalse(attempt.confirm(identity.copy(sessionId = otherId)))
            assertFalse(SessionAdmissionGate.permitsNewOwner(otherId))
        } finally { attempt.finishCaller(); attempt.finishNative(); clearGate() }
    }

    @Test fun `confirm must preserve the candidate exact path and fixed mode`() = runTest {
        val attempt = CronOwnerAdmissionAttempt { _, _ -> error("changed source must not persist") }
        try {
            assertTrue(attempt.begin(candidate.copy(transcriptPath = "/guest/original.jsonl")))
            assertFalse(attempt.confirm(identity.copy(transcriptPath = "/guest/other.jsonl")))
            assertFalse(SessionAdmissionGate.permitsNewOwner(otherId))
        } finally { attempt.finishCaller(); attempt.finishNative(); clearGate() }
    }

    @Test fun `cancellation during candidate validation holds only that UUID and never permits late begin`() = runTest {
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        val attempt = CronOwnerAdmissionAttempt(
            validateCandidate = { _, _ -> entered.complete(Unit); release.await(); true },
            persist = { _, _ -> error("rejected begin cannot persist") },
        )
        try {
            val result = async { attempt.begin(candidate) }
            entered.await()
            attempt.finishCaller()
            assertFalse(SessionAdmissionGate.permitsNewOwner(identity.sessionId))
            assertTrue(SessionAdmissionGate.permitsNewOwner(otherId))
            release.complete(Unit)
            assertFalse(result.await())
            attempt.finishNative()
            assertTrue(SessionAdmissionGate.permitsNewOwner(identity.sessionId))
        } finally { release.complete(Unit); attempt.finishNative(); clearGate() }
    }

    @Test fun `failed native barrier remains closed until barrier and strict scan succeed`() = runTest {
        val attempt = CronOwnerAdmissionAttempt { _, _ -> true }
        try {
            assertTrue(runCatching { attempt.settleAfterCaller { error("barrier failed") } }.isFailure)
            assertTrue(SessionAdmissionGate.hasBarrierFailure())
            assertFalse(SessionAdmissionGate.permitsNewOwner(otherId))
            clearGate()
            assertFalse(SessionAdmissionGate.permitsNewOwner(otherId))
            attempt.settleAfterCaller { }
            clearGate()
            assertFalse(SessionAdmissionGate.hasBarrierFailure())
            assertTrue(SessionAdmissionGate.permitsNewOwner(otherId))
        } finally { attempt.finishNative(); clearGate() }
    }

    @Test fun `previously accepted recovery waits for competing reservation and survives storage fencing`() = runTest {
        val token = checkNotNull(SessionAdmissionGate.beginCronReservation(identity.sessionId, "scheduled", com.lingxi.code.model.SessionMode.Chat, null))
        var preparation: Long? = null
        try {
            val waiting = async { SessionAdmissionGate.awaitSourcePreparation(identity.sessionId, admittedRecovery = true) }
            runCurrent()
            assertFalse(waiting.isCompleted)
            SessionAdmissionGate.finishCronReservation(token, committed = false)
            preparation = waiting.await()
            assertTrue(SessionAdmissionGate.needsOwnerScan())
            assertFalse(SessionAdmissionGate.permitsNewOwner(otherId))
        } finally {
            preparation?.let(SessionAdmissionGate::finishSourcePreparation)
            SessionAdmissionGate.finishCronReservation(token, committed = false)
            clearGate()
        }
    }

    @Test fun `previously accepted recovery waits for scope mutation before opening its source`() = runTest {
        val scope = SessionAdmissionGate.awaitScopeChange()
        var preparation: Long? = null
        try {
            val waiting = async { SessionAdmissionGate.awaitSourcePreparation(identity.sessionId, admittedRecovery = true) }
            runCurrent()
            assertFalse(waiting.isCompleted)
            SessionAdmissionGate.finishScopeChange(scope, validated = true)
            preparation = waiting.await()
        } finally {
            preparation?.let(SessionAdmissionGate::finishSourcePreparation)
            SessionAdmissionGate.finishScopeChange(scope, validated = true)
        }
    }
}
