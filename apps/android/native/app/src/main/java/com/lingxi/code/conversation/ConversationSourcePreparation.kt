package com.lingxi.code.conversation

import com.lingxi.code.model.canonicalSessionId
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext

/** Follow-ups may reach native only after the matching first turn starts. */
internal class NativeTurnAdmission {
    enum class Phase { Pending, Accepted, Failed }
    data class Status(val turnId: Long, val phase: Phase)
    private val mutableStatus = kotlinx.coroutines.flow.MutableStateFlow<Status?>(null)
    val status: kotlinx.coroutines.flow.StateFlow<Status?> get() = mutableStatus

    @Synchronized fun start(turnId: Long) { mutableStatus.value = Status(turnId, Phase.Pending) }
    @Synchronized fun confirm(turnId: Long?) {
        val current = mutableStatus.value
        if (turnId != null && current?.turnId == turnId && current.phase == Phase.Pending) {
            mutableStatus.value = current.copy(phase = Phase.Accepted)
        }
    }
    @Synchronized fun clear() {
        val current = mutableStatus.value
        if (current?.phase == Phase.Pending) mutableStatus.value = current.copy(phase = Phase.Failed)
    }
    @Synchronized fun blocksFollowUp(streaming: Boolean): Boolean = streaming && mutableStatus.value?.phase == Phase.Pending
}

/** Preserve a previously confirmed source only for the same session and owner generation. */
internal fun reconcileConversationSnapshot(
    snapshot: ConversationBackgroundSnapshot,
    existing: Collection<ConversationBackgroundSnapshot?>,
): ConversationBackgroundSnapshot {
    val spec = snapshot.recoverySpec ?: return snapshot
    val paths = existing.filterNotNull().filter { previous ->
        previous.recoverySpec?.ownerKey == spec.ownerKey &&
            canonicalSessionId(previous.sessionId) == canonicalSessionId(snapshot.sessionId)
    }.mapNotNull { it.recoverySpec?.transcriptPath?.takeIf(String::isNotBlank) }.toSet()
    check(paths.size <= 1) { "Conversation recovery sources conflict" }
    val known = paths.singleOrNull()
    val incoming = spec.transcriptPath?.takeIf(String::isNotBlank)
    check(known == null || incoming == null || known == incoming) { "Conversation recovery source changed" }
    return if (incoming == null && known != null) snapshot.copy(recoverySpec = spec.copy(transcriptPath = known)) else snapshot
}

/** A stale notification cannot replace another turn of the same live owner. */
internal fun reconcileConversationAction(
    snapshot: ConversationBackgroundSnapshot,
    existing: Map<String, ConversationBackgroundSnapshot?>,
): ConversationBackgroundSnapshot {
    val spec = snapshot.recoverySpec ?: return snapshot
    val related = existing.filterValues { previous -> previous != null && previous.recoverySpec?.ownerKey == spec.ownerKey &&
        canonicalSessionId(previous.sessionId) == canonicalSessionId(snapshot.sessionId) }
    val turns = related.values.mapNotNull { it?.turnId }.toSet()
    check(snapshot.turnId == null || turns.isEmpty() || turns == setOf(snapshot.turnId)) {
        "Conversation notification belongs to another turn"
    }
    val matching = related.filterValues { it?.turnId == snapshot.turnId }
    val uiOwners = matching.filterKeys { it != spec.ownerKey }
    val authoritative = if (uiOwners.isNotEmpty()) uiOwners else matching
    val taskSets = authoritative.values.filterNotNull().map { it.activeTaskIds }.toSet()
    check(taskSets.size <= 1) { "Conversation background tasks conflict" }
    val resolved = reconcileConversationSnapshot(snapshot, existing.values)
    return taskSets.singleOrNull()?.let { resolved.copy(activeTaskIds = it) } ?: resolved
}

/** Keep all leases for one source aligned before publishing a newly admitted turn. */
internal fun submissionOwnerSnapshots(
    owner: String,
    snapshot: ConversationBackgroundSnapshot,
    existing: Map<String, ConversationBackgroundSnapshot?>,
): Map<String, ConversationBackgroundSnapshot> {
    val spec = snapshot.recoverySpec ?: error("Conversation recovery source is unavailable")
    val current = checkNotNull(existing[owner]) { "Conversation background owner is unavailable" }
    check(!spec.transcriptPath.isNullOrBlank()) { "Conversation transcript is unavailable" }
    check(current.recoverySpec?.ownerKey == spec.ownerKey &&
        canonicalSessionId(current.sessionId) == canonicalSessionId(snapshot.sessionId) &&
        current.turnId == snapshot.turnId) { "Conversation turn changed before submission" }
    val confirmed = reconcileConversationSnapshot(snapshot, existing.values)
    return existing.mapNotNull { (key, previous) ->
        if (previous == null || previous.recoverySpec?.ownerKey != spec.ownerKey ||
            canonicalSessionId(previous.sessionId) != canonicalSessionId(snapshot.sessionId)) null
        else key to if (key == owner) confirmed else confirmed.copy(activeTaskIds = confirmed.activeTaskIds + previous.activeTaskIds)
    }.toMap()
}

/** Flush after staging releases its lock, then validate the still-current owner. */
internal fun flushConversationRecovery(stage: () -> () -> Boolean, revalidate: () -> Unit) {
    val flush = stage()
    check(flush()) { "Unable to persist conversation recovery source" }
    revalidate()
}

/** Persist the confirmed exact source before the native turn may be submitted. */
internal suspend fun prepareTurnRecovery(
    sessionId: String,
    turnId: Long,
    awaitTranscriptPath: suspend () -> String,
    isCurrent: () -> Boolean,
    snapshot: () -> ConversationBackgroundSnapshot?,
    persist: suspend (ConversationBackgroundSnapshot) -> Unit,
) {
    val path = awaitTranscriptPath().takeIf(String::isNotBlank) ?: error("Conversation transcript is unavailable")
    if (!isCurrent()) throw kotlinx.coroutines.CancellationException("Conversation turn changed")
    val current = checkNotNull(snapshot()) { "Conversation background owner is unavailable" }
    check(canonicalSessionId(current.sessionId) == canonicalSessionId(sessionId) && current.turnId == turnId &&
        current.recoverySpec?.transcriptPath == path) { "Conversation recovery source changed" }
    persist(current)
    if (!isCurrent()) throw kotlinx.coroutines.CancellationException("Conversation turn changed")
}

/** Native construction can hash a rootfs and cannot be interrupted halfway.
 * Keep its returned handle until the caller can adopt it, including cancellation.
 */
internal suspend fun prepareConversationSource(
    borrowed: ConversationSource? = null,
    dispatcher: CoroutineDispatcher = Dispatchers.IO,
    create: () -> ConversationSource,
): ConversationSource {
    var prepared: ConversationSource? = null
    try {
        withContext(dispatcher + NonCancellable) { prepared = create() }
        currentCoroutineContext().ensureActive()
        return checkNotNull(prepared).also { prepared = null }
    } finally {
        prepared?.takeIf { it !== borrowed }?.let { abandoned ->
            withContext(dispatcher + NonCancellable) { abandoned.close() }
        }
    }
}
