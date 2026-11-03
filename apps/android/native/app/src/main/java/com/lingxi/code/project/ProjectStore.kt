package com.lingxi.code.project

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.lingxi.code.R
import com.lingxi.code.model.SessionRow
import com.lingxi.code.model.canonicalSessionId
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File

private class RetryCatalogPublication : RuntimeException()
private class CatalogPublicationStopped : RuntimeException()

class ProjectStore private constructor(
    private val repository: ProjectRepository,
    private val synchronizer: SafProjectSynchronizer,
    private val appContext: Context,
) : ViewModel() {
    private val _state = MutableStateFlow(ProjectStoreState())
    val state: StateFlow<ProjectStoreState> = _state.asStateFlow()
    private val repositoryMutex = admissionRepositoryMutex
    private val ownerStore by lazy { ScopeStateStore(appContext) }
    private val ownerStateFile = File(appContext.filesDir, ScopeStateStore.SCOPE_STATE_FILE)
    private var ownerRegistryObserved = false

    init {
        reload()
    }

    fun reload() {
        viewModelScope.launch {
            runOperation(
                ProjectOperationKind.RefreshSessions,
                message = com.lingxi.code.theme.LocaleWrapper.getString(appContext, R.string.project_loading_message),
            ) {
                loadValidatedRepository()
            }
            recoverStagedAuthoringIntents()
        }
    }

    /** Repair only an interrupted pre-selection authoring owner; never select it. */
    internal suspend fun recoverStagedAuthoringIntents() {
        if (_state.value.storageUnavailable || _state.value.loading) return
        val intents = try { com.lingxi.code.apps.AuthoringIntentStore(appContext).staged() }
        catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
        catch (failure: Exception) {
            android.util.Log.w("ProjectStore", "Unable to read staged authoring intents", failure)
            return
        }
        val store = com.lingxi.code.apps.AuthoringIntentStore(appContext)
        for (intent in intents) {
            try {
                val sessionId = checkNotNull(intent.sessionId)
                val transcriptPath = checkNotNull(intent.transcriptPath)
                val ownerMode = checkNotNull(intent.sessionMode)
                check(ownerMode == com.lingxi.code.model.SessionMode.Code)
                val project = _state.value.projects.singleOrNull { it.record.id == intent.projectId } ?: continue
                if (project.record.directory != intent.sourceDirectory) continue
                val existing = project.sessions.singleOrNull { canonicalSessionId(it.sessionId) == sessionId }
                check(existing == null || (existing.mode == com.lingxi.code.model.SessionMode.Code && existing.transcriptPath == transcriptPath))
                val gate = com.lingxi.code.conversation.SessionAdmissionGate
                fun stillAvailable(): Boolean = !_state.value.storageUnavailable &&
                    _state.value.projects.any { it.record.id == intent.projectId && it.record.directory == intent.sourceDirectory } &&
                    synchronized(gate.lock) { gate.runtimeIdentity(sessionId) == null }
                if (!stillAvailable()) continue
                val apps = com.lingxi.code.apps.UnifiedLocalApps.get(appContext)
                val source = apps.prepareAuthoringSource(intent.sourceId, intent.sourceDirectory, intent.sourceRequestId, ownerMode)
                check(source.sourceId == intent.sourceId && source.path == intent.sourceDirectory &&
                    source.sourcePath == apps.hostFile(intent.sourceDirectory).absolutePath)
                val roots = MobileUserStorage.directories(appContext).projectGrantRoots()
                val proof = AuthoringSourceIdentity(intent.appId, intent.projectId, source.sourcePath, intent.sourceDirectory)
                check(isSafeProjectWorkspace(roots, intent.sourceDirectory, proof.hostPath, project, proof) && stillAvailable())
                if (!ownerStore.claimSessionOwner(sessionId, "project.${intent.projectId}", com.lingxi.code.model.SessionMode.Code, ::stillAvailable)) continue
                if (existing != null || recordStartedSession(intent.projectId, sessionId, project.record.name, com.lingxi.code.model.SessionMode.Code,
                        transcriptPath = transcriptPath, allowCommit = ::stillAvailable, markActive = { false })) {
                    store.markCatalogued(sessionId)
                }
            } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
            catch (failure: Exception) { android.util.Log.w("ProjectStore", "Unable to recover a staged authoring owner", failure) }
        }
    }

    suspend fun createInternal(name: String, mode: com.lingxi.code.model.SessionMode): ProjectSnapshot =
        withScopeChange { runSuspendingOperation(
            ProjectOperationKind.Create,
            message = com.lingxi.code.theme.LocaleWrapper.getString(appContext, R.string.project_creating_message),
        ) {
            loadValidatedRepository()
            val created = repository.createInternal(name, mode)
            _state.value = loadValidatedRepository()
            created
        } }

    suspend fun createAuthoringPlaceholder(name: String, mode: com.lingxi.code.model.SessionMode,
        projectId: String, authoringIntentId: String): ProjectSnapshot =
        withScopeChange { runSuspendingOperation(ProjectOperationKind.Create, message = appContext.getString(R.string.project_creating_message)) {
            loadValidatedRepository()
            repository.createAuthoringPlaceholder(name, mode, projectId, authoringIntentId).also { _state.value = loadValidatedRepository() }
        } }

    suspend fun registerAuthoringWorkspace(name: String, directory: String, mode: com.lingxi.code.model.SessionMode,
        projectId: String, authoringIntentId: String, allowCommit: () -> Boolean = { true }): ProjectSnapshot =
        withScopeChange { runSuspendingOperation(ProjectOperationKind.Create, message = appContext.getString(R.string.project_creating_message)) {
            loadValidatedRepository()
            repository.withWritePublication({ _, commit ->
                synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
                    check(allowCommit()) { "Authoring selection changed" }
                    commit()
                }
            }) {
                repository.registerAuthoringWorkspace(name, directory, mode, projectId, authoringIntentId)
            }.also { _state.value = loadValidatedRepository() }
        } }

    suspend fun importSaf(name: String, treeUri: Uri, mode: com.lingxi.code.model.SessionMode): ProjectSnapshot =
        withScopeChange { runSuspendingOperation(
            ProjectOperationKind.Import,
            message = com.lingxi.code.theme.LocaleWrapper.getString(appContext, R.string.project_importing_message),
        ) {
            loadValidatedRepository()
            persistTreePermission(treeUri)
            val created = repository.createSafMirror(
                name = name,
                sourceTreeUri = treeUri.toString(),
                sourceDisplayName = treeDisplayName(treeUri),
                mode = mode,
            )
            val result = synchronizer.importInitial(created.record.id)
            _state.value = loadValidatedRepository().copy(conflicts = result.conflicts)
            result.project
        } }

    /**
     * Persist the destination before ChatViewModel commits its replacement
     * Source. This is intentionally non-observable: [publishActive] runs in the
     * same Source transaction, so Compose never sees a Project/Engine mismatch.
     */
    suspend fun persistActive(projectId: String?, allowCommit: () -> Boolean = { true }): ProjectStoreState =
        withRepository {
            try {
                repository.withWritePublication({ _, commit ->
                    synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
                        if (!allowCommit()) throw StaleActiveSelection()
                        check(!_state.value.storageUnavailable) { "Conversation storage is unavailable" }
                        commit()
                    }
                }) {
                    repository.setActiveProject(projectId)
                }
            } catch (_: StaleActiveSelection) {
                _state.value
            }
        }

    private class StaleActiveSelection : RuntimeException()

    suspend fun validateOwnerIndexes() = withRepository {
        publishRepositoryState(loadValidatedRepository(synchronizeSdk = false))
    }

    /** Publish the already-durable Project state without another I/O race. */
    fun publishActive(persisted: ProjectStoreState, allowCommit: () -> Boolean = { true }, activate: () -> Unit): Boolean =
        synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
            if (_state.value.storageUnavailable || !allowCommit()) false else {
                _state.value = persisted.copy(
                    conflicts = _state.value.conflicts,
                    operation = _state.value.operation,
                    errorMessage = null,
                )
                activate()
                true
            }
        }

    /** Publish a retained owner immediately; persistence is separately conditional. */
    fun publishRetainedActive(projectId: String?, allowCommit: () -> Boolean, activate: () -> Unit): Boolean =
        synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
            if (!allowCommit() || (projectId != null && _state.value.projects.none { it.record.id == projectId })) false
            else {
                _state.value = _state.value.copy(activeProjectId = projectId)
                activate()
                true
            }
        }

    suspend fun addVisibleMode(projectId: String, mode: com.lingxi.code.model.SessionMode) = withRepository {
        repository.addVisibleMode(projectId, mode)
        publishRepositoryState(loadValidatedRepository())
    }

    suspend fun recordWorkspace(
        projectId: String?,
        state: SessionWorkspaceState,
        expectedPreviousPath: String? = null,
        checkPreviousPath: Boolean = false,
        allowCommit: () -> Boolean = { true },
        updateProjectDirectory: () -> Boolean = { true },
    ): Unit = withRepository {
        val id = canonicalSessionId(state.sessionId)
        val key = projectId?.let { "project.$it" } ?: "global"
        val gate = com.lingxi.code.conversation.SessionAdmissionGate
        val token = synchronized(gate.lock) {
            if (!allowCommit() || !gate.permitsNewOwnersGlobally()) return@synchronized null
            gate.beginCatalogWrite(setOf(id))
        } ?: return@withRepository
        try {
            val current = repository.load()
            val row = (if (projectId == null) current.globalSessions else current.projects.firstOrNull { it.record.id == projectId }?.sessions.orEmpty())
                .firstOrNull { canonicalSessionId(it.sessionId) == id }
            requireMatchingRuntimeSource(id, key, row?.mode, state.transcriptPath)
            if (row?.transcriptPath != null && state.transcriptPath.isNotBlank() && row.transcriptPath != state.transcriptPath &&
                expectedPreviousPath == row.transcriptPath) {
                val receipts = com.lingxi.code.cron.CronRunHistoryStore(File(appContext.filesDir, "cron/history")).validatedOwnerReceipts()
                check(receipts.none { canonicalSessionId(checkNotNull(it.sessionId)) == id && it.sessionTranscriptPath != state.transcriptPath }) {
                    "Scheduled session $id still references a different transcript source"
                }
            }
            while (true) {
                val updateDirectory = synchronized(gate.lock) { updateProjectDirectory() }
                var published = 0
                try {
                    val result = repository.withWritePublication({ target, commit ->
                        synchronized(gate.lock) {
                            val valid = allowCommit() && gate.permitsNewOwnersGlobally() &&
                                gate.permitsCatalogWrite(id, key, row?.mode, state.transcriptPath, token)
                            if (!valid) {
                                if (published == 0) throw CatalogPublicationStopped()
                            } else {
                                requireMatchingRuntimeSource(id, key, row?.mode, state.transcriptPath)
                                if (target.name == PROJECT_MANIFEST_FILE && updateProjectDirectory() != updateDirectory) {
                                    if (published == 0) throw RetryCatalogPublication()
                                } else {
                                    commit()
                                    published++
                                }
                            }
                        }
                    }) {
                        repository.recordWorkspace(projectId, state, expectedPreviousPath, checkPreviousPath,
                            updateProjectDirectory = updateDirectory)
                    }
                    synchronized(gate.lock) {
                        publishRepositoryState(result.copy(activeProjectId = _state.value.activeProjectId))
                    }
                    return@withRepository
                } catch (_: RetryCatalogPublication) {
                    // Rebuild with the latest selected owner before publication.
                } catch (_: CatalogPublicationStopped) {
                    return@withRepository
                }
            }
        } finally {
            gate.finishCatalogWrite(token)
        }
    }

    suspend fun syncEngineSessions(projectId: String?, rows: List<SessionRow>): ProjectStoreState =
        withRepository {
            readRecordedOwners()
            val timestamp = System.currentTimeMillis()
            val key = projectId?.let { "project.$it" } ?: "global"
            val current = repository.load()
            val previousRows = if (projectId == null) current.globalSessions else
                current.projects.firstOrNull { it.record.id == projectId }?.sessions.orEmpty()
            val gate = com.lingxi.code.conversation.SessionAdmissionGate
            val ids = (rows.map { canonicalSessionId(it.uuid) } + previousRows.map { canonicalSessionId(it.sessionId) }).toSet()
            val (token, reserved) = synchronized(gate.lock) {
                if (!gate.permitsNewOwnersGlobally()) return@withRepository current
                rows.forEach { row -> requireMatchingRuntimeSource(canonicalSessionId(row.uuid), key, row.mode, row.transcriptPath) }
                val available = ids.filterTo(mutableSetOf(), gate::catalogWriteAvailable)
                (gate.beginCatalogWrite(available) ?: return@withRepository current) to available
            }
            try {
                val recorded = readRecordedOwners()
                val blocked = rows.filterNot { row ->
                    val id = canonicalSessionId(row.uuid)
                    id in reserved && gate.permitsCatalogWrite(id, key, row.mode, row.transcriptPath, token)
                }.mapTo(mutableSetOf()) { canonicalSessionId(it.uuid) }
                val permitted = rows.filter { canonicalSessionId(it.uuid) !in blocked }
                val preserve = blocked + (ids - reserved) + (gate.pendingSessionIds(token) -
                    permitted.mapTo(mutableSetOf()) { canonicalSessionId(it.uuid) })
                requireMatchingSessionOwners(permitted.associate {
                    canonicalSessionId(it.uuid) to ScopeStateStore.SessionOwner(key, it.mode)
                }, recorded)
                val result = try {
                    repository.withWritePublication({ _, commit ->
                        synchronized(gate.lock) {
                            if (!gate.permitsNewOwnersGlobally() || permitted.any { row ->
                                    !gate.permitsCatalogWrite(canonicalSessionId(row.uuid), key, row.mode, row.transcriptPath, token)
                                }) throw CatalogPublicationStopped()
                            permitted.forEach { row ->
                                requireMatchingRuntimeSource(canonicalSessionId(row.uuid), key, row.mode, row.transcriptPath)
                            }
                            commit()
                        }
                    }) {
                        repository.updateSessions(projectId, permitted.mapIndexed { index, row ->
                            ProjectSessionSummary(
                                sessionId = row.uuid,
                                title = row.title,
                                transcriptPath = row.transcriptPath,
                                workspaceDirectory = row.workspaceDirectory,
                                messageCount = row.messageCount,
                                relativeTime = row.relativeTime,
                                updatedAtEpochMillis = row.modifiedAtEpochSeconds
                                    ?.takeIf { it > 0L }
                                    ?.times(1000L)
                                    ?: (timestamp - index),
                                mode = row.mode,
                            )
                        }, preserveSessionIds = preserve)
                    }
                } catch (_: CatalogPublicationStopped) {
                    return@withRepository repository.load()
                }
                synchronized(gate.lock) {
                    result.copy(activeProjectId = _state.value.activeProjectId).also(::publishRepositoryState)
                }
            } finally {
                gate.finishCatalogWrite(token)
            }
        }

    suspend fun recordStartedSession(
        projectId: String?,
        sessionId: String,
        title: String,
        mode: com.lingxi.code.model.SessionMode,
        initialMessageCount: Int = 0,
        transcriptPath: String? = null,
        allowCommit: () -> Boolean = { true },
        markActive: () -> Boolean = { true },
    ): Boolean = withRepository {
            readRecordedOwners()
            val id = canonicalSessionId(sessionId)
            val key = projectId?.let { "project.$it" } ?: "global"
            val gate = com.lingxi.code.conversation.SessionAdmissionGate
            val token = synchronized(gate.lock) {
                if (!allowCommit() || !gate.permitsNewOwnersGlobally()) return@synchronized null
                requireMatchingRuntimeSource(id, key, mode, transcriptPath)
                if (!gate.permitsCatalogWrite(id, key, mode, transcriptPath)) return@synchronized null
                gate.beginCatalogWrite(setOf(id))
            }
            if (token == null) return@withRepository false
            try {
                val recorded = readRecordedOwners()
                requireMatchingSessionOwners(mapOf(id to ScopeStateStore.SessionOwner(key, mode)), recorded)
                while (true) {
                    val active = markActive()
                    var published = 0
                    try {
                        val result = repository.withWritePublication({ _, commit ->
                            synchronized(gate.lock) {
                                val valid = allowCommit() && gate.permitsNewOwnersGlobally() &&
                                    gate.permitsCatalogWrite(id, key, mode, transcriptPath, token)
                                if (!valid) {
                                    if (published == 0) throw CatalogPublicationStopped()
                                } else {
                                    requireMatchingRuntimeSource(id, key, mode, transcriptPath)
                                    if (projectId != null && markActive() != active) {
                                        if (published == 0) throw RetryCatalogPublication()
                                    } else {
                                        commit()
                                        published++
                                    }
                                }
                            }
                        }) {
                            repository.recordStartedSession(projectId, sessionId, title, mode, initialMessageCount, transcriptPath,
                                markActive = active)
                        }
                        synchronized(gate.lock) {
                            // Selection may have changed while this owner's row
                            // was staged. Keep the current visible project.
                            publishRepositoryState(result.copy(activeProjectId = _state.value.activeProjectId))
                        }
                        return@withRepository true
                    } catch (_: RetryCatalogPublication) {
                        // Recompute the project manifest with the current owner.
                    } catch (_: CatalogPublicationStopped) {
                        return@withRepository false
                    }
                }
                @Suppress("UNREACHABLE_CODE") false
            } finally {
                gate.finishCatalogWrite(token)
            }
    }

    suspend fun setSessionArchived(projectId: String?, sessionId: String, archived: Boolean) {
        withRepository {
            publishRepositoryState(repository.setSessionArchived(projectId, sessionId, archived))
            if (archived) com.lingxi.code.cron.AndroidCronRepository.get(appContext)
                .pauseForArchivedSession(projectId ?: com.lingxi.code.cron.GLOBAL_CRON_SCOPE_ID, sessionId)
        }
    }

    suspend fun markActiveSession(projectId: String, sessionId: String) {
        withRepository {
            publishRepositoryState(
                repository.markActiveSession(projectId, sessionId),
            )
        }
    }

    fun reimport(projectId: String) {
        viewModelScope.launch {
            runOperation(
                ProjectOperationKind.Reimport,
                projectId,
                com.lingxi.code.theme.LocaleWrapper.getString(appContext, R.string.project_reimporting_message),
                scopeChange = true,
            ) {
                val result = synchronizer.reimport(projectId)
                loadValidatedRepository().copy(conflicts = result.conflicts)
            }
        }
    }

    fun reauthorize(projectId: String, treeUri: Uri) {
        viewModelScope.launch {
            runOperation(
                ProjectOperationKind.Reimport,
                projectId,
                com.lingxi.code.theme.LocaleWrapper.getString(appContext, R.string.project_reauthorizing_message),
                scopeChange = true,
            ) {
                persistTreePermission(treeUri)
                val current = repository.project(projectId)
                repository.updateProject(
                    current.record.copy(
                        sourceTreeUri = treeUri.toString(),
                        sourceDisplayName = treeDisplayName(treeUri),
                        syncState = ProjectSyncState.ChangesPending,
                    ),
                )
                val result = synchronizer.reimport(projectId)
                loadValidatedRepository().copy(conflicts = result.conflicts)
            }
        }
    }

    fun export(projectId: String) {
        viewModelScope.launch {
            runOperation(
                ProjectOperationKind.Export,
                projectId,
                com.lingxi.code.theme.LocaleWrapper.getString(appContext, R.string.project_exporting_message),
            ) {
                val result = synchronizer.export(projectId)
                loadValidatedRepository().copy(conflicts = result.conflicts)
            }
        }
    }

    fun resolveConflicts(projectId: String, resolution: ConflictResolution) {
        viewModelScope.launch {
            runOperation(
                ProjectOperationKind.ResolveConflicts,
                projectId,
                com.lingxi.code.theme.LocaleWrapper.getString(appContext, R.string.project_resolving_conflicts_message),
                scopeChange = true,
            ) {
                val result = synchronizer.resolve(projectId, resolution)
                loadValidatedRepository().copy(conflicts = result.conflicts)
            }
        }
    }

    fun clearError() {
        _state.value = _state.value.copy(errorMessage = null)
    }

    fun reportOwnerConflict(sessionId: String, indexed: ScopeStateStore.SessionOwner, recorded: ScopeStateStore.SessionOwner?) {
        val scopes = listOfNotNull(indexed.scopeKey, recorded?.scopeKey).distinct().sorted()
        publishRepositoryFailure(null, ProjectCatalogUnavailableException(scopes,
            "Session $sessionId has conflicting recorded owners: ${scopes.joinToString()}"))
    }

    fun reportOwnerStateFailure(failure: Exception, scope: String = "owner-registry") {
        synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
            val scopes = ((_state.value.unavailableScopes.takeIf { _state.value.storageUnavailable }.orEmpty()) + scope)
                .distinct().sorted()
            publishRepositoryFailure(null, ProjectCatalogUnavailableException(scopes,
                "Conversation state is unavailable for $scope: ${failure.message.orEmpty()}", failure))
        }
    }

    private fun requireMatchingRuntimeSource(id: String, scope: String, mode: com.lingxi.code.model.SessionMode?, path: String?) {
        val owner = com.lingxi.code.conversation.SessionAdmissionGate.conflictingRuntimeIdentity(id, scope, mode, path) ?: return
        val scopes = listOf(scope, owner.workspace).distinct().sorted()
        throw ProjectCatalogUnavailableException(scopes,
            "Session $id conflicts with an active owner in ${scopes.joinToString()}")
    }

    private fun publishRepositoryFailure(recovered: ProjectStoreState?, failure: Throwable) {
        synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
            _state.value = repositoryFailureState(_state.value, recovered,
                failure.message ?: failure::class.simpleName,
                (failure as? ProjectCatalogUnavailableException)?.scopes.orEmpty())
        }
    }

    private fun publishRepositoryState(next: ProjectStoreState) {
        _state.value = next.copy(
            conflicts = _state.value.conflicts,
            operation = _state.value.operation,
            errorMessage = _state.value.errorMessage,
        )
    }

    private suspend fun readRecordedOwners(hasIndexedOwners: Boolean = false): Map<String, ScopeStateStore.SessionOwner> = try {
        val present = ownerStateFile.exists() || java.nio.file.Files.isSymbolicLink(ownerStateFile.toPath())
        requireOwnerRegistryPresent(present, ownerRegistryObserved, hasIndexedOwners)
        ownerStore.readSessionOwners().also { if (present) ownerRegistryObserved = true }
    } catch (cancelled: kotlinx.coroutines.CancellationException) {
        throw cancelled
    } catch (error: ProjectCatalogUnavailableException) {
        throw error
    } catch (error: Exception) {
        throw ProjectCatalogUnavailableException(listOf("owner-registry"), "Conversation owner registry is unavailable", error)
    }

    private suspend fun loadValidatedRepository(synchronizeSdk: Boolean = true): ProjectStoreState {
        val scanEpoch = com.lingxi.code.conversation.SessionAdmissionGate.scanEpoch()
        if (com.lingxi.code.conversation.SessionAdmissionGate.hasBarrierFailure()) {
            throw ProjectCatalogUnavailableException(listOf("scheduled"), "Scheduled owner admission could not be verified")
        }
        try {
            com.lingxi.code.conversation.ConversationServiceOwners.initialize(appContext)
            com.lingxi.code.conversation.ConversationServiceOwners.persisted(appContext)
        } catch (failure: Exception) {
            throw ProjectCatalogUnavailableException(listOf("conversation-recovery"), "Conversation recovery state is unavailable", failure)
        }
        // Do not treat a deleted pair of conversation indexes as a fresh
        // installation when the scheduler still has durable owner evidence.
        val preflightSdk = if (repository.bothSessionIndexesMissing()) {
            try {
                val local = com.lingxi.code.cron.CronRunHistoryStore(File(appContext.filesDir, "cron/history")).validatedOwnerReceipts()
                val sdk = if (synchronizeSdk) {
                    val projectIds = try { repository.knownProjectIds() } catch (failure: Exception) {
                        throw ProjectCatalogUnavailableException(listOf("projects-index"), "Project index is unavailable", failure)
                    }
                    com.lingxi.code.cron.AndroidCronRepository.get(appContext).strictOwnerReceipts(projectIds)
                } else emptyList()
                if (local.isNotEmpty() || sdk.isNotEmpty()) {
                    throw ProjectCatalogUnavailableException(listOf("global", "projects-index", "scheduled"),
                        "Conversation indexes are missing while scheduled owners exist")
                }
                sdk
            } catch (cancelled: kotlinx.coroutines.CancellationException) {
                throw cancelled
            } catch (failure: ProjectCatalogUnavailableException) {
                throw failure
            } catch (failure: Exception) {
                throw ProjectCatalogUnavailableException(listOf("scheduled"), "Scheduled session index is unavailable", failure)
            }
        } else null
        val state = repository.load()
        val indexed = repository.indexedSessionOwners(state.globalSessions)
        val recorded = readRecordedOwners(hasIndexedOwners = indexed.isNotEmpty())
        requireMatchingSessionOwners(indexed, recorded)
        val receipts = try {
            val discovered = if (synchronizeSdk)
                preflightSdk ?: com.lingxi.code.cron.AndroidCronRepository.get(appContext).strictOwnerReceipts(repository.knownProjectIds())
            else emptyList()
            val paths = buildMap {
                state.globalSessions.forEach { row -> row.transcriptPath?.let { put(canonicalSessionId(row.sessionId), it) } }
                state.projects.forEach { project -> project.sessions.forEach { row ->
                    row.transcriptPath?.let { put(canonicalSessionId(row.sessionId), it) }
                } }
            }
            ownerStore.reconcileCronReceipts(discovered, indexed, paths, state.projects.mapTo(mutableSetOf()) { it.record.id })
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (failure: ProjectCatalogUnavailableException) {
            throw failure
        } catch (failure: Exception) {
            throw ProjectCatalogUnavailableException(listOf("scheduled"), "Scheduled session index is unavailable", failure)
        }
        recorded.filterValues { it.scopeKey == "scheduled" }.forEach { (id, owner) ->
            if (receipts.none { canonicalSessionId(checkNotNull(it.sessionId)) == id &&
                    it.sessionScopeKey == owner.scopeKey && it.sessionMode == owner.mode.wireValue }) {
                throw ProjectCatalogUnavailableException(listOf("scheduled"), "Scheduled session $id is missing its owner receipt")
            }
        }
        receipts.forEach { run ->
            val id = canonicalSessionId(checkNotNull(run.sessionId))
            val owner = ScopeStateStore.SessionOwner(checkNotNull(run.sessionScopeKey),
                com.lingxi.code.model.sessionModeFromWireValue(checkNotNull(run.sessionMode)))
            val known = indexed[id] ?: recorded[id]
            if (known != null && known != owner) {
                throw ProjectCatalogUnavailableException(listOf("scheduled", owner.scopeKey, known.scopeKey).distinct().sorted(),
                    "Session $id has conflicting scheduled and recorded owners")
            }
            val indexedPath = when (val scope = com.lingxi.code.model.conversationScopeFromKey(owner.scopeKey)) {
                com.lingxi.code.model.ConversationScope.Global -> state.globalSessions.firstOrNull { canonicalSessionId(it.sessionId) == id }?.transcriptPath
                is com.lingxi.code.model.ConversationScope.Project -> state.projects.firstOrNull { it.record.id == scope.projectId }
                    ?.sessions?.firstOrNull { canonicalSessionId(it.sessionId) == id }?.transcriptPath
                else -> null
            }
            if (indexedPath != null && indexedPath != run.sessionTranscriptPath) {
                throw ProjectCatalogUnavailableException(listOf("scheduled", owner.scopeKey).distinct().sorted(),
                    "Session $id has conflicting transcript identities")
            }
        }
        if (synchronizeSdk) com.lingxi.code.conversation.SessionAdmissionGate.completeOwnerScan(scanEpoch)
        return state
    }

    /** A failed validation must fence new owners even when a live refresh found it. */
    private suspend fun <T> withRepository(block: suspend () -> T): T = withContext(Dispatchers.IO) {
        repositoryMutex.withLock {
            try {
                loadValidatedRepository()
                block()
            } catch (cancelled: kotlinx.coroutines.CancellationException) {
                throw cancelled
            } catch (error: Throwable) {
                val reload = runCatching { loadValidatedRepository() }
                val failure = reload.exceptionOrNull() ?: error
                publishRepositoryFailure(reload.getOrNull(), failure)
                throw error
            }
        }
    }

    private suspend fun <T> withScopeChange(block: suspend () -> T): T {
        val token = com.lingxi.code.conversation.SessionAdmissionGate.awaitScopeChange()
        var validated = false
        try {
            return block().also { validated = true }
        } finally {
            com.lingxi.code.conversation.SessionAdmissionGate.finishScopeChange(token, validated)
        }
    }

    private suspend fun <T> runSuspendingOperation(
        kind: ProjectOperationKind,
        projectId: String? = null,
        message: String,
        block: suspend () -> T,
    ): T {
        _state.value = _state.value.copy(
            operation = ProjectOperation(kind, projectId, message),
            errorMessage = null,
        )
        return try {
            withContext(Dispatchers.IO) {
                repositoryMutex.withLock { block() }
            }
        } catch (t: Throwable) {
            if (t is kotlinx.coroutines.CancellationException) throw t
            val reload = try {
                Result.success(withContext(Dispatchers.IO) { repositoryMutex.withLock { loadValidatedRepository() } })
            } catch (cancelled: kotlinx.coroutines.CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                Result.failure(error)
            }
            val failure = reload.exceptionOrNull() ?: t
            publishRepositoryFailure(reload.getOrNull(), failure)
            throw t
        } finally {
            _state.value = _state.value.copy(operation = null)
        }
    }

    private suspend fun runOperation(
        kind: ProjectOperationKind,
        projectId: String? = null,
        message: String,
        scopeChange: Boolean = false,
        block: suspend () -> ProjectStoreState,
    ) {
        runCatching {
            if (scopeChange) withScopeChange { runSuspendingOperation(kind, projectId, message, block) }
            else runSuspendingOperation(kind, projectId, message, block)
        }.onSuccess { _state.value = it }
    }

    private fun persistTreePermission(treeUri: Uri) {
        val readWrite = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        runCatching {
            appContext.contentResolver.takePersistableUriPermission(treeUri, readWrite)
        }.recoverCatching {
            appContext.contentResolver.takePersistableUriPermission(
                treeUri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION,
            )
        }.getOrThrow()
    }

    private fun treeDisplayName(treeUri: Uri): String? {
        val documentId = runCatching { DocumentsContract.getTreeDocumentId(treeUri) }.getOrNull()
            ?: return null
        val root = DocumentsContract.buildDocumentUriUsingTree(treeUri, documentId)
        val cursor = appContext.contentResolver.query(
            root,
            arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME),
            null,
            null,
            null,
        ) ?: return null
        return cursor.use { if (it.moveToFirst()) it.getString(0) else null }
    }

    companion object {
        internal val admissionRepositoryMutex = Mutex()
        fun factory(context: Context): ViewModelProvider.Factory {
            val appContext = context.applicationContext
            return object : ViewModelProvider.Factory {
                @Suppress("UNCHECKED_CAST")
                override fun <T : ViewModel> create(modelClass: Class<T>): T {
                    val paths = MobileUserStorage.directories(appContext)
                    val repository = ProjectRepository(
                        File(paths.projects),
                        userHome = File(paths.home),
                        projectsGuestRoot = paths.projectsGuestPath,
                        userHomeGuestPath = paths.guestHome,
                        ownerStateFile = File(appContext.filesDir, ScopeStateStore.SCOPE_STATE_FILE),
                        strings = projectStrings(appContext),
                    )
                    return ProjectStore(
                        repository = repository,
                        synchronizer = SafProjectSynchronizer(appContext, repository),
                        appContext = appContext,
                    ) as T
                }
            }
        }
    }
}
