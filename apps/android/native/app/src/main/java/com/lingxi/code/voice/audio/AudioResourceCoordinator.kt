package com.lingxi.code.voice.audio

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** Stable host ownership. Tool/request identity is deliberately not part of this key. */
internal data class AudioOwnerKey(
    val kind: Kind,
    val id: String,
) {
    enum class Kind { Session, Ui, System }

    companion object {
        fun session(sessionId: String) = AudioOwnerKey(Kind.Session, sessionId)
        fun ui(instanceId: String) = AudioOwnerKey(Kind.Ui, instanceId)
        fun system(instanceId: String) = AudioOwnerKey(Kind.System, instanceId)
    }
}

internal data class AudioOperationIdentity(
    val id: String,
    val generation: Long,
    val serviceEpoch: Long,
)

internal enum class AudioResource { Capture, Playback, SystemRender, OfflineRender }

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
            AudioResource.OfflineRender -> {
                if (leases.any {
                        it.modelId == modelId && it.resource in setOf(AudioResource.OfflineRender, AudioResource.Playback)
                    }
                ) {
                    return AudioLeaseDecision.Busy
                }
            }
            AudioResource.SystemRender -> {
                if (systemRendering || deviceIo.isNotEmpty()) return AudioLeaseDecision.Busy
            }
            AudioResource.Capture -> {
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
