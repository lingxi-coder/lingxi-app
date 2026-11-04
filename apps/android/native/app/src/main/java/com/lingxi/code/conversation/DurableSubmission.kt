package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientException
import com.lingxi.code.model.canonicalSessionId
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeoutOrNull

/** Stop and the native call compete for one admission decision. */
internal class NativeSubmissionGate(private val onRejected: () -> Unit) {
    private enum class State { Pending, Entered, Rejected }
    private var state = State.Pending

    @Synchronized fun enter(): Boolean {
        if (state != State.Pending) return false
        state = State.Entered
        return true
    }

    @Synchronized fun hasEntered(): Boolean = state == State.Entered

    fun reject(): Boolean = reject(allowEntered = false)

    /** An explicit host rejection can settle even after the call was entered. */
    fun rejectKnown(): Boolean = reject(allowEntered = true)

    private fun reject(allowEntered: Boolean): Boolean {
        val rejected = synchronized(this) {
            if (state == State.Rejected || (!allowEntered && state != State.Pending)) false
            else { state = State.Rejected; true }
        }
        if (rejected) onRejected()
        return rejected
    }
}

/** The direct result of one Send, tied to its original engine owner and turn. */
data class NativePromptIdentity(val sessionId: String, val sideId: String? = null) {
    fun canonical(): NativePromptIdentity? {
        val session = canonicalSessionId(sessionId)
        if (session.isBlank() || session == "new" || sideId?.isBlank() == true) return null
        return copy(sessionId = session)
    }
}

internal class NativePromptSubmission(
    private val owner: Any,
    private val turnId: Long,
    private val gate: NativeSubmissionGate,
) {
    private val result = CompletableDeferred<Result<Unit>>()
    private var identity: NativePromptIdentity? = null

    @Synchronized fun bindIdentity(owner: Any, identity: NativePromptIdentity, turnId: Long): Boolean {
        val confirmed = identity.canonical() ?: return false
        if (this.owner !== owner || this.turnId != turnId) return false
        if (this.identity != null && this.identity != confirmed) return false
        this.identity = confirmed
        return true
    }

    fun record(result: Result<Unit>) { this.result.complete(result) }

    internal suspend fun settleAfterStop(
        owner: Any,
        identity: NativePromptIdentity?,
        turnId: Long?,
        timeoutMs: Long,
        clearCheckpoint: suspend (NativePromptIdentity) -> Boolean,
    ): Boolean {
        val originalIdentity = synchronized(this) { this.identity } ?: return false
        if (this.owner !== owner || originalIdentity != identity?.canonical() ||
            this.turnId != turnId || !gate.hasEntered()) return false
        val outcome = withTimeoutOrNull(timeoutMs) { result.await() } ?: return false
        val failure = outcome.exceptionOrNull()
        if (failure !is ClientException.Rejected || failure.message != "turn_cancelled_before_admission") return false
        check(clearCheckpoint(originalIdentity)) { "Unable to clear stopped conversation checkpoint" }
        return gate.rejectKnown()
    }
}

/** Record the original Send result before its collector can be cancelled. */
internal suspend fun submitWithReceipt(
    submit: suspend () -> Unit,
    record: (Result<Unit>) -> Unit,
) {
    try {
        submit()
        record(Result.success(Unit))
    } catch (failure: Throwable) {
        record(Result.failure(failure))
        throw failure
    }
}

/** A successful Cancel is necessary, but not sufficient, to settle a Send. */
internal suspend fun stopWithSubmissionReceipt(
    stop: suspend () -> Unit,
    submission: NativePromptSubmission?,
    owner: Any,
    identity: NativePromptIdentity?,
    turnId: Long?,
    timeoutMs: Long,
    clearCheckpoint: suspend (NativePromptIdentity) -> Boolean,
): Boolean {
    stop()
    return submission?.settleAfterStop(owner, identity, turnId, timeoutMs, clearCheckpoint) ?: false
}

/** Only failures before the native submission boundary are known not to be admitted. */
internal suspend fun submitDurableTurn(
    prepare: suspend () -> Unit,
    submit: suspend () -> Unit,
    onNotAdmitted: () -> Unit,
) {
    try {
        prepare()
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (failure: Throwable) {
        onNotAdmitted()
        throw failure
    }
    submit()
}
