package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.PermissionRequest
import com.lingxi.code.bindings.client.ClientException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.FlowCollector
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.launch

/** Linearizes Stop with admission of one prompt into the Host. */
class TurnSubmissionAdmission {
    enum class Outcome { Admitted, NotAdmitted, Unknown }
    data class Completion(val sessionId: String, val outcome: Outcome)
    private val lock = Any()
    private var cancelled = false
    private var admittedSession: String? = null
    private val completed = CompletableDeferred<Outcome>()
    @Volatile var outcome: Outcome? = null
        private set

    fun begin(sessionId: String): Boolean = synchronized(lock) {
        if (cancelled || sessionId.isBlank()) false else {
            admittedSession = sessionId
            true
        }
    }

    fun finished(error: Throwable? = null) {
        val result = when (error) {
            null -> Outcome.Admitted
            is ClientException.SubmissionNotAdmitted -> Outcome.NotAdmitted
            else -> Outcome.Unknown
        }
        synchronized(lock) {
            if (!completed.isCompleted) {
                outcome = result
                completed.complete(result)
            }
        }
    }

    suspend fun cancelAndAwait(): Completion? {
        val session = synchronized(lock) {
            cancelled = true
            admittedSession
        }
        return session?.let { Completion(it, completed.await()) }
    }
}

/** A replay boundary is complete only after every subscribed UI consumes it. */
class AttachConsumptionBarrier internal constructor(
    participants: Set<Long>,
    private val completion: CompletableDeferred<Boolean>,
) {
    private val waiting = participants.toMutableSet()
    private var applied = false
    private var failed = false

    @Synchronized fun includes(id: Long) = waiting.contains(id)
    @Synchronized fun finished() = completion.isCompleted

    @Synchronized fun acknowledge(id: Long, result: Boolean?) {
        if (!waiting.remove(id)) return
        if (result == true) applied = true
        if (result == false) failed = true
        if (waiting.isEmpty()) completion.complete(applied && !failed)
    }

    @Synchronized fun abandon(id: Long) {
        if (waiting.remove(id)) completion.complete(false)
    }
}

class RecoveryRetirementBarrier internal constructor(
    participants: Set<Long>,
    private val onReady: () -> Unit,
) {
    private val waiting = participants.toMutableSet()

    fun acknowledge(id: Long) {
        val ready = synchronized(this) { waiting.remove(id) && waiting.isEmpty() }
        if (ready) onReady()
    }

    @Synchronized fun finished() = waiting.isEmpty()

    fun force() {
        val ready = synchronized(this) { if (waiting.isEmpty()) false else { waiting.clear(); true } }
        if (ready) onReady()
    }
}

internal class AttachConsumerRegistry {
    private var nextId = 0L
    private val observers = mutableMapOf<Long, Boolean>()
    private val barriers = mutableSetOf<AttachConsumptionBarrier>()
    private val retirements = mutableSetOf<RecoveryRetirementBarrier>()
    private val count = MutableStateFlow(0)

    suspend fun awaitSubscriber() { count.first { it > 0 } }

    @Synchronized private fun register(ui: Boolean): Long {
        val id = ++nextId
        observers[id] = ui
        count.value = observers.count { it.value }
        return id
    }

    private fun unregister(id: Long) {
        val (attach, retire) = synchronized(this) {
            observers.remove(id)
            count.value = observers.count { it.value }
            barriers.toList() to retirements.toList()
        }
        attach.forEach { it.abandon(id) }
        retire.forEach { it.acknowledge(id) }
        synchronized(this) {
            barriers.removeAll { it.finished() }
            retirements.removeAll { it.finished() }
        }
    }

    @Synchronized fun barrier(completion: CompletableDeferred<Boolean>): AttachConsumptionBarrier {
        val ui = observers.filterValues { it }.keys.toSet()
        val barrier = AttachConsumptionBarrier(ui, completion)
        if (ui.isEmpty()) completion.complete(false) else barriers.add(barrier)
        return barrier
    }

    fun retirement(onReady: () -> Unit): RecoveryRetirementBarrier? {
        val ticket = synchronized(this) {
            if (observers.isEmpty()) null else RecoveryRetirementBarrier(observers.keys.toSet(), onReady).also { retirements.add(it) }
        }
        if (ticket == null) onReady()
        return ticket
    }

    @Synchronized fun forget(barrier: AttachConsumptionBarrier?) {
        if (barrier != null) barriers.remove(barrier)
    }

    fun track(events: SharedFlow<ConversationDelivery>, ui: Boolean = true,
        onSubscribed: suspend FlowCollector<ConversationDelivery>.() -> Unit = {}): Flow<ConversationDelivery> = flow {
        var id = 0L
        try {
            events.onSubscription { id = register(ui); onSubscribed() }.collect { delivery ->
                val barrier = when (delivery) {
                    is ConversationDelivery.AttachReplayEnded -> delivery.barrier
                    is ConversationDelivery.ReconciliationNotFound -> delivery.barrier
                    else -> null
                }
                if (barrier != null && barrier.includes(id)) {
                    val acknowledgment = CompletableDeferred<Boolean?>()
                    when (delivery) {
                        is ConversationDelivery.AttachReplayEnded -> emit(delivery.copy(acknowledged = acknowledgment))
                        is ConversationDelivery.ReconciliationNotFound -> emit(delivery.copy(acknowledged = acknowledgment))
                        else -> error("Missing recovery boundary")
                    }
                    barrier.acknowledge(id, if (acknowledgment.isCompleted) acknowledgment.await() else false)
                    if (barrier.finished()) forget(barrier)
                } else if (delivery is ConversationDelivery.RecoveryInterestEnded) {
                    // This marker is internal; every observer first consumes
                    // earlier items, then releases its claim on the lease.
                    delivery.barrier.acknowledge(id)
                    if (delivery.barrier.finished()) synchronized(this@AttachConsumerRegistry) { retirements.remove(delivery.barrier) }
                } else emit(delivery)
            }
        } finally {
            if (id != 0L) unregister(id)
        }
    }
}

/** Serializes suspending native registrations in the order requests enter. */
internal class OrderedSuspendQueue(scope: CoroutineScope) {
    private val queue = Channel<suspend () -> Unit>(Channel.UNLIMITED)

    init {
        scope.launch {
            for (operation in queue) operation()
        }
    }

    fun offer(operation: suspend () -> Unit): Boolean = queue.trySend(operation).isSuccess

    suspend fun run(operation: suspend () -> Unit) {
        val finished = CompletableDeferred<Unit>()
        check(offer {
            try {
                operation()
                finished.complete(Unit)
            } catch (error: Throwable) {
                finished.completeExceptionally(error)
            }
        })
        finished.await()
    }
}

internal inline fun submitTurnCancellation(
    permissionIngress: PermissionIngress,
    submit: () -> Unit,
) {
    val permissionSnapshot = permissionIngress.beginCancellation()
    try {
        submit()
    } catch (error: Throwable) {
        permissionIngress.restoreAfterFailedCancellation(permissionSnapshot)
        throw error
    }
}

/**
 * Process-wide FIFO for permission callbacks. Workflow children can request
 * permission while the main turn is idle, so main-turn lifecycle events must
 * not suppress or clear their prompts. The engine's correlated
 * `PermissionRequestResolved` event removes the exact request that completed.
 */
internal class PermissionIngress(
    private val permissions: MutableStateFlow<PermissionPromptState?>,
    private val strings: ConversationStrings = DefaultConversationStrings,
) {
    /** Multiple workflow children can park independently; preserve callback order. */
    private val queued = linkedMapOf<ULong, PermissionPromptState>()

    internal class CancellationSnapshot internal constructor()

    @Synchronized
    fun beginTurn() = Unit

    @Synchronized
    fun confirmTurnStarted() = Unit

    @Synchronized
    fun beginCancellation(): CancellationSnapshot = CancellationSnapshot()

    @Synchronized
    fun restoreAfterFailedCancellation(@Suppress("UNUSED_PARAMETER") snapshot: CancellationSnapshot) = Unit

    @Synchronized
    fun endTurn() = Unit

    @Synchronized
    fun publish(request: PermissionRequest) {
        queued[request.requestId] = permissionRequestToPrompt(request, strings)
        publishHead()
    }

    @Synchronized
    fun resolve(requestId: ULong) {
        queued.remove(requestId)
        publishHead()
    }

    private fun publishHead() {
        permissions.value = queued.values.firstOrNull()
    }
}

/**
 * Non-blocking callback ingress with lossless, ordered delivery to a Flow.
 *
 * Native event callbacks must return promptly, while assistant deltas and turn
 * boundaries must never be discarded. An unlimited channel decouples the
 * callback thread from a single suspending SharedFlow pump: slow collectors add
 * bounded-by-turn memory pressure instead of blocking Rust or dropping tokens.
 */
internal class LosslessEventRelay<T>(
    scope: CoroutineScope,
) {
    private val queue = Channel<T>(capacity = Channel.UNLIMITED)
    private val shared = MutableSharedFlow<T>(extraBufferCapacity = 64)
    val events: SharedFlow<T> = shared.asSharedFlow()

    init {
        scope.launch {
            for (event in queue) shared.emit(event)
        }
    }

    /** Safe for a native callback thread; preserves FIFO order without suspension. */
    fun offer(event: T): Boolean = queue.trySend(event).isSuccess

    fun close() {
        queue.close()
    }
}
