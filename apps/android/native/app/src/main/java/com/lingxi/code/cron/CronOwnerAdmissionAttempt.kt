package com.lingxi.code.cron

import com.lingxi.code.conversation.SessionAdmissionGate
import com.lingxi.code.model.canonicalSessionId
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withContext

internal data class CronOwnerIdentity(val sessionId: String, val transcriptPath: String, val mode: String, val scopeKey: String)
internal data class CronOwnerCandidate(val sessionId: String, val transcriptPath: String?, val mode: String, val scopeKey: String)

/** Keeps admission fenced if the caller exits while its SDK callback is still running. */
internal class CronOwnerAdmissionAttempt(
    private val validateCandidate: suspend (CronOwnerCandidate, Long) -> Boolean = { _, _ -> true },
    private val persist: suspend (CronOwnerIdentity, Long) -> Boolean,
) {
    private val lock = Any()
    private var token: Long? = null
    private var reservedId: String? = null
    private var candidate: CronOwnerCandidate? = null
    private var beginInFlight = false
    private var beginAccepted = false
    private var beginResult: CompletableDeferred<Boolean>? = null
    private var callerFinished = false
    private var nativeFinished = false
    private var callbackInFlight = false
    private var callbackIdentity: CronOwnerIdentity? = null
    private var callbackResult: CompletableDeferred<Boolean>? = null
    private var ended = false
    private var barrierFailed = false
    private var committed: CronOwnerIdentity? = null

    /** Called before the SDK creates a transcript or persists its reservation. */
    suspend fun begin(proposed: CronOwnerCandidate): Boolean = withContext(NonCancellable) {
        val mode = runCatching { validateCronOwnerCandidate(proposed) }.getOrNull() ?: return@withContext false
        val id = canonicalSessionId(proposed.sessionId)
        val waiting = synchronized(lock) {
            if (callerFinished || nativeFinished || ended) return@withContext false
            candidate?.let { existing ->
                if (existing != proposed) {
                    if (beginAccepted) SessionAdmissionGate.invalidateCronOwners()
                    return@withContext false
                }
                if (beginInFlight) return@synchronized beginResult
                return@withContext beginAccepted
            }
            val reservation = SessionAdmissionGate.beginCronReservation(id, proposed.scopeKey, mode, proposed.transcriptPath)
                ?: return@withContext false
            reservedId = id
            token = reservation
            candidate = proposed
            beginInFlight = true
            beginResult = CompletableDeferred()
            null
        }
        if (waiting != null) return@withContext waiting.await()
        val accepted = try {
            validateCandidate(proposed, checkNotNull(token))
        } catch (_: Throwable) {
            SessionAdmissionGate.invalidateCronOwners()
            false
        }
        synchronized(lock) {
            beginInFlight = false
            val reservation = checkNotNull(token)
            val admitted = accepted && !callerFinished && !nativeFinished &&
                SessionAdmissionGate.permitsNewOwner(id, reservation)
            beginAccepted = admitted
            if (!admitted) {
                ended = true
                // A rejected begin cannot have created an SDK source or ledger entry.
                SessionAdmissionGate.finishCronReservation(reservation, committed = true)
            }
            beginResult?.complete(admitted)
            admitted
        }
    }

    suspend fun confirm(identity: CronOwnerIdentity): Boolean = withContext(NonCancellable) {
        val waiting = synchronized(lock) {
            if (callerFinished && !callbackInFlight) return@withContext false
            val proposed = candidate
            if (!beginAccepted || reservedId == null || canonicalSessionId(identity.sessionId) != reservedId ||
                proposed == null || proposed.mode != identity.mode || proposed.scopeKey != identity.scopeKey ||
                (proposed.transcriptPath != null && proposed.transcriptPath != identity.transcriptPath)) {
                // A mismatched callback may follow a durable SDK write.
                SessionAdmissionGate.invalidateCronOwners()
                return@withContext false
            }
            committed?.let { saved ->
                if (saved != identity) SessionAdmissionGate.invalidateCronOwners()
                return@withContext saved == identity
            }
            if (callbackInFlight) {
                if (callbackIdentity != identity) {
                    SessionAdmissionGate.invalidateCronOwners()
                    return@withContext false
                }
                return@synchronized callbackResult
            }
            if (callerFinished || ended) return@withContext false
            callbackInFlight = true
            callbackIdentity = identity
            callbackResult = CompletableDeferred()
            null
        }
        if (waiting != null) return@withContext waiting.await()
        val accepted = try { persist(identity, checkNotNull(token)) } catch (_: Throwable) { false }
        synchronized(lock) {
            callbackInFlight = false
            val reservation = checkNotNull(token)
            val committedNow = accepted && !callerFinished && SessionAdmissionGate.permitsNewOwner(checkNotNull(reservedId), reservation)
            if (committedNow) committed = identity
            if (committedNow || nativeFinished) {
                ended = true
                SessionAdmissionGate.finishCronReservation(reservation, committedNow)
            }
            callbackResult?.complete(committedNow)
            committedNow
        }
    }

    fun finishCaller() = synchronized(lock) {
        callerFinished = true
    }

    suspend fun settleAfterCaller(barrier: suspend () -> Unit) = withContext(NonCancellable) {
        finishCaller()
        try {
            barrier()
        } catch (failure: Throwable) {
            synchronized(lock) {
                if (!barrierFailed) {
                    barrierFailed = true
                    SessionAdmissionGate.noteBarrierFailure()
                }
            }
            throw failure
        }
        synchronized(lock) {
            if (barrierFailed) {
                barrierFailed = false
                SessionAdmissionGate.resolveBarrierFailure()
            }
        }
        finishNative()
    }

    /** Invoke only after the independently-owned native admission driver settles. */
    fun finishNative() = synchronized(lock) {
        nativeFinished = true
        callerFinished = true
        if (!ended && !callbackInFlight && !beginInFlight) {
            ended = true
            token?.let { SessionAdmissionGate.finishCronReservation(it, committed = false) }
        }
    }
}
