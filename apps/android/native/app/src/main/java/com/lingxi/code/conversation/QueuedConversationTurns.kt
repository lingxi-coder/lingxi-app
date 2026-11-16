package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ClientException
import com.lingxi.code.bindings.client.TurnRecoverySnapshotDto
import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import com.lingxi.code.model.canonicalSessionId
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.lang.ref.ReferenceQueue
import java.lang.ref.WeakReference

/** Native callbacks are receipts by identity, even when DTO contents match. */
private class EventReceipts<V> {
    private class Key(event: ClientEvent, queue: ReferenceQueue<ClientEvent>? = null) : WeakReference<ClientEvent>(event, queue) {
        private val identity = System.identityHashCode(event)
        override fun hashCode(): Int = identity
        override fun equals(other: Any?): Boolean = this === other ||
            (other is Key && get()?.let { it === other.get() } == true)
    }
    private val queue = ReferenceQueue<ClientEvent>()
    private val entries = mutableMapOf<Key, V>()
    private fun prune() { while (true) entries.remove(queue.poll() ?: return) }
    @Synchronized operator fun get(event: ClientEvent): V? { prune(); return entries[Key(event)] }
    @Synchronized operator fun set(event: ClientEvent, value: V) { prune(); entries[Key(event, queue)] = value }
}

/** Host facts, not a second prompt/image store or an inference-message cache. */
internal data class QueuedTurnAdmission(
    val turnId: Long,
    val admitted: Boolean,
    val queued: Boolean,
    val settled: Boolean,
    val executorActive: Boolean,
    val recovery: TurnRecoverySnapshotDto?,
)

internal data class QueuedTurnRecovery(
    val record: DurableConversationTurnRecord?,
    val executorActive: Boolean = false,
    val repairPendingId: Long? = null,
    val rejectedIds: List<Long> = emptyList(),
)

/** One source serializes admission, FIFO publication and authoritative recovery. */
internal class QueuedConversationTurns(
    private val store: DurableConversationTurnClientStore,
    private val currentSession: () -> String?,
    private val query: suspend (String, List<Long>) -> List<QueuedTurnAdmission>,
) {
    private val mutex = Mutex()
    private val inFlight = mutableSetOf<Pair<String, Long>>()
    private val rejectionHandlers = mutableMapOf<Pair<String, Long>, () -> Unit>()
    // A relay may run ahead of its UI collector. A receipt stays alive exactly
    // as long as the buffered event; terminal retirement cannot erase it early.
    private data class OwnerReceipt(val sessionId: String, val turnId: Long)
    private val executorReceipts = EventReceipts<OwnerReceipt>()
    private val ownerReceipts = EventReceipts<OwnerReceipt>()
    private val rejectionReceipts = EventReceipts<Boolean>()
    private val repairReceipts = EventReceipts<Boolean>()
    private val attachSnapshots = EventReceipts<Boolean>()
    private val rawOwners = EventReceipts<Long>()
    @Volatile private var attachingOwner: Long? = null
    @Volatile private var ingressOwner: Long? = null
    @Volatile private var blockedExecutor: Long? = null

    fun executorFor(event: ClientEvent): Long? = executorReceipts[event]?.turnId
    fun ownerFor(event: ClientEvent): Long? = ownerReceipts[event]?.turnId
    fun sessionFor(event: ClientEvent): String? = ownerReceipts[event]?.sessionId
    fun isRejection(event: ClientEvent): Boolean = rejectionReceipts[event] == true
    fun requiresTranscriptRepair(event: ClientEvent): Boolean = repairReceipts[event] == true
    fun rawOwnerFor(event: ClientEvent): Long? = rawOwners[event]
    fun isAttachSnapshot(event: ClientEvent): Boolean = attachSnapshots[event] == true
    fun beginAttach(turnId: Long) { attachingOwner = turnId }
    fun endAttach(turnId: Long) { if (attachingOwner == turnId) attachingOwner = null }
    fun noteIncoming(event: ClientEvent) {
        if (event is ClientEvent.TurnRecoveryState && event.snapshot.turnId.toLong() == attachingOwner) attachSnapshots[event] = true
        when (event) {
            is ClientEvent.TurnRecoveryState -> if (attachSnapshots[event] != true && event.snapshot.state == TurnRecoveryStateDto.RUNNING) ingressOwner = event.snapshot.turnId.toLong()
            is ClientEvent.TurnStarted -> event.turnId?.toLong()?.let { ingressOwner = it }
            is ClientEvent.SessionStarted, is ClientEvent.SessionResumed -> ingressOwner = null
            else -> ingressOwner?.let { rawOwners[event] = it }
        }
    }

    fun rejectionNotice(recovery: QueuedTurnRecovery): ClientEvent? = if (recovery.rejectedIds.isEmpty()) null else
        ClientEvent.SystemNotice("The queued prompt was not delivered. Its input can be sent again.".toClientTextDto(), true)
            .also { rejectionReceipts[it] = true }

    fun blocksNewTurn(): Boolean {
        val session = currentSession()?.let(::canonicalSessionId) ?: return false
        val state = store.state(session)
        return blockedExecutor != null || state.queuedTurnIds.isNotEmpty() || state.active?.queuedOwner == true
    }

    private fun checkSession(session: String) {
        check(currentSession()?.let(::canonicalSessionId) == session) { "Queued conversation owner changed" }
    }

    suspend fun submit(
        command: ClientCommand.SendPrompt,
        ownerTurnId: Long,
        tryBeginNative: () -> Boolean,
        onNotAdmitted: () -> Unit,
        send: suspend () -> Unit,
    ) {
        val session = checkNotNull(currentSession()).let(::canonicalSessionId)
        val id = checkNotNull(command.turnId).toLong()
        val key = session to id
        var entered = false
        try {
            mutex.withLock {
                checkSession(session)
                store.enqueue(session, ownerTurnId, id)
                inFlight += key
                rejectionHandlers[key] = onNotAdmitted
            }
            if (!tryBeginNative()) throw CancellationException("Queued prompt cancelled before submission")
            entered = true
            send()
        } catch (failure: Throwable) {
            val rejected = !entered || (failure is ClientException.Rejected && failure.message == "turn_cancelled_before_admission")
            if (rejected) {
                val reject = withContext(NonCancellable) { mutex.withLock {
                    try {
                        if (currentSession()?.let(::canonicalSessionId) == session) store.removeQueued(session, id)
                    } catch (cleanup: Exception) { failure.addSuppressed(cleanup) }
                    rejectionHandlers.remove(key) ?: onNotAdmitted.takeIf { !entered }
                }
                }
                reject?.invoke()
            }
            // Ambiguous native failures keep their ID. Recovery queries the
            // original admission after its receipt, and never resends a prompt.
            throw failure
        } finally {
            withContext(NonCancellable) { mutex.withLock { inFlight -= key } }
        }
    }

    private suspend fun facts(session: String, state: DurableConversationTurnState): List<QueuedTurnAdmission> {
        val ids = (listOfNotNull(state.active?.turnId) + state.queuedTurnIds).distinct()
        if (ids.isEmpty()) return emptyList()
        val facts = query(session, ids)
        checkSession(session)
        check(facts.map { it.turnId }.toSet() == ids.toSet() && facts.size == ids.size) { "Incomplete queued admission proof" }
        check(facts.all { it.recovery == null || canonicalSessionId(it.recovery.sessionId) == session }) { "Queued admission belongs to another session" }
        return facts
    }

    private suspend fun recoverLocked(session: String, desiredOwner: Long? = null): QueuedTurnRecovery {
        checkSession(session)
        val before = store.state(session)
        if (before.queuedTurnIds.isEmpty() && before.active?.queuedOwner != true) return QueuedTurnRecovery(before.active)
        val facts = facts(session, before)
        val rejected = mutableListOf<Long>()
        for (fact in facts) {
            if (fact.admitted) rejectionHandlers.remove(session to fact.turnId)
            if (fact.turnId in before.queuedTurnIds && (session to fact.turnId) !in inFlight &&
                (!fact.admitted || (fact.settled && fact.recovery == null))) {
                store.removeQueued(session, fact.turnId)
                if (!fact.admitted) {
                    rejected += fact.turnId
                    rejectionHandlers.remove(session to fact.turnId)?.invoke()
                }
            }
        }
        val previous = before.active?.let { active -> facts.single { it.turnId == active.turnId } }
        // Do not replace a real executor or a recoverable paused predecessor.
        val mayReplace = previous == null || previous.settled || !previous.admitted || previous.queued
        val remaining = store.state(session)
        val candidates = facts.filter { fact ->
            fact.admitted && fact.recovery != null &&
                (fact.turnId in remaining.queuedTurnIds || (remaining.active?.queuedOwner == true && fact.turnId == remaining.active.turnId)) &&
                (desiredOwner == null || fact.recovery.turnId.toLong() == desiredOwner)
        }
        val chosen = candidates.firstOrNull { it.executorActive }
            ?: candidates.firstOrNull { !it.queued || it.settled }
        if (chosen != null) {
            val owner = chosen.recovery!!.turnId.toLong()
            if (remaining.active?.turnId == owner || mayReplace) {
                store.adoptQueuedOwner(session, chosen.turnId, owner, deferPrevious = previous?.queued == true)
                return QueuedTurnRecovery(store.load(), chosen.executorActive, rejectedIds = rejected)
            }
        }
        if (previous != null && (previous.settled || !previous.admitted)) store.clear(session, previous.turnId)
        val after = store.state(session)
        val activeFact = after.active?.let { active -> facts.firstOrNull { it.turnId == active.turnId } }
        val pending = if (after.active == null) after.queuedTurnIds.firstOrNull { id ->
            facts.any { it.turnId == id && it.admitted && it.queued && !it.settled }
        } else null
        return QueuedTurnRecovery(after.active, activeFact?.executorActive == true, pending, rejected)
    }

    suspend fun recover(sessionId: String): QueuedTurnRecovery = mutex.withLock {
        recoverLocked(canonicalSessionId(sessionId))
    }

    /** Runs once in the source's lossless FIFO, before any output is published. */
    suspend fun beforeEvent(event: ClientEvent): Boolean = try {
        prepareEvent(event)
    } catch (failure: Exception) {
        if (event is ClientEvent.TurnRecoveryState && event.snapshot.state == TurnRecoveryStateDto.RUNNING) {
            blockedExecutor = event.snapshot.turnId.toLong()
        }
        throw failure
    }

    private suspend fun prepareEvent(event: ClientEvent): Boolean = mutex.withLock {
        val session = currentSession()?.let(::canonicalSessionId) ?: return@withLock true
        if (blockedExecutor != null && event !is ClientEvent.SystemNotice && event !is ClientEvent.TurnRecoveryState &&
            event !is ClientEvent.TurnStarted && event !is ClientEvent.SessionStarted && event !is ClientEvent.SessionResumed &&
            (event is ClientEvent.TurnEventReplay || clientEventToReply(event) != null)) return@withLock false
        when (event) {
            is ClientEvent.SessionStarted, is ClientEvent.SessionResumed -> {
                store.selectSession(session)
                blockedExecutor = null
            }
            is ClientEvent.TurnRecoveryState -> {
                val snapshot = event.snapshot
                if (canonicalSessionId(snapshot.sessionId) != session) return@withLock true
                val id = snapshot.turnId.toLong()
                if (store.load()?.let { it.queuedOwner && it.turnId == id } == true) ownerReceipts[event] = OwnerReceipt(session, id)
                if (snapshot.state == TurnRecoveryStateDto.RUNNING) {
                    val state = store.state(session)
                    if (state.queuedTurnIds.isNotEmpty() || state.active?.queuedOwner == true) {
                        val proof = recoverLocked(session, id)
                        if (proof.record?.turnId == id && proof.record.queuedOwner) {
                            ownerReceipts[event] = OwnerReceipt(session, id)
                            if (attachSnapshots[event] != true) {
                                executorReceipts[event] = OwnerReceipt(session, id)
                                blockedExecutor = null
                            }
                        } else if (state.active?.turnId != id) {
                            blockedExecutor = id
                            return@withLock false
                        }
                    }
                } else if (snapshot.state in terminalStates) {
                    if (blockedExecutor == id) {
                        val proof = recoverLocked(session, id)
                        if (proof.record?.turnId == id) {
                            ownerReceipts[event] = OwnerReceipt(session, id)
                            repairReceipts[event] = true
                            blockedExecutor = null
                        }
                    }
                    val before = store.state(session)
                    if (before.queuedTurnIds.isNotEmpty()) {
                        // One terminal owner can prove all coalesced members;
                        // unrelated later pending inputs retain their order.
                        for (fact in facts(session, before)) {
                            if ((session to fact.turnId) !in inFlight && fact.settled &&
                                (fact.recovery == null || fact.recovery.turnId.toLong() == id)) store.removeQueued(session, fact.turnId)
                        }
                    }
                    store.clear(session, id)
                }
            }
            is ClientEvent.TurnStarted -> event.turnId?.toLong()?.let { id ->
                if (blockedExecutor == id) recoverLocked(session, id)
                val active = store.load()
                if (active?.queuedOwner == true && active.turnId == id) {
                    executorReceipts[event] = OwnerReceipt(session, id)
                    ownerReceipts[event] = OwnerReceipt(session, id)
                    blockedExecutor = null
                }
            }
            is ClientEvent.UserTranscriptRowIdentity -> {
                if (event.rowToken.startsWith("client-turn:") && store.state(session).queuedTurnIds.isNotEmpty()) {
                    // ACK facts distinguish a mid-turn append from an executor
                    // merely announcing its own persisted input row.
                    recoverLocked(session)
                }
            }
            else -> Unit
        }
        true
    }

    private companion object {
        val terminalStates = setOf(TurnRecoveryStateDto.COMPLETED, TurnRecoveryStateDto.FAILED, TurnRecoveryStateDto.CANCELLED)
    }
}
