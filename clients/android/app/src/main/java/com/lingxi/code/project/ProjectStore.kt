package com.lingxi.code.project

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.lingxi.code.model.SessionRow
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

    init {
        reload()
    }

    fun reload() {
        viewModelScope.launch {
            runOperation(ProjectOperationKind.RefreshSessions, message = "正在读取项目…") {
                repository.load()
            }
        }
    }

    suspend fun createInternal(name: String): ProjectSnapshot =
        runSuspendingOperation(ProjectOperationKind.Create, message = "正在创建项目…") {
            val created = repository.createInternal(name)
            _state.value = repository.load()
            created
        }

    suspend fun importSaf(name: String, treeUri: Uri): ProjectSnapshot =
        runSuspendingOperation(ProjectOperationKind.Import, message = "正在导入外部目录…") {
            persistTreePermission(treeUri)
            val created = repository.createSafMirror(
                name = name,
                sourceTreeUri = treeUri.toString(),
                sourceDisplayName = treeDisplayName(treeUri),
            )
            val result = synchronizer.importInitial(created.record.id)
            _state.value = repository.load().copy(conflicts = result.conflicts)
            result.project
        }

    /**
     * Persist the destination before ChatViewModel commits its replacement
     * Source. This is intentionally non-observable: [publishActive] runs in the
     * same Source transaction, so Compose never sees a Project/Engine mismatch.
     */
    suspend fun persistActive(projectId: String?): ProjectStoreState =
        withContext(Dispatchers.IO) {
            repositoryMutex.withLock {
                repository.setActiveProject(projectId)
            }
        }

    /** Publish the already-durable Project state without another I/O race. */
    fun publishActive(persisted: ProjectStoreState) {
        _state.value = persisted.copy(
            conflicts = _state.value.conflicts,
            operation = _state.value.operation,
            errorMessage = null,
        )
    }

    suspend fun syncEngineSessions(projectId: String?, rows: List<SessionRow>): ProjectStoreState =
        withContext(Dispatchers.IO) {
            repositoryMutex.withLock {
                val timestamp = System.currentTimeMillis()
                repository.updateSessions(
                    projectId,
                    rows.mapIndexed { index, row ->
                        ProjectSessionSummary(
                            sessionId = row.uuid,
                            title = row.title,
                            messageCount = row.messageCount,
                            relativeTime = row.relativeTime,
                            updatedAtEpochMillis = timestamp - index,
                        )
                    },
                ).also(::publishRepositoryState)
            }
        }

    suspend fun recordStartedSession(projectId: String?, sessionId: String, title: String) {
        withContext(Dispatchers.IO) {
            repositoryMutex.withLock {
                publishRepositoryState(
                    repository.recordStartedSession(projectId, sessionId, title),
                )
            }
        }
    }

    suspend fun markActiveSession(projectId: String, sessionId: String) {
        withContext(Dispatchers.IO) {
            repositoryMutex.withLock {
                publishRepositoryState(
                    repository.markActiveSession(projectId, sessionId),
                )
            }
        }
    }

    fun reimport(projectId: String) {
        viewModelScope.launch {
            runOperation(ProjectOperationKind.Reimport, projectId, "正在从外部目录重新导入…") {
                val result = synchronizer.reimport(projectId)
                repository.load().copy(conflicts = result.conflicts)
            }
        }
    }

    fun reauthorize(projectId: String, treeUri: Uri) {
        viewModelScope.launch {
            runOperation(ProjectOperationKind.Reimport, projectId, "正在重新授权并导入…") {
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
                repository.load().copy(conflicts = result.conflicts)
            }
        }
    }

    fun export(projectId: String) {
        viewModelScope.launch {
            runOperation(ProjectOperationKind.Export, projectId, "正在同步回外部目录…") {
                val result = synchronizer.export(projectId)
                repository.load().copy(conflicts = result.conflicts)
            }
        }
    }

    fun resolveConflicts(projectId: String, resolution: ConflictResolution) {
        viewModelScope.launch {
            runOperation(ProjectOperationKind.ResolveConflicts, projectId, "正在解决同步冲突…") {
                val result = synchronizer.resolve(projectId, resolution)
                repository.load().copy(conflicts = result.conflicts)
            }
        }
    }

    fun clearError() {
        _state.value = _state.value.copy(errorMessage = null)
    }

    private fun publishRepositoryState(next: ProjectStoreState) {
        _state.value = next.copy(
            conflicts = _state.value.conflicts,
            operation = _state.value.operation,
            errorMessage = _state.value.errorMessage,
        )
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
            val recovered = withContext(Dispatchers.IO) {
                repositoryMutex.withLock { repository.load() }
            }
            _state.value = recovered.copy(
                errorMessage = t.message ?: t::class.simpleName,
            )
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
                    val repository = ProjectRepository(File(appContext.filesDir, "projects"))
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
