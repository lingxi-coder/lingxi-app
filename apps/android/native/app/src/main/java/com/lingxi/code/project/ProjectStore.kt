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

class ProjectStore private constructor(
    private val repository: ProjectRepository,
    private val synchronizer: SafProjectSynchronizer,
    private val appContext: Context,
) : ViewModel() {
    private val _state = MutableStateFlow(ProjectStoreState())
    val state: StateFlow<ProjectStoreState> = _state.asStateFlow()
    private val repositoryMutex = Mutex()
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
        }
    }

    suspend fun createInternal(name: String, mode: com.lingxi.code.model.SessionMode): ProjectSnapshot =
        runSuspendingOperation(
            ProjectOperationKind.Create,
            message = com.lingxi.code.theme.LocaleWrapper.getString(appContext, R.string.project_creating_message),
        ) {
            loadValidatedRepository()
            val created = repository.createInternal(name, mode)
            _state.value = loadValidatedRepository()
            created
        }

    suspend fun registerAuthoringWorkspace(name: String, directory: String, mode: com.lingxi.code.model.SessionMode): ProjectSnapshot =
        runSuspendingOperation(ProjectOperationKind.Create, message = appContext.getString(R.string.project_creating_message)) {
            loadValidatedRepository()
            repository.registerAuthoringWorkspace(name, directory, mode).also { _state.value = loadValidatedRepository() }
        }

    suspend fun importSaf(name: String, treeUri: Uri, mode: com.lingxi.code.model.SessionMode): ProjectSnapshot =
        runSuspendingOperation(
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
        }

    /**
     * Persist the destination before ChatViewModel commits its replacement
     * Source. This is intentionally non-observable: [publishActive] runs in the
     * same Source transaction, so Compose never sees a Project/Engine mismatch.
     */
    suspend fun persistActive(projectId: String?): ProjectStoreState =
        withRepository {
            synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
                check(!_state.value.storageUnavailable) { "Conversation storage is unavailable" }
                repository.setActiveProject(projectId)
            }
        }

    suspend fun validateOwnerIndexes() = withRepository {
        publishRepositoryState(loadValidatedRepository())
    }

    /** Publish the already-durable Project state without another I/O race. */
    fun publishActive(persisted: ProjectStoreState, activate: () -> Unit): Boolean =
        synchronized(com.lingxi.code.conversation.SessionAdmissionGate.lock) {
            if (_state.value.storageUnavailable) false else {
                _state.value = persisted.copy(
                    conflicts = _state.value.conflicts,
                    operation = _state.value.operation,
                    errorMessage = null,
                )
                activate()
                true
            }
        }

    suspend fun addVisibleMode(projectId: String, mode: com.lingxi.code.model.SessionMode) = withRepository {
        repository.addVisibleMode(projectId, mode)
        publishRepositoryState(loadValidatedRepository())
    }

    suspend fun recordWorkspace(projectId: String, state: SessionWorkspaceState): Unit = withRepository {
        _state.value = repository.recordWorkspace(projectId, state)
    }

    suspend fun syncEngineSessions(projectId: String?, rows: List<SessionRow>): ProjectStoreState =
        withRepository {
            requireMatchingSessionOwners(rows.associate {
                canonicalSessionId(it.uuid) to ScopeStateStore.SessionOwner(projectId?.let { id -> "project.$id" } ?: "global", it.mode)
            }, readRecordedOwners())
            val timestamp = System.currentTimeMillis()
            repository.updateSessions(
                projectId,
                rows.mapIndexed { index, row ->
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
                },
            ).also(::publishRepositoryState)
        }

    suspend fun recordStartedSession(
        projectId: String?,
        sessionId: String,
        title: String,
        mode: com.lingxi.code.model.SessionMode,
        initialMessageCount: Int = 0,
    ) {
        withRepository {
            requireMatchingSessionOwners(mapOf(canonicalSessionId(sessionId) to
                ScopeStateStore.SessionOwner(projectId?.let { "project.$it" } ?: "global", mode)), readRecordedOwners())
            publishRepositoryState(
                repository.recordStartedSession(projectId, sessionId, title, mode, initialMessageCount),
            )
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

    private suspend fun loadValidatedRepository(): ProjectStoreState {
        val state = repository.load()
        val indexed = repository.indexedSessionOwners(state.globalSessions)
        requireMatchingSessionOwners(indexed, readRecordedOwners(hasIndexedOwners = indexed.isNotEmpty()))
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
        block: suspend () -> ProjectStoreState,
    ) {
        runCatching {
            runSuspendingOperation(kind, projectId, message, block)
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
