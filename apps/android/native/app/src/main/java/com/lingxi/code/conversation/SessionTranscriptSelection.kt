package com.lingxi.code.conversation

import com.lingxi.code.cron.CronRunRecord
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.model.persistenceKey
import com.lingxi.code.project.ProjectStoreState

/** Select only the catalog entry owned by this logical scope and fixed mode. */
internal fun sessionTranscriptPath(
    scope: ConversationScope,
    sessionId: String,
    mode: SessionMode,
    projects: ProjectStoreState,
    scheduled: List<CronRunRecord>,
): String? {
    if (projects.loading || projects.storageUnavailable) return null
    val id = canonicalSessionId(sessionId)
    val claims = buildList {
        projects.globalSessions.filter { canonicalSessionId(it.sessionId) == id }
            .forEach { add("global" to it.mode) }
        projects.projects.forEach { project ->
            project.sessions.filter { canonicalSessionId(it.sessionId) == id }
                .forEach { add("project.${project.record.id}" to it.mode) }
        }
        scheduled.filter { it.sessionScopeKey == "scheduled" && it.sessionId?.let(::canonicalSessionId) == id }
            .forEach { run -> SessionMode.entries.firstOrNull { it.wireValue == run.sessionMode }?.let { add("scheduled" to it) } }
    }
    if (claims.any { it != (scope.persistenceKey() to mode) }) return null
    val candidates = when (scope) {
        ConversationScope.Global -> projects.globalSessions
        is ConversationScope.Project -> projects.projects.firstOrNull { it.record.id == scope.projectId }?.sessions.orEmpty()
        ConversationScope.Scheduled -> emptyList()
    }.filter { canonicalSessionId(it.sessionId) == id && it.mode == mode }
        .mapNotNull { it.transcriptPath?.takeIf(String::isNotBlank) }
    val scheduledPaths = if (scope == ConversationScope.Scheduled) scheduled.filter {
        it.sessionId?.let(::canonicalSessionId) == id && it.sessionMode == mode.wireValue &&
            it.sessionScopeKey == scope.persistenceKey()
    }.mapNotNull { it.sessionTranscriptPath?.takeIf(String::isNotBlank) } else emptyList()
    return (candidates + scheduledPaths).distinct().singleOrNull()
}
