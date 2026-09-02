package com.lingxi.code.project

import androidx.compose.ui.graphics.Color
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

fun ProjectSnapshot.toDrawerProject(): Project {
    val color = ProjectColors[(record.id.hashCode() and Int.MAX_VALUE) % ProjectColors.size]
    val storage = when (record.storageKind) {
        ProjectStorageKind.Internal -> "本机"
        ProjectStorageKind.SafMirror -> record.sourceDisplayName?.let { "外部镜像 · $it" } ?: "外部镜像"
    }
    return Project(
        id = record.id,
        wsId = LocalProjectWorkspace.id,
        name = record.name,
        icon = if (record.storageKind == ProjectStorageKind.Internal) "◇" else "◫",
        color = color,
        desc = "$storage · ${record.syncState.label}",
        storageKind = record.storageKind.wireName,
        syncState = record.syncState.label,
        updatedAtEpochMillis = record.updatedAtEpochMillis,
        sessions = sessions.map { session ->
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
