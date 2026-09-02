package com.lingxi.code.drawer

import androidx.compose.runtime.Immutable
import com.lingxi.code.localapps.LocalAppSessionRow
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.Cron
import com.lingxi.code.model.Project
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.SessionRow

enum class WorkspaceGroupKind {
    Global,
    Project,
    LocalApp,
}

@Immutable
data class DrawerLocalAppWorkspace(
    val appId: String,
    val name: String,
    val status: String? = null,
    val updatedAtEpochSeconds: Long = 0L,
    val sessions: List<LocalAppSessionRow> = emptyList(),
)

@Immutable
data class WorkspaceGroup(
    val stableKey: String,
    val scope: ConversationScope,
    val kind: WorkspaceGroupKind,
    val name: String,
    val status: String? = null,
    val pinnedAtEpochSeconds: Long? = null,
    val workspaceUpdatedAtEpochSeconds: Long = 0L,
    val sessions: List<SessionRow> = emptyList(),
) {
    val latestSessionEpochSeconds: Long
        get() = sessions.asSequence().mapNotNull(SessionRow::modifiedAtEpochSeconds).maxOrNull() ?: 0L
}

@Immutable
data class CronWorkspaceGroup(
    val stableKey: String,
    val scope: ConversationScope,
    val kind: WorkspaceGroupKind,
    val name: String,
    val workspaceUpdatedAtEpochSeconds: Long = 0L,
    val crons: List<Cron> = emptyList(),
)

internal fun buildWorkspaceGroups(
    mode: SessionMode,
    globalName: String,
    globalSessions: List<SessionRow>,
    projects: List<Project>,
    localApps: List<DrawerLocalAppWorkspace>,
    pinnedAtEpochMillis: (String) -> Long? = { null },
): List<WorkspaceGroup> = sortWorkspaceGroups(
    buildList {
        add(
            WorkspaceGroup(
                stableKey = "global",
                scope = ConversationScope.Global,
                kind = WorkspaceGroupKind.Global,
                name = globalName,
                pinnedAtEpochSeconds = pinnedAtEpochMillis("global"),
                sessions = globalSessions.filter { it.mode == mode },
            ),
        )
        projects.forEach { project ->
            val stableKey = "project.${project.id}"
            add(
                WorkspaceGroup(
                    stableKey = stableKey,
                scope = ConversationScope.Project(project.id),
                kind = WorkspaceGroupKind.Project,
                name = project.name,
                status = project.desc.ifBlank { null },
                pinnedAtEpochSeconds = pinnedAtEpochMillis(stableKey),
                workspaceUpdatedAtEpochSeconds = project.updatedAtEpochMillis / 1000L,
                sessions = project.sessions.map { session ->
                        SessionRow(
                            uuid = session.id,
                            title = session.title,
                            messageCount = session.msgs,
                            mode = session.mode,
                            modifiedAtEpochSeconds = session.updatedAtEpochMillis.takeIf { it > 0L }?.div(1000L),
                            relativeTime = session.activity,
                        )
                    }.filter { it.mode == mode },
                ),
            )
        }
        localApps.forEach { app ->
            val stableKey = "app.${app.appId}"
            add(
                WorkspaceGroup(
                    stableKey = stableKey,
                    scope = ConversationScope.LocalApp(app.appId),
                    kind = WorkspaceGroupKind.LocalApp,
                    name = app.name,
                    status = app.status,
                    pinnedAtEpochSeconds = pinnedAtEpochMillis(stableKey),
                    workspaceUpdatedAtEpochSeconds = app.updatedAtEpochSeconds,
                    sessions = app.sessions.map { session ->
                        SessionRow(
                            uuid = session.uuid,
                            title = session.title,
                            messageCount = session.messageCount,
                            mode = session.mode,
                            modifiedAtEpochSeconds = session.modifiedAtEpochSeconds,
                            relativeTime = session.relativeTime,
                        )
                    }.filter { it.mode == mode },
                ),
            )
        }
    },
)

internal fun buildCronWorkspaceGroups(
    globalName: String,
    crons: List<Cron>,
    projects: List<Project>,
): List<CronWorkspaceGroup> {
    val byWorkspace = crons.groupBy(Cron::wsId)
    return sortCronWorkspaceGroups(
        buildList {
            add(
                CronWorkspaceGroup(
                    stableKey = "global",
                    scope = ConversationScope.Global,
                    kind = WorkspaceGroupKind.Global,
                    name = globalName,
                    crons = byWorkspace["global"].orEmpty(),
                ),
            )
            projects.forEach { project ->
                add(
                    CronWorkspaceGroup(
                        stableKey = "project.${project.id}",
                        scope = ConversationScope.Project(project.id),
                        kind = WorkspaceGroupKind.Project,
                        name = project.name,
                        crons = byWorkspace[project.wsId].orEmpty(),
                    ),
                )
            }
        },
    )
}

internal fun sortWorkspaceGroups(groups: List<WorkspaceGroup>): List<WorkspaceGroup> =
    groups.sortedWith(
        compareByDescending<WorkspaceGroup> { it.kind == WorkspaceGroupKind.Global }
            .thenByDescending { it.pinnedAtEpochSeconds ?: Long.MIN_VALUE }
            .thenByDescending { if (it.latestSessionEpochSeconds > 0L) it.latestSessionEpochSeconds else it.workspaceUpdatedAtEpochSeconds }
            .thenBy(String.CASE_INSENSITIVE_ORDER) { it.name }
            .thenBy { it.stableKey },
    )

internal fun sortCronWorkspaceGroups(groups: List<CronWorkspaceGroup>): List<CronWorkspaceGroup> =
    groups.sortedWith(
        compareByDescending<CronWorkspaceGroup> { it.kind == WorkspaceGroupKind.Global }
            .thenByDescending { it.workspaceUpdatedAtEpochSeconds }
            .thenBy(String.CASE_INSENSITIVE_ORDER) { it.name }
            .thenBy { it.stableKey },
    )

internal fun filterWorkspaceGroups(groups: List<WorkspaceGroup>, query: String): List<WorkspaceGroup> {
    val normalized = query.trim().lowercase()
    if (normalized.isEmpty()) return groups
    return groups.mapNotNull { group ->
        val workspaceMatches = group.name.lowercase().contains(normalized)
        val matchingSessions = group.sessions.filter { session ->
            session.title.lowercase().contains(normalized)
        }
        when {
            workspaceMatches -> group
            matchingSessions.isNotEmpty() -> group.copy(sessions = matchingSessions)
            else -> null
        }
    }
}

internal fun filterCronWorkspaceGroups(groups: List<CronWorkspaceGroup>, query: String): List<CronWorkspaceGroup> {
    val normalized = query.trim().lowercase()
    if (normalized.isEmpty()) return groups
    return groups.mapNotNull { group ->
        val workspaceMatches = group.name.lowercase().contains(normalized)
        val matchingCrons = group.crons.filter { cron ->
            cron.title.lowercase().contains(normalized) ||
                cron.desc.lowercase().contains(normalized) ||
                cron.cron.lowercase().contains(normalized)
        }
        when {
            workspaceMatches -> group
            matchingCrons.isNotEmpty() -> group.copy(crons = matchingCrons)
            else -> null
        }
    }
}
