package com.lingxi.code.conversation

import com.lingxi.code.model.SessionMode
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.util.concurrent.atomic.AtomicLong

internal fun nextNavigationIntent(sequence: AtomicLong): Long = synchronized(SessionAdmissionGate.lock) {
    sequence.incrementAndGet()
}

/** The scheduled catalog can also reveal a conflicting UUID in another scope. */
internal fun canResolveConversationNotification(projectsLoading: Boolean, scheduledLoading: Boolean): Boolean =
    !projectsLoading && !scheduledLoading

/** Prepare without holding the navigation commit lock; a newer intent wins. */
internal suspend fun <Lease : Any, Prepared : Any> withPreparedNavigation(
    mutex: Mutex,
    isCurrent: () -> Boolean,
    acquire: suspend () -> Lease?,
    prepare: suspend (Lease) -> Prepared?,
    commit: suspend (Lease, Prepared) -> Boolean,
    afterCommit: suspend (Lease, Prepared) -> Unit = { _, _ -> },
    release: suspend (Lease) -> Unit,
): Boolean {
    if (!isCurrent()) return false
    val lease = acquire() ?: return false
    try {
        if (!isCurrent()) return false
        val prepared = prepare(lease) ?: return false
        if (!isCurrent()) return false
        val committed = mutex.withLock {
            if (!isCurrent()) false else commit(lease, prepared)
        }
        if (committed) afterCommit(lease, prepared)
        return committed
    } finally {
        withContext(NonCancellable) { release(lease) }
    }
}

internal enum class SessionOwnerAdmission { Accepted, Deferred, Conflict }

/** Serializes storage publication and a short SDK Cron-owner reservation. */
internal object SessionAdmissionGate {
    val lock = Any()
    internal data class RuntimeIdentity(val workspace: String, val mode: SessionMode, val transcriptPath: String?)
    internal data class ReceiptIdentity(val id: String, val workspace: String, val mode: SessionMode, val transcriptPath: String)
    private data class RuntimeOwner(val workspace: String, val mode: SessionMode, val owner: Any, val transcriptPath: () -> String?)
    private data class CronReservation(val id: String, val workspace: String, val mode: SessionMode, val transcriptPath: String?)
    private data class OwnerWrite(val id: String, val workspace: String, val mode: SessionMode)
    private data class ReceiptWrite(val identities: Set<ReceiptIdentity>, val cronToken: Long?)
    private val pending = mutableMapOf<Long, CronReservation>()
    private val preparing = mutableMapOf<Long, String>()
    private val catalogWrites = mutableMapOf<Long, Set<String>>()
    private val ownerWrites = mutableMapOf<Long, OwnerWrite>()
    private val receiptWrites = mutableMapOf<Long, ReceiptWrite>()
    private val deferredReceiptTokens = mutableSetOf<Long>()
    private var deferredReceiptRescan = false
    private var catalogWriteDeferred = false
    private val scopeChanges = mutableSetOf<Long>()
    private val runtimeOwners = mutableMapOf<String, RuntimeOwner>()
    private var nextToken = 0L
    private var epoch = 0L
    private var receiptVersion = 0L
    private var needsScan = false
    private var barrierFailures = 0
    private val changes = kotlinx.coroutines.flow.MutableStateFlow(0L)
    private val sourceAvailability = kotlinx.coroutines.flow.MutableStateFlow(0L)
    val revision: kotlinx.coroutines.flow.StateFlow<Long> get() = changes

    /** Reserve only the UUID named by the SDK, before it creates any transcript. */
    fun beginCronReservation(sessionId: String, workspace: String, mode: SessionMode, transcriptPath: String?): Long? = synchronized(lock) {
        if (needsScan || scopeChanges.isNotEmpty() || pending.values.any { it.id == sessionId } ||
            sessionId in preparing.values || catalogWrites.values.any { sessionId in it } ||
            ownerWrites.values.any { it.id == sessionId } || receiptWrites.values.any { group -> group.identities.any { it.id == sessionId } }) return@synchronized null
        (++nextToken).also { pending[it] = CronReservation(sessionId, workspace, mode, transcriptPath); epoch++; changes.value++ }
    }

    fun finishCronReservation(token: Long, committed: Boolean) = synchronized(lock) {
        if (pending.remove(token) != null) {
            if (!committed) needsScan = true
            if (deferredReceiptTokens.remove(token)) deferredReceiptRescan = true
            if (pending.isEmpty() && deferredReceiptRescan) {
                needsScan = true
                deferredReceiptRescan = false
            }
            epoch++
            changes.value++
            sourceAvailability.value++
        }
    }

    fun permitsNewOwner(sessionId: String, reservationToken: Long? = null): Boolean = synchronized(lock) {
        !needsScan && scopeChanges.isEmpty() && catalogWrites.values.none { sessionId in it } &&
            ownerWrites.values.none { it.id == sessionId } &&
            receiptWrites.values.none { group -> group.identities.any { it.id == sessionId } } &&
            if (reservationToken == null) pending.values.none { it.id == sessionId }
            else pending[reservationToken]?.id == sessionId
    }

    fun permitsNewOwnersGlobally(): Boolean = synchronized(lock) { !needsScan && scopeChanges.isEmpty() }

    /** Another in-flight Cron callback must mirror this UUID with its own token. */
    fun deferReceiptForOtherCron(identity: ReceiptIdentity, ownToken: Long?): Boolean = synchronized(lock) {
        val other = pending.entries.firstOrNull { (token, reservation) -> token != ownToken && reservation.id == identity.id }
            ?: return@synchronized false
        val reservation = other.value
        if (reservation.workspace != identity.workspace || reservation.mode != identity.mode ||
            (reservation.transcriptPath != null && reservation.transcriptPath != identity.transcriptPath)) {
            needsScan = true
            epoch++
            changes.value++
            error("Scheduled session ${identity.id} conflicts with an in-flight owner")
        }
        deferredReceiptTokens += other.key
        true
    }
    fun pendingSessionIds(exceptCatalogToken: Long? = null): Set<String> = synchronized(lock) {
        pending.values.mapTo(mutableSetOf()) { it.id }.also { result ->
            catalogWrites.filterKeys { it != exceptCatalogToken }.values.forEach(result::addAll)
            ownerWrites.values.mapTo(result) { it.id }
            result.addAll(preparing.values)
            receiptWrites.values.forEach { group -> group.identities.mapTo(result) { it.id } }
        }
    }

    fun catalogWriteAvailable(sessionId: String): Boolean = synchronized(lock) {
        pending.values.none { it.id == sessionId } && ownerWrites.values.none { it.id == sessionId } && sessionId !in preparing.values &&
            receiptWrites.values.none { group -> group.identities.any { it.id == sessionId } } &&
            catalogWrites.values.none { sessionId in it }
    }

    /** Fence a single durable owner claim while its file is staged. */
    fun beginOwnerWrite(sessionId: String, workspace: String, mode: SessionMode, cronToken: Long?): Long? = synchronized(lock) {
        if (!permitsNewOwner(sessionId, cronToken)) return@synchronized null
        (++nextToken).also { ownerWrites[it] = OwnerWrite(sessionId, workspace, mode) }
    }

    fun permitsOwnerWrite(token: Long, sessionId: String, cronToken: Long?): Boolean = synchronized(lock) {
        ownerWrites[token]?.id == sessionId && !needsScan && scopeChanges.isEmpty() &&
            catalogWrites.values.none { sessionId in it } &&
            receiptWrites.values.none { group -> group.identities.any { it.id == sessionId } } &&
            if (cronToken == null) pending.values.none { it.id == sessionId }
            else pending[cronToken]?.id == sessionId
    }

    fun finishOwnerWrite(token: Long) = synchronized(lock) {
        if (ownerWrites.remove(token) != null) {
            changes.value++
            sourceAvailability.value++
        }
    }

    /** Keep only affected UUIDs reserved while a catalog write is staged off-lock. */
    fun beginCatalogWrite(sessionIds: Set<String>): Long? = synchronized(lock) {
        if (needsScan || scopeChanges.isNotEmpty() || sessionIds.any { id ->
                pending.values.any { it.id == id } || ownerWrites.values.any { it.id == id } ||
                    receiptWrites.values.any { group -> group.identities.any { it.id == id } } ||
                    id in preparing.values || catalogWrites.values.any { id in it }
            }) {
            catalogWriteDeferred = true
            return@synchronized null
        }
        (++nextToken).also { catalogWrites[it] = sessionIds }
    }

    fun finishCatalogWrite(token: Long) = synchronized(lock) {
        if (catalogWrites.remove(token) != null) {
            if (catalogWriteDeferred) {
                catalogWriteDeferred = false
                changes.value++
            }
            sourceAvailability.value++
        }
    }

    /** Mirror durable SDK receipts without freezing unrelated owner claims. */
    fun beginReceiptWrite(identities: Set<ReceiptIdentity>, cronToken: Long? = null): Long? = synchronized(lock) {
        if (cronToken != null && identities.none { it.id == pending[cronToken]?.id }) return@synchronized null
        if (identities.any { identity ->
                val id = identity.id
                id in preparing.values || ownerWrites.values.any { it.id == id } ||
                    catalogWrites.values.any { id in it } || receiptWrites.values.any { group -> group.identities.any { it.id == id } } ||
                    pending.any { (token, reservation) -> reservation.id == id &&
                        (token != cronToken || reservation.workspace != identity.workspace || reservation.mode != identity.mode ||
                            (reservation.transcriptPath != null && reservation.transcriptPath != identity.transcriptPath)) }
            }) return@synchronized null
        (++nextToken).also { receiptWrites[it] = ReceiptWrite(identities, cronToken) }
    }

    fun permitsReceiptWrite(token: Long): Boolean = synchronized(lock) {
        val write = receiptWrites[token] ?: return@synchronized false
        if (write.cronToken != null && write.identities.none { it.id == pending[write.cronToken]?.id }) return@synchronized false
        write.identities.all { identity ->
            val runtime = runtimeOwners[identity.id]
            val path = runtime?.let { runCatching { it.transcriptPath() }.getOrNull()?.takeIf(String::isNotBlank) }
            (runtime == null || (runtime.workspace == identity.workspace && runtime.mode == identity.mode && path == identity.transcriptPath)) &&
                pending.none { (cronToken, reservation) -> reservation.id == identity.id &&
                    (cronToken != write.cronToken || reservation.workspace != identity.workspace || reservation.mode != identity.mode ||
                        (reservation.transcriptPath != null && reservation.transcriptPath != identity.transcriptPath)) }
        }
    }

    fun receiptVersion(): Long = synchronized(lock) { receiptVersion }

    fun noteReceiptPublication(token: Long) = synchronized(lock) {
        check(token in receiptWrites)
        receiptVersion++
    }

    fun finishReceiptWrite(token: Long) = synchronized(lock) {
        if (receiptWrites.remove(token) != null) {
            changes.value++
            sourceAvailability.value++
        }
    }

    fun publicationPending(sessionId: String): Boolean = synchronized(lock) {
        catalogWrites.values.any { sessionId in it } || ownerWrites.values.any { it.id == sessionId } ||
            receiptWrites.values.any { group -> group.identities.any { it.id == sessionId } }
    }

    /** A real lifecycle event waits for a transient disk publication, not a false owner conflict. */
    suspend fun awaitPublication(sessionId: String) {
        while (true) {
            val (pendingWrite, revision) = synchronized(lock) { publicationPending(sessionId) to sourceAvailability.value }
            if (!pendingWrite) return
            sourceAvailability.first { it != revision }
        }
    }

    /** A proven conflict must be reported, rather than treated as a transient wait. */
    fun conflictingRuntimeIdentity(sessionId: String, workspace: String, mode: SessionMode?, transcriptPath: String?): RuntimeIdentity? = synchronized(lock) {
        val owner = runtimeOwners[sessionId] ?: return@synchronized null
        val activePath = runCatching { owner.transcriptPath() }.getOrNull()?.takeIf(String::isNotBlank)
        val suppliedPath = transcriptPath?.takeIf(String::isNotBlank)
        if (owner.workspace != workspace || (mode != null && owner.mode != mode) ||
            (activePath != null && suppliedPath != null && activePath != suppliedPath)) {
            RuntimeIdentity(owner.workspace, owner.mode, activePath)
        } else null
    }

    /** Only the retained source's exact identity may enter an index. */
    fun permitsCatalogWrite(sessionId: String, workspace: String, mode: SessionMode?, transcriptPath: String?,
        catalogToken: Long? = null): Boolean = synchronized(lock) {
        if (sessionId in preparing.values) return@synchronized false
        if (ownerWrites.values.any { it.id == sessionId }) return@synchronized false
        if (receiptWrites.values.any { group -> group.identities.any { it.id == sessionId } }) return@synchronized false
        if (catalogWrites.any { (token, ids) -> token != catalogToken && sessionId in ids }) {
            catalogWriteDeferred = true
            return@synchronized false
        }
        if (catalogToken != null && catalogWrites[catalogToken]?.contains(sessionId) != true) return@synchronized false
        runtimeOwners[sessionId]?.let { owner ->
            val activePath = runCatching { owner.transcriptPath() }.getOrNull()?.takeIf(String::isNotBlank)
            if (owner.workspace != workspace || owner.mode != mode || activePath == null || activePath != transcriptPath) {
                return@synchronized false
            }
        }
        val reservation = pending.values.firstOrNull { it.id == sessionId } ?: return@synchronized true
        reservation.transcriptPath != null && workspace == reservation.workspace && mode == reservation.mode &&
            (transcriptPath.isNullOrBlank() || transcriptPath == reservation.transcriptPath)
    }

    /** Fence a known destination before its native source is constructed. */
    fun beginSourcePreparation(sessionId: String, admittedRecovery: Boolean = false): Long? = synchronized(lock) {
        if (scopeChanges.isNotEmpty() || (!admittedRecovery && !permitsNewOwner(sessionId)) ||
            pending.values.any { it.id == sessionId } || sessionId in preparing.values ||
            catalogWrites.values.any { sessionId in it } || ownerWrites.values.any { it.id == sessionId } ||
            receiptWrites.values.any { group -> group.identities.any { it.id == sessionId } }) return@synchronized null
        (++nextToken).also { preparing[it] = sessionId }
    }

    fun finishSourcePreparation(token: Long) = synchronized(lock) {
        if (preparing.remove(token) != null) {
            changes.value++
            sourceAvailability.value++
        }
    }

    /** Wait without holding the host mutex or closing an admitted source. */
    suspend fun awaitSourcePreparation(sessionId: String, admittedRecovery: Boolean = false): Long {
        while (true) {
            val (token, revision) = synchronized(lock) {
                beginSourcePreparation(sessionId, admittedRecovery) to sourceAvailability.value
            }
            if (token != null) return token
            sourceAvailability.first { it != revision }
        }
    }

    /** Scope publication and external imports can introduce an unseen owner ledger. */
    suspend fun awaitScopeChange(): Long {
        while (true) {
            val (token, revision) = synchronized(lock) {
                val ready = !needsScan && barrierFailures == 0 && pending.isEmpty() && preparing.isEmpty() &&
                    catalogWrites.isEmpty() && ownerWrites.isEmpty() && receiptWrites.isEmpty() && scopeChanges.isEmpty()
                val reservation = if (ready) (++nextToken).also { scopeChanges += it; epoch++; changes.value++ } else null
                reservation to sourceAvailability.value
            }
            if (token != null) return token
            sourceAvailability.first { it != revision }
        }
    }

    fun finishScopeChange(token: Long, validated: Boolean) = synchronized(lock) {
        if (scopeChanges.remove(token)) {
            if (!validated) needsScan = true
            epoch++
            changes.value++
            sourceAvailability.value++
        }
    }

    /** Runtime claims and SDK reservations share this lock, including late native announcements. */
    fun mayRegisterRuntimeOwner(sessionId: String, workspace: String, mode: SessionMode, owner: Any): Boolean = synchronized(lock) {
        val existing = runtimeOwners[sessionId]
        if (existing != null) existing.workspace == workspace && existing.mode == mode && existing.owner === owner
        else {
            val claim = ownerWrites.values.firstOrNull { it.id == sessionId }
            if (claim == null) permitsNewOwner(sessionId)
            else claim.workspace == workspace && claim.mode == mode && !needsScan && scopeChanges.isEmpty() &&
                pending.values.none { it.id == sessionId } && catalogWrites.values.none { sessionId in it } &&
                receiptWrites.values.none { group -> group.identities.any { it.id == sessionId } }
        }
    }

    fun recordRuntimeOwner(sessionId: String, workspace: String, mode: SessionMode, owner: Any,
        transcriptPath: () -> String? = { null }) = synchronized(lock) {
        check(mayRegisterRuntimeOwner(sessionId, workspace, mode, owner))
        val isNew = runtimeOwners[sessionId]?.owner !== owner
        runtimeOwners[sessionId] = RuntimeOwner(workspace, mode, owner, transcriptPath)
        if (isNew) changes.value++
    }

    fun removeRuntimeOwner(owner: Any) = synchronized(lock) {
        if (runtimeOwners.entries.removeAll { it.value.owner === owner }) changes.value++
    }

    /** Retry deferred catalog rows when a retained source publishes its path. */
    fun runtimeSourceChanged(owner: Any) = synchronized(lock) {
        if (runtimeOwners.values.any { it.owner === owner }) changes.value++
    }

    fun runtimeIdentity(sessionId: String): RuntimeIdentity? = synchronized(lock) {
        val existing = runtimeOwners[sessionId]
        existing?.let { RuntimeIdentity(it.workspace, it.mode, runCatching { it.transcriptPath() }.getOrNull()) }
    }

    fun scanEpoch(): Long = synchronized(lock) { epoch }
    fun needsOwnerScan(): Boolean = synchronized(lock) { needsScan && pending.isEmpty() && scopeChanges.isEmpty() }
    fun hasBarrierFailure(): Boolean = synchronized(lock) { barrierFailures != 0 }
    fun noteBarrierFailure() = synchronized(lock) {
        barrierFailures++
        needsScan = true
        epoch++
        changes.value++
    }
    fun resolveBarrierFailure() = synchronized(lock) {
        check(barrierFailures > 0)
        barrierFailures--
        epoch++
        changes.value++
    }
    fun invalidateCronOwners() = synchronized(lock) {
        needsScan = true
        epoch++
        changes.value++
    }
    fun completeOwnerScan(startEpoch: Long) = synchronized(lock) {
        if (startEpoch == epoch && pending.isEmpty() && scopeChanges.isEmpty() && barrierFailures == 0 && needsScan) {
            needsScan = false
            epoch++
            changes.value++
            sourceAvailability.value++
        }
    }
}

/** Keep the current owner mounted while fencing a damaged index from new owners. */
internal fun shouldBlockForUnavailableStorage(storageUnavailable: Boolean, hasRegisteredOwner: Boolean): Boolean =
    storageUnavailable && !hasRegisteredOwner

internal fun shouldBlockForOwnerStateValidation(valid: Boolean?, hasRegisteredOwner: Boolean): Boolean =
    shouldBlockForUnavailableStorage(valid != true, hasRegisteredOwner)

/** Associates an immutable mode and exact workspace/session identity with one owner. */
internal class SessionOwnerRegistry<T : Any> {
    private data class Owner<T>(val workspace: String, val mode: SessionMode, val value: T)
    private val owners = mutableMapOf<String, Owner<T>>()

    @Synchronized fun register(workspace: String, session: String, mode: SessionMode, owner: T, allowNew: Boolean = true): Boolean {
        val old = owners[session]
        if (old != null) return old.workspace == workspace && old.mode == mode && old.value === owner
        if (!allowNew) return false
        owners[session] = Owner(workspace, mode, owner)
        return true
    }

    @Synchronized fun find(workspace: String, session: String, mode: SessionMode? = null): T? {
        val saved = owners[session] ?: return null
        return saved.value.takeIf { saved.workspace == workspace && (mode == null || mode == saved.mode) }
    }

    @Synchronized fun contains(session: String): Boolean = session in owners

    @Synchronized fun removeOwner(owner: T) {
        owners.entries.removeAll { it.value.value === owner }
    }
}
