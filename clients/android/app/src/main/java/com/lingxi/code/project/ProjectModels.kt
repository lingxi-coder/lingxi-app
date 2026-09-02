package com.lingxi.code.project

import com.lingxi.code.model.SessionMode
import java.io.File

enum class ProjectStorageKind(val wireName: String) {
    Internal("internal"),
    SafMirror("saf-mirror");

    companion object {
        fun fromWireName(value: String): ProjectStorageKind =
            entries.firstOrNull { it.wireName == value }
                ?: throw IllegalArgumentException("unknown project storage kind: $value")
    }
}

enum class ProjectSyncState(val label: String) {
    LocalOnly("本机"),
    Synced("已同步"),
    ChangesPending("有待同步更改"),
    Conflict("存在同步冲突"),
    AuthorizationLost("外部目录授权失效"),
    Syncing("同步中"),
    Error("同步失败"),
}

data class ProjectRecord(
    val id: String,
    val name: String,
    val storageKind: ProjectStorageKind,
    val createdAtEpochMillis: Long,
    val updatedAtEpochMillis: Long,
    val sourceTreeUri: String? = null,
    val sourceDisplayName: String? = null,
    val lastSyncAtEpochMillis: Long? = null,
    val lastActiveSessionId: String? = null,
    val syncState: ProjectSyncState =
        if (storageKind == ProjectStorageKind.Internal) {
            ProjectSyncState.LocalOnly
        } else {
            ProjectSyncState.ChangesPending
        },
)

data class ProjectWorkspace(
    val projectId: String,
    val hostPath: String,
    val guestPath: String = "/workspace/$projectId",
) {
    val hostDirectory: File get() = File(hostPath)
}

data class ProjectSessionSummary(
    val sessionId: String,
    val title: String,
    val messageCount: Int,
    val relativeTime: String,
    val updatedAtEpochMillis: Long,
    val mode: SessionMode = SessionMode.Code,
)

data class ProjectSyncFile(
    val relativePath: String,
    val sha256: String,
    val sizeBytes: Long,
)

data class ProjectSyncBaseline(
    val files: Map<String, ProjectSyncFile> = emptyMap(),
)

data class ProjectSyncConflict(
    val projectId: String,
    val relativePath: String,
    val internalSha256: String?,
    val externalSha256: String?,
    val baselineSha256: String?,
)

data class ProjectSnapshot(
    val record: ProjectRecord,
    val workspace: ProjectWorkspace,
    val sessions: List<ProjectSessionSummary>,
)

enum class ProjectOperationKind {
    Create,
    Import,
    Switch,
    RefreshSessions,
    Reimport,
    Export,
    ResolveConflicts,
}

data class ProjectOperation(
    val kind: ProjectOperationKind,
    val projectId: String? = null,
    val message: String,
)

data class ProjectStoreState(
    val projects: List<ProjectSnapshot> = emptyList(),
    val activeProjectId: String? = null,
    val globalSessions: List<ProjectSessionSummary> = emptyList(),
    val conflicts: List<ProjectSyncConflict> = emptyList(),
    val loading: Boolean = true,
    val operation: ProjectOperation? = null,
    val errorMessage: String? = null,
) {
    val activeProject: ProjectSnapshot?
        get() = projects.firstOrNull { it.record.id == activeProjectId }
}

enum class ConflictResolution {
    KeepInternal,
    KeepExternal,
}
