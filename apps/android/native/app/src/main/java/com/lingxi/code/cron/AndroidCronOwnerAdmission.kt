package com.lingxi.code.cron

import android.content.Context
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.project.MobileUserStorage
import com.lingxi.code.project.ProjectRepository
import com.lingxi.code.project.ProjectStore
import com.lingxi.code.project.ScopeStateStore
import com.lingxi.code.project.projectStrings
import com.lingxi.code.project.requireMatchingSessionOwners
import com.lingxi.code.project.requireOwnerRegistryPresent
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File
import java.nio.file.Files

/** Admits a durable SDK reservation without re-entering the SDK's Cron lock. */
internal class AndroidCronOwnerAdmission(context: Context, private val scope: CronScope) {
    private val appContext = context.applicationContext

    private fun repository(): ProjectRepository {
        val paths = MobileUserStorage.directories(appContext)
        val ownerFile = File(appContext.filesDir, ScopeStateStore.SCOPE_STATE_FILE)
        return ProjectRepository(
            File(paths.projects), userHome = File(paths.home), projectsGuestRoot = paths.projectsGuestPath,
            userHomeGuestPath = paths.guestHome, ownerStateFile = ownerFile, strings = projectStrings(appContext),
        )
    }

    /** Scan all SDK ledgers before starting a new fire, including after cancellation. */
    suspend fun prepare() = withContext(Dispatchers.IO) {
        val epoch = com.lingxi.code.conversation.SessionAdmissionGate.scanEpoch()
        com.lingxi.code.conversation.ConversationServiceOwners.initialize(appContext)
        com.lingxi.code.conversation.ConversationServiceOwners.persisted(appContext)
        val repository = repository()
        ProjectStore.admissionRepositoryMutex.withLock {
            check(!repository.bothSessionIndexesMissing()) { "Conversation indexes are missing" }
            val state = repository.load()
            check(repository.knownProjectIds() == state.projects.map { it.record.id }.toSet()) {
                "A project owner index is unavailable"
            }
            val indexed = repository.indexedSessionOwners(state.globalSessions)
            val owners = ScopeStateStore(appContext)
            val ownerFile = File(appContext.filesDir, ScopeStateStore.SCOPE_STATE_FILE)
            requireOwnerRegistryPresent(ownerFile.exists() || Files.isSymbolicLink(ownerFile.toPath()),
                observedBefore = false, hasIndexedOwners = indexed.isNotEmpty())
            val recorded = owners.readSessionOwners()
            requireMatchingSessionOwners(indexed, recorded)
            val discovered = AndroidCronRepository.get(appContext).strictOwnerReceipts(repository.knownProjectIds())
            val indexedPaths = buildMap {
                state.globalSessions.forEach { row -> row.transcriptPath?.let { put(canonicalSessionId(row.sessionId), it) } }
                state.projects.forEach { project -> project.sessions.forEach { row ->
                    row.transcriptPath?.let { put(canonicalSessionId(row.sessionId), it) }
                } }
            }
            owners.reconcileCronReceipts(discovered, indexed, indexedPaths, state.projects.mapTo(mutableSetOf()) { it.record.id })
        }
        com.lingxi.code.conversation.SessionAdmissionGate.completeOwnerScan(epoch)
    }

    /** Reject a conflicting candidate before the SDK can create a source or ledger entry. */
    suspend fun validateCandidate(candidate: CronOwnerCandidate, token: Long): Boolean = withContext(Dispatchers.IO) {
        val mode = validateCronOwnerCandidate(candidate)
        val id = canonicalSessionId(candidate.sessionId)
        // A turn accepted before process death may not have a native engine yet.
        // It still owns the session until its durable recovery has finished.
        com.lingxi.code.conversation.ConversationServiceOwners.initialize(appContext)
        if (com.lingxi.code.conversation.ConversationServiceOwners.persisted(appContext)
                .any { canonicalSessionId(it.sessionId) == id }) return@withContext false
        val repository = repository()
        // The per-UUID Cron reservation also fences project scope changes.
        // Read scheduler stores before taking the repository mutex so a slow
        // scan does not serialize unrelated navigation or catalog updates.
        val scannedProjectIds = repository.knownProjectIds()
        val sdkReceipts = AndroidCronRepository.get(appContext).strictOwnerReceipts(scannedProjectIds)
        ProjectStore.admissionRepositoryMutex.withLock {
            check(!repository.bothSessionIndexesMissing()) { "Conversation indexes are missing" }
            val state = repository.load()
            check(repository.knownProjectIds() == state.projects.map { it.record.id }.toSet()) {
                "A project owner index is unavailable"
            }
            check(repository.knownProjectIds() == scannedProjectIds) { "Project scopes changed during scheduled owner validation" }
            val project = com.lingxi.code.model.conversationScopeFromKey(candidate.scopeKey) as? com.lingxi.code.model.ConversationScope.Project
            if (project != null) check(state.projects.any { it.record.id == project.projectId }) { "The scheduled project owner is unavailable" }
            val indexed = repository.indexedSessionOwners(state.globalSessions)
            val indexedPath = (state.globalSessions + state.projects.flatMap { it.sessions })
                .firstOrNull { canonicalSessionId(it.sessionId) == id }?.transcriptPath
            // Another scan may have observed a durable SDK reservation after
            // the initial scheduler preflight but before this UUID was held.
            // beginOwner runs before the SDK task lock, so re-read the ledgers
            // while this candidate's UUID reservation excludes new writers.
            val owners = ScopeStateStore(appContext)
            val ownerFile = File(appContext.filesDir, ScopeStateStore.SCOPE_STATE_FILE)
            val indexedPaths = buildMap {
                state.globalSessions.forEach { row -> row.transcriptPath?.let { put(canonicalSessionId(row.sessionId), it) } }
                state.projects.forEach { project -> project.sessions.forEach { row ->
                    row.transcriptPath?.let { put(canonicalSessionId(row.sessionId), it) }
                } }
            }
            // Reconcile every newly observed owner, not only this candidate.
            // A receipt reserved by another in-flight callback stays deferred
            // until that callback mirrors it with its own token.
            val reconciled = owners.reconcileCronReceipts(sdkReceipts, indexed, indexedPaths,
                state.projects.mapTo(mutableSetOf()) { it.record.id })
            owners.withSessionOwnersLocked { recorded ->
                val gate = com.lingxi.code.conversation.SessionAdmissionGate
                while (true) {
                    val version = gate.receiptVersion()
                    val receipts = (CronRunHistoryStore(File(appContext.filesDir, "cron/history")).validatedOwnerReceipts() + reconciled)
                        .filter { canonicalSessionId(checkNotNull(it.sessionId)) == id }
                    requireOwnerRegistryPresent(ownerFile.exists() || Files.isSymbolicLink(ownerFile.toPath()),
                        observedBefore = false, hasIndexedOwners = indexed.isNotEmpty() || receipts.isNotEmpty())
                    requireMatchingSessionOwners(indexed, recorded)
                    val result: Boolean? = synchronized(gate.lock) {
                        if (version != gate.receiptVersion()) return@synchronized null
                        if (!gate.permitsNewOwner(id, token)) return@synchronized false
                        val runtime = gate.runtimeIdentity(id)
                        val owner = ScopeStateStore.SessionOwner(candidate.scopeKey, mode)
                        val knownOwners = listOfNotNull(indexed[id], recorded[id]) + receipts.map {
                            ScopeStateStore.SessionOwner(checkNotNull(it.sessionScopeKey),
                                com.lingxi.code.model.sessionModeFromWireValue(checkNotNull(it.sessionMode)))
                        }
                        val paths = listOfNotNull(indexedPath, runtime?.transcriptPath) + receipts.map { checkNotNull(it.sessionTranscriptPath) }
                        val expectedPath = candidate.transcriptPath
                        val matchingOwner = knownOwners.none { it != owner } &&
                            (runtime == null || (runtime.workspace == candidate.scopeKey && runtime.mode == mode))
                        val valid = if (!matchingOwner) false else if (expectedPath == null) {
                            knownOwners.isEmpty() && runtime == null && paths.isEmpty()
                        } else {
                            (knownOwners.isNotEmpty() || runtime != null) && paths.isNotEmpty() && paths.all { it == expectedPath } &&
                                (runtime == null || runtime.transcriptPath == expectedPath)
                        }
                        if (!valid && sdkReceipts.any { canonicalSessionId(checkNotNull(it.sessionId)) == id }) {
                            // An existing, not-yet-mirrored SDK owner must be
                            // reconciled before any unrelated new claim.
                            gate.invalidateCronOwners()
                        }
                        valid
                    }
                    if (result != null) return@withSessionOwnersLocked result
                }
                @Suppress("UNREACHABLE_CODE") false
            }
        }
    }

    suspend fun persist(identity: CronOwnerIdentity, token: Long): Boolean = withContext(Dispatchers.IO) {
        val mode = validateCronOwnerIdentity(identity)
        val id = canonicalSessionId(identity.sessionId)
        synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
            check(com.lingxi.code.conversation.SessionAdmissionGate.permitsNewOwner(id, token)) {
                "Scheduled session reservation is no longer valid"
            }
            val runtime = com.lingxi.code.conversation.SessionAdmissionGate.runtimeIdentity(id)
            check(runtime == null || (runtime.workspace == identity.scopeKey && runtime.mode == mode &&
                runtime.transcriptPath == identity.transcriptPath)) { "Scheduled session conflicts with an active owner" }
        }
        val repository = repository()
        val ownerFile = File(appContext.filesDir, ScopeStateStore.SCOPE_STATE_FILE)
        ProjectStore.admissionRepositoryMutex.withLock {
            check(!repository.bothSessionIndexesMissing()) { "Conversation indexes are missing" }
            val state = repository.load()
            check(repository.knownProjectIds() == state.projects.map { it.record.id }.toSet()) {
                "A project owner index is unavailable"
            }
            val ownerScope = com.lingxi.code.model.conversationScopeFromKey(identity.scopeKey)
            if (ownerScope is com.lingxi.code.model.ConversationScope.Project) {
                check(state.projects.any { it.record.id == ownerScope.projectId }) { "The scheduled project owner is unavailable" }
            }
            val indexed = repository.indexedSessionOwners(state.globalSessions)
            val owners = ScopeStateStore(appContext)
            requireOwnerRegistryPresent(ownerFile.exists() || Files.isSymbolicLink(ownerFile.toPath()),
                observedBefore = false, hasIndexedOwners = indexed.isNotEmpty())
            val recorded = owners.readSessionOwners()
            requireMatchingSessionOwners(indexed, recorded)
            val indexedPaths = buildMap {
                state.globalSessions.forEach { row -> row.transcriptPath?.let { put(canonicalSessionId(row.sessionId), it) } }
                state.projects.forEach { project -> project.sessions.forEach { row ->
                    row.transcriptPath?.let { put(canonicalSessionId(row.sessionId), it) }
                } }
            }
            val receipt = CronRunRecord("reservation:${scope.scopeId}:$id", "reservation", scope.scopeId,
                scope.projectId, scope.projectName, "", 0L, 0L, status = CronRunStatus.Running,
                sessionId = id, sessionTranscriptPath = identity.transcriptPath,
                sessionMode = identity.mode, sessionScopeKey = identity.scopeKey)
            owners.reconcileCronReceipts(listOf(receipt), indexed, indexedPaths,
                state.projects.mapTo(mutableSetOf()) { it.record.id }, reservationToken = token)
            owners.claimCronSessionOwner(id, identity.scopeKey, mode, token)
        }
    }
}
