package com.lingxi.code.conversation

import com.lingxi.code.model.SessionMode

data class SessionModeSnapshot(val sessionId: String, val mode: SessionMode, val pending: SessionMode?, val revision: ULong)

/** Applying a pending request may reuse its revision, but cannot be undone by a delayed copy. */
internal fun acceptsSessionModeRevision(previous: ULong?, previousPending: SessionMode?, incoming: ULong, incomingPending: SessionMode?): Boolean =
    previous == null || incoming > previous || (incoming == previous && (previousPending != null || incomingPending == null))

/** Holds recent session snapshots even when events precede a resume or UI rebind. */
internal class SessionModeSnapshotCache {
    private val snapshots = linkedMapOf<String, SessionModeSnapshot>()

    @Synchronized
    fun record(next: SessionModeSnapshot): SessionModeSnapshot {
        val previous = snapshots[next.sessionId]
        if (previous == null || acceptsSessionModeRevision(previous.revision, previous.pending, next.revision, next.pending)) {
            snapshots.remove(next.sessionId)
            snapshots[next.sessionId] = next
            while (snapshots.size > 16) snapshots.remove(snapshots.keys.first())
        }
        return snapshots.getValue(next.sessionId)
    }

    @Synchronized
    fun forSession(sessionId: String): SessionModeSnapshot? = snapshots[sessionId]
}
