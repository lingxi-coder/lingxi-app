package com.lingxi.code.cron

import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.project.ProjectStoreState
import com.lingxi.code.project.ScopeStateStore

internal data class CronSessionTarget(
    val schedulerScopeId: String,
    val scopeKey: String,
    val sessionId: String,
    val transcriptPath: String,
    val mode: SessionMode,
    val label: String,
)

internal fun cronSessionTargets(
    state: ProjectStoreState,
    runs: List<CronRunRecord>,
    owners: Map<String, ScopeStateStore.SessionOwner>,
    mode: SessionMode,
): List<CronSessionTarget> {
    if (state.loading || state.storageUnavailable) return emptyList()
    val claims = buildList {
        state.globalSessions.forEach { add(Triple(canonicalSessionId(it.sessionId), "global" to it.mode, it.transcriptPath?.takeIf(String::isNotBlank))) }
        state.projects.forEach { project -> project.sessions.forEach {
            add(Triple(canonicalSessionId(it.sessionId), "project.${project.record.id}" to it.mode, it.transcriptPath?.takeIf(String::isNotBlank)))
        } }
        runs.filter { it.sessionScopeKey == "scheduled" }.forEach { run ->
            val id = run.sessionId?.let(::canonicalSessionId) ?: return@forEach
            val runMode = SessionMode.entries.firstOrNull { it.wireValue == run.sessionMode } ?: return@forEach
            add(Triple(id, "scheduled" to runMode, run.sessionTranscriptPath?.takeIf(String::isNotBlank)))
        }
    }.groupBy { it.first }
    val conflicted = claims.filterValues { group ->
        group.map { it.second }.distinct().size > 1 || group.mapNotNull { it.third }.distinct().size > 1
    }.keys
    val candidates = buildList {
        fun addRow(scheduler: String, scope: String, row: com.lingxi.code.project.ProjectSessionSummary, label: String) {
            val path = row.transcriptPath?.takeIf(String::isNotBlank) ?: return
            if (!row.isArchived && row.mode == mode) {
                add(CronSessionTarget(scheduler, scope, canonicalSessionId(row.sessionId), path, row.mode, label))
            }
        }
        state.globalSessions.forEach { addRow(GLOBAL_CRON_SCOPE_ID, "global", it, it.title) }
        state.projects.forEach { project ->
            project.sessions.forEach { row -> addRow(project.record.id, "project.${project.record.id}", row, "${project.record.name} / ${row.title}") }
        }
        runs.forEach { run ->
            val id = run.sessionId?.let(::canonicalSessionId) ?: return@forEach
            val path = run.sessionTranscriptPath?.takeIf(String::isNotBlank) ?: return@forEach
            if (run.sessionScopeKey == "scheduled" && run.sessionMode == mode.wireValue) {
                add(CronSessionTarget(GLOBAL_CRON_SCOPE_ID, "scheduled", id, path, mode, run.prompt.take(120)))
            }
        }
    }
    // Conflicting rows do not become safe merely because one claim happens to win.
    return candidates.groupBy { it.sessionId }.values.mapNotNull { group ->
        val identities = group.map { Triple(it.scopeKey, it.mode, it.transcriptPath) }.distinct()
        val target = group.first()
        target.takeIf { target.sessionId !in conflicted && identities.size == 1 &&
            owners[target.sessionId] == ScopeStateStore.SessionOwner(target.scopeKey, target.mode) }
    }
}

internal fun CronAutomation.clearTarget(): CronAutomation = change("targetSessionId", null)
    .change("targetSessionTranscriptPath", null).change("targetSessionMode", null).change("targetSessionScopeKey", null)

internal fun CronAutomation.withTarget(target: CronSessionTarget): CronAutomation =
    change("targetSessionId", target.sessionId).change("targetSessionTranscriptPath", target.transcriptPath)
        .change("targetSessionMode", target.mode.wireValue).change("targetSessionScopeKey", target.scopeKey)

internal fun CronAutomation.matchesTarget(target: CronSessionTarget, scopeId: String, mode: SessionMode): Boolean =
    target.schedulerScopeId == scopeId && target.mode == mode &&
        canonicalSessionId(targetSessionId) == target.sessionId && targetSessionTranscriptPath == target.transcriptPath &&
        targetSessionMode == target.mode.wireValue && targetSessionScopeKey == target.scopeKey

internal fun cronCreatedSessionScope(scopeId: String): String =
    if (scopeId == GLOBAL_CRON_SCOPE_ID) "scheduled" else "project.$scopeId"
