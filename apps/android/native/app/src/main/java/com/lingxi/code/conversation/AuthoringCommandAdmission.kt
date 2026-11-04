package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.model.canonicalSessionId

internal class SideChatNotAdmittedException(message: String) : IllegalStateException(message)

internal fun requireSideChatParentAdmission(ready: Boolean) {
    if (!ready) throw SideChatNotAdmittedException("The authoring conversation is still preparing")
}

/** A short selection fence refuses new work while the retained source is checked. */
internal class AuthoringSelectionFence {
    private enum class State { Idle, Selecting, Mutating, Uncertain }
    private val state = java.util.concurrent.atomic.AtomicReference(State.Idle)
    val ready: Boolean get() = state.get() == State.Idle
    fun reserve(): Boolean = state.compareAndSet(State.Idle, State.Selecting)
    fun release() { state.compareAndSet(State.Selecting, State.Idle) }

    /** Keep the fence until the native result arrives even if the caller is cancelled. */
    suspend fun <T> mutateWorkspace(action: suspend () -> T): T {
        check(state.compareAndSet(State.Idle, State.Mutating)) { "The authoring workspace is busy" }
        return kotlinx.coroutines.withContext(kotlinx.coroutines.NonCancellable) {
            try {
                action().also { check(state.compareAndSet(State.Mutating, State.Idle)) }
            } catch (failure: Throwable) {
                state.set(State.Uncertain)
                throw failure
            }
        }
    }
}

/** Keep the reservation only when a fresh native snapshot still matches the selected owner. */
internal suspend fun <T : Any> reserveValidatedAuthoringSelection(
    reserve: () -> Boolean,
    readWorkspace: suspend () -> T,
    matches: (T) -> Boolean,
    release: () -> Unit,
): T? {
    if (!reserve()) return null
    try {
        val workspace = readWorkspace()
        if (matches(workspace)) return workspace
        release()
        return null
    } catch (failure: Throwable) {
        release()
        throw failure
    }
}

/** The first native activation permanently fixes the source's authoring UUID. */
internal class AuthoringSessionBoundary(initialSessionId: String?) {
    @Volatile private var sessionId: String? = initialSessionId?.let(::canonicalSessionId)

    @Synchronized fun observe(session: String): Boolean {
        val id = canonicalSessionId(session)
        if (sessionId == null) sessionId = id
        return sessionId == id
    }

    fun permits(session: String?): Boolean = session != null && sessionId != null && canonicalSessionId(session) == sessionId
}

/** Only observation and explicit stop commands may cross a pending authoring gate. */
internal fun maySubmitWhileAuthoringPending(command: ClientCommand): Boolean = when (command) {
    ClientCommand.ListModels, ClientCommand.ListSessionAgents, ClientCommand.GetConversationControls,
    is ClientCommand.RefreshListings, is ClientCommand.ListSessions, is ClientCommand.TaskList,
    is ClientCommand.TaskOutput, is ClientCommand.LoadSessionAgentTranscript,
    is ClientCommand.Cancel, is ClientCommand.TaskStop, is ClientCommand.PauseTurn,
    is ClientCommand.UiDetach -> true
    else -> false
}
