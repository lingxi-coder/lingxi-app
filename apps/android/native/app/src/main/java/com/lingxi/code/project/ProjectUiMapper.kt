package com.lingxi.code.project

import androidx.compose.ui.graphics.Color
import com.lingxi.code.R
import com.lingxi.code.conversation.ConversationStrings
import com.lingxi.code.conversation.DefaultConversationStrings
import com.lingxi.code.model.Project
import com.lingxi.code.model.ProjectSession
import com.lingxi.code.model.Workspace

private val ProjectColors = listOf(
    Color(red = 0.4340f, green = 0.5865f, blue = 1.0000f),
    Color(red = 0.0000f, green = 0.7601f, blue = 0.7664f),
    Color(red = 0.8090f, green = 0.4552f, blue = 0.8891f),
    Color(red = 0.8696f, green = 0.5765f, blue = 0.0000f),
)

val LocalProjectWorkspace = Workspace(
    id = "android-local",
    name = "本机",
    icon = "◐",
    color = ProjectColors.first(),
)

fun ProjectSnapshot.toDrawerProject(strings: ConversationStrings = DefaultConversationStrings): Project {
    val color = ProjectColors[(record.id.hashCode() and Int.MAX_VALUE) % ProjectColors.size]
    val storage = when (record.storageKind) {
        ProjectStorageKind.Internal -> strings.resolve(R.string.project_storage_local, "本机")
        ProjectStorageKind.SafMirror -> record.sourceDisplayName?.let { strings.resolve(R.string.project_storage_external_fmt, "外部镜像 · %1\$s", it) } ?: strings.resolve(R.string.project_storage_external, "外部镜像")
    }
    val syncLabel = strings.resolve(when (record.syncState) {
        ProjectSyncState.LocalOnly -> R.string.project_sync_state_local
        ProjectSyncState.Synced -> R.string.project_sync_state_synced
        ProjectSyncState.ChangesPending -> R.string.project_sync_state_pending
        ProjectSyncState.Conflict -> R.string.project_sync_state_conflict
        ProjectSyncState.AuthorizationLost -> R.string.project_sync_state_auth_lost
        ProjectSyncState.Syncing -> R.string.project_sync_state_syncing
        ProjectSyncState.Error -> R.string.project_sync_state_error
    }, record.syncState.label)
    return Project(
        id = record.id,
        wsId = LocalProjectWorkspace.id,
        name = record.name,
        icon = if (record.storageKind == ProjectStorageKind.Internal) "◇" else "◫",
        color = color,
        desc = "$storage · $syncLabel",
        storageKind = record.storageKind.wireName,
        syncState = record.syncState.label,
        updatedAtEpochMillis = record.updatedAtEpochMillis,
        sessions = sessions.filterNot { it.isArchived }.map { session ->
            ProjectSession(
                id = session.sessionId,
                title = session.title,
                activity = session.relativeTime,
                preview = "",
                msgs = session.messageCount,
                mode = session.mode,
                updatedAtEpochMillis = session.updatedAtEpochMillis,
            )
        },
    )
}
