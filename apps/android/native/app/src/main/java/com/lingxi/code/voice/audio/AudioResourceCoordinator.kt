package com.lingxi.code.voice.audio

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.async
import kotlinx.coroutines.Job
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** One issued attachment namespace. This is identity data, not an authorization token. */
internal data class AudioOwnerAttachmentIdentity(
    val hostId: String,
    val generation: ULong,
)

/** Stable host ownership. Tool/request identity is deliberately not part of this key. */
internal data class AudioOwnerKey(
    val kind: Kind,
    val id: String,
    val attachment: AudioOwnerAttachmentIdentity? = null,
) {
    enum class Kind { Session, Ui, System }

    companion object {
        fun session(sessionId: String) = AudioOwnerKey(Kind.Session, sessionId)
        fun ui(instanceId: String) = AudioOwnerKey(Kind.Ui, instanceId)
        fun system(instanceId: String) = AudioOwnerKey(Kind.System, instanceId)
        fun attached(
            kind: Kind,
            logicalOwnerId: String,
            hostId: String,
            generation: ULong,
        ) = AudioOwnerKey(
            kind = kind,
            id = logicalOwnerId,
            attachment = AudioOwnerAttachmentIdentity(hostId, generation),
        )
    }
}

internal data class AudioOperationIdentity(
    val id: String,
    val generation: Long,
    val serviceEpoch: Long,
)

internal enum class AudioResource { Capture, Playback, SystemRender, OfflineRender, ProviderRender }

internal data class AudioLease(
    val leaseId: Long,
    val identity: AudioOperationIdentity,
    val owner: AudioOwnerKey,
    val resource: AudioResource,
    val modelId: String? = null,
    val flowDuplex: Boolean = false,
)

internal sealed interface AudioLeaseDecision {
    data class Granted(val lease: AudioLease) : AudioLeaseDecision
    data class PreemptionRequired(val ticket: AudioPreemptionTicket) : AudioLeaseDecision
    data class NativeFailure(val message: String) : AudioLeaseDecision

    data object Busy : AudioLeaseDecision
    data object StaleEpoch : AudioLeaseDecision
}

internal data class AudioPreemptionTicket(
    val identity: AudioOperationIdentity,
    val owner: AudioOwnerKey,
    val resource: AudioResource,
    val modelId: String?,
    val flowDuplex: Boolean,
    val preempted: List<AudioLease>,
)

/** Serializes physical audio access while keeping offline model renders independent. */
internal class AudioResourceCoordinator(initialEpoch: Long) {
    private var epoch = initialEpoch
    private var nextLeaseId = 1L
    private val active = linkedMapOf<Long, AudioLease>()

    @Synchronized
    fun acquire(
        identity: AudioOperationIdentity,
        owner: AudioOwnerKey,
        resource: AudioResource,
        modelId: String? = null,
        foregroundUserInitiated: Boolean = false,
        flowDuplex: Boolean = false,
    ): AudioLeaseDecision {
        if (identity.serviceEpoch != epoch) return AudioLeaseDecision.StaleEpoch
        if (active.values.any { it.identity == identity }) return AudioLeaseDecision.Busy
        if (resource == AudioResource.OfflineRender && modelId.isNullOrBlank()) {
            return AudioLeaseDecision.Busy
        }

        val leases = active.values.toList()
        val deviceIo = leases.filter { it.resource == AudioResource.Capture || it.resource == AudioResource.Playback }
        val systemRendering = leases.any { it.resource == AudioResource.SystemRender }
        var preempt: List<AudioLease> = emptyList()

        when (resource) {
            AudioResource.ProviderRender -> Unit
            AudioResource.OfflineRender -> {
                if (leases.any {
                        it.modelId == modelId && it.resource in setOf(AudioResource.OfflineRender, AudioResource.Playback, AudioResource.Capture)
                    }
                ) {
                    return AudioLeaseDecision.Busy
                }
            }
            AudioResource.SystemRender -> {
                if (systemRendering || deviceIo.isNotEmpty()) return AudioLeaseDecision.Busy
            }
            AudioResource.Capture -> {
                if (modelId != null && leases.any { it.resource == AudioResource.OfflineRender && it.modelId == modelId }) {
                    return AudioLeaseDecision.Busy
                }
                if (systemRendering || deviceIo.any { it.resource == AudioResource.Capture }) {
                    return AudioLeaseDecision.Busy
                }
                val playback = deviceIo.filter { it.resource == AudioResource.Playback }
                if (playback.isNotEmpty()) {
                    val sameFlow = flowDuplex && playback.all { it.owner == owner && it.flowDuplex }
                    if (sameFlow) {
                        // Existing Flow interaction owns both sides until it ends.
                    } else if (foregroundUserInitiated) {
                        preempt = playback
                    } else {
                        return AudioLeaseDecision.Busy
                    }
                }
            }
            AudioResource.Playback -> {
                if (systemRendering) return AudioLeaseDecision.Busy
                if (modelId != null && leases.any { it.resource == AudioResource.OfflineRender && it.modelId == modelId }) {
                    return AudioLeaseDecision.Busy
                }
                val captures = deviceIo.filter { it.resource == AudioResource.Capture }
                if (captures.isNotEmpty() &&
                    !(flowDuplex && captures.all { it.owner == owner && it.flowDuplex })
                ) {
                    return AudioLeaseDecision.Busy
                }
                val playback = deviceIo.filter { it.resource == AudioResource.Playback }
                if (playback.isNotEmpty()) {
                    if (foregroundUserInitiated) preempt = playback
                    else return AudioLeaseDecision.Busy
                }
            }
        }

        if (preempt.isNotEmpty()) {
            return AudioLeaseDecision.PreemptionRequired(
                AudioPreemptionTicket(identity, owner, resource, modelId, flowDuplex, preempt),
            )
        }
        return addLease(identity, owner, resource, modelId, flowDuplex)
    }

    @Synchronized
    fun completePreemption(ticket: AudioPreemptionTicket): AudioLeaseDecision {
        if (ticket.identity.serviceEpoch != epoch) return AudioLeaseDecision.StaleEpoch
        if (ticket.preempted.any { active[it.leaseId] != it }) return AudioLeaseDecision.Busy
        ticket.preempted.forEach { active.remove(it.leaseId) }
        return addLease(ticket.identity, ticket.owner, ticket.resource, ticket.modelId, ticket.flowDuplex)
    }

    private fun addLease(
        identity: AudioOperationIdentity,
        owner: AudioOwnerKey,
        resource: AudioResource,
        modelId: String?,
        flowDuplex: Boolean,
    ): AudioLeaseDecision.Granted {
        val lease = AudioLease(
            leaseId = nextLeaseId++,
            identity = identity,
            owner = owner,
            resource = resource,
            modelId = modelId,
            flowDuplex = flowDuplex,
        )
        active[lease.leaseId] = lease
        return AudioLeaseDecision.Granted(lease)
    }

    @Synchronized
    fun release(lease: AudioLease): Boolean {
        if (lease.identity.serviceEpoch != epoch) return false
        val current = active[lease.leaseId] ?: return false
        if (current != lease) return false
        active.remove(lease.leaseId)
        return true
    }

    /** Adds a model reference when automatic speech falls back after reserving playback. */
    @Synchronized
    fun addModelReference(lease: AudioLease, modelId: String): AudioLease? {
        if (modelId.isBlank() || active[lease.leaseId] != lease) return null
        if (active.values.any {
                it.leaseId != lease.leaseId && it.resource == AudioResource.OfflineRender && it.modelId == modelId
            }
        ) return null
        val updated = lease.copy(modelId = modelId)
        active[lease.leaseId] = updated
        return updated
    }

    @Synchronized
    fun endOwner(owner: AudioOwnerKey): List<AudioLease> {
        val ended = active.values.filter { it.owner == owner }
        ended.forEach { active.remove(it.leaseId) }
        return ended
    }

    @Synchronized
    fun leasesForOwner(owner: AudioOwnerKey): List<AudioLease> = active.values.filter { it.owner == owner }

    @Synchronized
    fun allLeases(): List<AudioLease> = active.values.toList()

    @Synchronized
    fun invalidate(newEpoch: Long): List<AudioLease> {
        require(newEpoch > epoch) { "service epoch must increase" }
        val invalidated = active.values.toList()
        active.clear()
        epoch = newEpoch
        return invalidated
    }

    @Synchronized
    fun isActive(lease: AudioLease): Boolean = active[lease.leaseId] == lease
}

/** Stops native audio before changing the logical lease set. All admission and teardown share one gate. */
internal class AudioLeaseAdmission(
    private val coordinator: AudioResourceCoordinator,
    private val stopNative: suspend (AudioLease) -> Unit,
) {
    private val mutex = Mutex()

    suspend fun acquire(
        identity: AudioOperationIdentity,
        owner: AudioOwnerKey,
        resource: AudioResource,
        modelId: String? = null,
        foregroundUserInitiated: Boolean = false,
        flowDuplex: Boolean = false,
    ): AudioLeaseDecision = mutex.withLock {
        when (
            val decision = coordinator.acquire(
                identity = identity,
                owner = owner,
                resource = resource,
                modelId = modelId,
                foregroundUserInitiated = foregroundUserInitiated,
                flowDuplex = flowDuplex,
            )
        ) {
            is AudioLeaseDecision.PreemptionRequired -> {
                val ticket = decision.ticket
                try {
                    withContext(NonCancellable) {
                        for (lease in ticket.preempted) stopNative(lease)
                    }
                } catch (error: Throwable) {
                    return@withLock AudioLeaseDecision.NativeFailure(error.message ?: "audio preemption failed")
                }
                try {
                    currentCoroutineContext().ensureActive()
                } catch (cancelled: CancellationException) {
                    ticket.preempted.forEach(coordinator::release)
                    throw cancelled
                }
                coordinator.completePreemption(ticket)
            }
            else -> decision
        }
    }

    suspend fun release(lease: AudioLease): Boolean = mutex.withLock {
        try {
            withContext(NonCancellable) { stopNative(lease) }
        } catch (_: Throwable) {
            return@withLock false
        }
        coordinator.release(lease)
    }

    /** Requests native stop while retaining the logical lease until its jobs have settled. */
    suspend fun stopNativeRetainingLease(lease: AudioLease): Boolean = mutex.withLock {
        if (!coordinator.isActive(lease)) return@withLock false
        withContext(NonCancellable) { stopNative(lease) }
        true
    }

    /** Releases a lease after its native device and stream jobs have already stopped. */
    suspend fun releaseAlreadyStopped(lease: AudioLease): Boolean = mutex.withLock {
        coordinator.release(lease)
    }

    /** Null means native teardown failed; an empty list means there was no lease to retire. */
    suspend fun endOwner(owner: AudioOwnerKey): List<AudioLease>? = mutex.withLock {
        val owned = coordinator.leasesForOwner(owner)
        try {
            withContext(NonCancellable) {
                for (lease in owned) stopNative(lease)
            }
        } catch (_: Throwable) {
            return@withLock null
        }
        coordinator.endOwner(owner)
    }

    /** Null means native teardown failed; an empty list is a successful empty invalidation. */
    suspend fun invalidate(newEpoch: Long): List<AudioLease>? = mutex.withLock {
        val active = coordinator.allLeases()
        try {
            withContext(NonCancellable) {
                for (lease in active) stopNative(lease)
            }
        } catch (_: Throwable) {
            return@withLock null
        }
        coordinator.invalidate(newEpoch)
    }
}

internal sealed interface RecordingLookup {
    data class Found(val handle: String) : RecordingLookup
    data object NotFound : RecordingLookup
    data object WrongOwner : RecordingLookup
    data object StaleEpoch : RecordingLookup
}

/** Successful recordings outlive their start operation and are scoped to a stable host owner. */
internal class AudioRecordingRegistry {
    private data class RecordingOwner(val owner: AudioOwnerKey, val epoch: Long)
    private val recordings = mutableMapOf<String, RecordingOwner>()

    @Synchronized
    fun register(handle: String, owner: AudioOwnerKey, serviceEpoch: Long) {
        require(handle.isNotBlank()) { "recording handle must not be blank" }
        require(recordings.putIfAbsent(handle, RecordingOwner(owner, serviceEpoch)) == null) {
            "recording handle already exists"
        }
    }

    @Synchronized
    fun lookup(handle: String, owner: AudioOwnerKey, serviceEpoch: Long): RecordingLookup {
        val registered = recordings[handle] ?: return RecordingLookup.NotFound
        if (registered.epoch != serviceEpoch) return RecordingLookup.StaleEpoch
        if (registered.owner != owner) return RecordingLookup.WrongOwner
        return RecordingLookup.Found(handle)
    }

    @Synchronized
    fun remove(handle: String, owner: AudioOwnerKey, serviceEpoch: Long): Boolean {
        return lookup(handle, owner, serviceEpoch) is RecordingLookup.Found && recordings.remove(handle) != null
    }

    @Synchronized
    fun endOwner(owner: AudioOwnerKey, serviceEpoch: Long): List<String> {
        val handles = recordings.filterValues { it.owner == owner && it.epoch == serviceEpoch }.keys.toList()
        handles.forEach(recordings::remove)
        return handles
    }

    @Synchronized
    fun invalidate(newEpoch: Long) {
        recordings.entries.removeAll { it.value.epoch != newEpoch }
    }
}

internal class AudioOwnerMediaOpenAdmission {
    internal class OpenToken internal constructor(
        val owner: AudioOwnerKey,
        val generation: Long,
        internal val id: Long,
    )

    internal class EndToken internal constructor(
        val owner: AudioOwnerKey,
        val generation: Long,
        internal val openJobs: List<Job>,
        internal val openReceipts: List<CompletableDeferred<Unit>>,
        internal val closeActions: List<suspend () -> Unit>,
        internal val drained: CompletableDeferred<Unit>,
    )

    internal class InvalidationToken internal constructor(
        internal val openJobs: List<Job>,
        internal val openReceipts: List<CompletableDeferred<Unit>>,
        internal val closeActions: List<suspend () -> Unit>,
        internal val drains: List<CompletableDeferred<Unit>>,
    )

    private class OpenRecord(val transactionJob: Job) {
        var transactionCompleted = false
        var leaseIDs: MutableSet<Long>? = null
        var closeAction: (suspend () -> Unit)? = null
        val completionReceipt = CompletableDeferred<Unit>()
        var resultHandedOff = false
    }

    private class OwnerState(var generation: Long) {
        var ending = false
        var endAttempts = 0
        val openRecords = linkedMapOf<Long, OpenRecord>()
        var drained: CompletableDeferred<Unit>? = null
    }

    private val generations = AtomicLong(0L)
    private val tokenIDs = AtomicLong(0L)
    private val owners = mutableMapOf<AudioOwnerKey, OwnerState>()
    private var invalidating = false

    /** Must be called before the caller's first suspension or lease admission. */
    @Synchronized
    fun beginOpen(owner: AudioOwnerKey, transactionJob: Job, allowed: () -> Boolean): OpenToken? {
        if (invalidating || !allowed()) return null
        val state = owners.getOrPut(owner) { OwnerState(nextGeneration()) }
        if (state.ending) return null
        val id = nextTokenID()
        state.openRecords[id] = OpenRecord(transactionJob)
        val token = OpenToken(owner, state.generation, id)
        transactionJob.invokeOnCompletion { onOpenJobCompleted(owner, id, transactionJob) }
        return token
    }

    @Synchronized
    fun isAdmitted(token: OpenToken): Boolean {
        val state = owners[token.owner] ?: return false
        val record = state.openRecords[token.id] ?: return false
        return !invalidating && !state.ending && state.generation == token.generation && record.closeAction == null
    }

    /** Linearization point for a successful open publication. */
    @Synchronized
    fun publish(token: OpenToken, leaseIDs: List<Long>, close: suspend () -> Unit): Boolean {
        val state = owners[token.owner] ?: return false
        val record = state.openRecords[token.id] ?: return false
        if (invalidating || state.ending || state.generation != token.generation || record.closeAction != null) return false
        record.leaseIDs = leaseIDs.toMutableSet()
        record.closeAction = close
        return true
    }

    /** Linearization point before a successfully opened session is returned to its caller. */
    @Synchronized
    fun commitOpenResult(token: OpenToken): Boolean {
        val state = owners[token.owner] ?: return false
        val record = state.openRecords[token.id] ?: return false
        if (invalidating || state.ending || state.generation != token.generation ||
            record.closeAction == null || record.resultHandedOff
        ) return false
        record.resultHandedOff = true
        return true
    }

    /**
     * Return cleanup for a published result whose caller never committed the
     * handoff. The original close action remains in the owner record until its
     * leases are confirmed stopped, so a failed eager cleanup is still owned
     * by EndOwner/invalidation and can be retried.
     */
    @Synchronized
    fun unhandedCloseAction(token: OpenToken): (suspend () -> Unit)? {
        val state = owners[token.owner] ?: return null
        val record = state.openRecords[token.id] ?: return null
        if (state.generation != token.generation || record.resultHandedOff) return null
        return record.closeAction
    }

    @Synchronized
    fun finishOpen(token: OpenToken) {
        val state = owners[token.owner] ?: return
        val record = state.openRecords[token.id] ?: return
        record.completionReceipt.complete(Unit)
        if (record.closeAction == null || record.jobsCompleted() && record.leaseIDs.isNullOrEmpty()) {
            state.openRecords.remove(token.id)
        }
        signalDrainedIfEmpty(state)
        removeIfIdle(token.owner, state)
    }

    @Synchronized
    fun leaseStopped(leaseID: Long) {
        owners.toList().forEach { (owner, state) ->
            val completed = state.openRecords.entries.firstOrNull { (_, record) ->
                val ids = record.leaseIDs ?: return@firstOrNull false
                ids.remove(leaseID) && ids.isEmpty() && record.jobsCompleted() && record.completionReceipt.isCompleted
            }?.key
            if (completed != null) state.openRecords.remove(completed)
            signalDrainedIfEmpty(state)
            removeIfIdle(owner, state)
        }
    }

    @Synchronized
    fun isEnding(owner: AudioOwnerKey): Boolean = invalidating || owners[owner]?.ending == true

    @Synchronized
    fun beginEnd(owner: AudioOwnerKey): EndToken {
        val state = owners.getOrPut(owner) { OwnerState(nextGeneration()) }
        if (!state.ending) {
            state.ending = true
            state.drained = CompletableDeferred()
            state.generation = nextGeneration()
            signalDrainedIfEmpty(state)
        }
        state.endAttempts += 1
        return EndToken(
            owner = owner,
            generation = state.generation,
            openJobs = state.openRecords.values.flatMap { it.jobs() }.distinct(),
            openReceipts = state.openRecords.values.map { it.completionReceipt },
            closeActions = state.openRecords.values.mapNotNull { it.closeAction },
            drained = checkNotNull(state.drained),
        )
    }

    @Synchronized
    fun beginInvalidation(): InvalidationToken {
        invalidating = true
        val drains = ArrayList<CompletableDeferred<Unit>>(owners.size)
        val jobs = ArrayList<Job>()
        val receipts = ArrayList<CompletableDeferred<Unit>>()
        val closeActions = ArrayList<suspend () -> Unit>()
        owners.values.forEach { state ->
            if (!state.ending) {
                state.ending = true
                state.drained = CompletableDeferred()
                state.generation = nextGeneration()
                signalDrainedIfEmpty(state)
            }
            drains += checkNotNull(state.drained)
            state.openRecords.values.forEach { record ->
                jobs += record.jobs()
                receipts += record.completionReceipt
                record.closeAction?.let(closeActions::add)
            }
        }
        return InvalidationToken(jobs, receipts, closeActions, drains)
    }

    suspend fun cancelAndAwait(token: InvalidationToken, timeoutMs: Long, cleanupScope: CoroutineScope) {
        token.openJobs.forEach { it.cancel(kotlinx.coroutines.CancellationException("audio service is invalidating")) }
        awaitCloseActionsAndDrains(token.openJobs, token.openReceipts, token.closeActions, token.drains, timeoutMs, cleanupScope)
    }

    @Synchronized
    fun finishInvalidation(succeeded: Boolean) {
        if (succeeded) {
            owners.clear()
            invalidating = false
        }
    }

    /** Cancellation only asks open transactions to roll back; callers must wait for their cleanup. */
    suspend fun cancelAndAwait(token: EndToken, timeoutMs: Long, cleanupScope: CoroutineScope) {
        token.openJobs.forEach { it.cancel(kotlinx.coroutines.CancellationException("audio owner is ending")) }
        awaitCloseActionsAndDrains(token.openJobs, token.openReceipts, token.closeActions, listOf(token.drained), timeoutMs, cleanupScope)
    }

    private suspend fun awaitCloseActionsAndDrains(
        openJobs: List<Job>,
        openReceipts: List<CompletableDeferred<Unit>>,
        closeActions: List<suspend () -> Unit>,
        drains: List<CompletableDeferred<Unit>>,
        timeoutMs: Long,
        cleanupScope: CoroutineScope,
    ) {
        val closeJobs = closeActions.map { close -> cleanupScope.async { close() } }
        try {
            withTimeout(timeoutMs) {
                closeJobs.awaitAll()
                openJobs.forEach { it.join() }
                openReceipts.forEach { it.await() }
                drains.forEach { it.await() }
            }
        } catch (_: kotlinx.coroutines.TimeoutCancellationException) {
            throw IllegalStateException("media audio open cleanup remains unconfirmed")
        }
    }

    /** A failed end leaves this owner closed; a later EndOwner call may retry the drain. */
    @Synchronized
    fun finishEnd(token: EndToken, succeeded: Boolean): Boolean {
        val state = owners[token.owner] ?: return succeeded
        if (state.generation != token.generation) return false
        if (state.endAttempts > 0) state.endAttempts -= 1
        if (succeeded && !invalidating && state.endAttempts == 0 && state.openRecords.isEmpty()) {
            owners.remove(token.owner, state)
            return true
        }
        return false
    }

    private fun signalDrainedIfEmpty(state: OwnerState) {
        if (state.ending && state.openRecords.isEmpty()) state.drained?.complete(Unit)
    }

    @Synchronized
    private fun onOpenJobCompleted(owner: AudioOwnerKey, id: Long, job: Job) {
        val state = owners[owner] ?: return
        val record = state.openRecords[id]
        if (record != null) {
            if (record.transactionJob === job) record.transactionCompleted = true
            if (record.closeAction != null && record.jobsCompleted() &&
                record.leaseIDs.isNullOrEmpty() && record.completionReceipt.isCompleted
            ) {
                state.openRecords.remove(id)
            }
        }
        signalDrainedIfEmpty(state)
        removeIfIdle(owner, state)
    }

    private fun OpenRecord.jobs(): List<Job> = listOf(transactionJob)

    private fun OpenRecord.jobsCompleted(): Boolean = transactionCompleted

    private fun removeIfIdle(owner: AudioOwnerKey, state: OwnerState) {
        if (!state.ending && state.endAttempts == 0 && state.openRecords.isEmpty()) owners.remove(owner, state)
    }

    private fun nextGeneration(): Long = generations.incrementAndGet().also {
        check(it > 0L) { "audio owner media-open generation is exhausted" }
    }

    private fun nextTokenID(): Long = tokenIDs.incrementAndGet().also {
        check(it > 0L) { "audio owner media-open token space is exhausted" }
    }
}
