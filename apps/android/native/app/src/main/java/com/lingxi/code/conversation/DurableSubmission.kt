package com.lingxi.code.conversation

import kotlinx.coroutines.CancellationException

/** Stop and the native call compete for one admission decision. */
internal class NativeSubmissionGate(private val onRejected: () -> Unit) {
    private enum class State { Pending, Entered, Rejected }
    private var state = State.Pending

    @Synchronized fun enter(): Boolean {
        if (state != State.Pending) return false
        state = State.Entered
        return true
    }

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
