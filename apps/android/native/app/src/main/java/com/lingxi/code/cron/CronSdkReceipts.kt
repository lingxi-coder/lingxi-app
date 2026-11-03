package com.lingxi.code.cron

import com.lingxi.code.bindings.runtime.CronTaskDto
import com.lingxi.code.model.SessionMode
import org.json.JSONObject

/** Strictly import the SDK journal's captured session identity, never infer it from cwd. */
internal fun sdkCronReceipts(scope: CronScope, task: CronTaskDto): List<CronRunRecord> =
    sdkCronReceipts(scope, task.id, task.prompt, task.automationJson)

internal fun sdkCronReceipts(scope: CronScope, taskId: String, prompt: String, raw: String?): List<CronRunRecord> {
    if (raw == null) return emptyList()
    val metadata = JSONObject(raw)
    if (!metadata.has("runs") || metadata.isNull("runs")) return emptyList()
    val runs = metadata.getJSONArray("runs")
    return (0 until runs.length()).mapNotNull { index ->
        val run = runs.getJSONObject(index)
        fun optional(key: String): String? {
            if (!run.has(key) || run.isNull(key)) return null
            return run.getString(key).takeIf(String::isNotBlank) ?: error("Scheduled receipt has an empty $key")
        }
        val idField = optional("sessionId")
        val pathField = optional("sessionTranscriptPath")
        val modeField = optional("sessionMode")
        val scopeField = optional("sessionScopeKey")
        if (listOf(idField, pathField, modeField, scopeField).all { it == null }) return@mapNotNull null
        val sessionId = requireNotNull(idField) { "Scheduled receipt has no session id" }
        val path = requireNotNull(pathField) { "Scheduled receipt has no transcript path" }
        val mode = requireNotNull(modeField) { "Scheduled receipt has no session mode" }
        val scopeKey = requireNotNull(scopeField) { "Scheduled receipt has no session owner" }
        validateCronOwnerIdentity(CronOwnerIdentity(sessionId, path, mode, scopeKey))
        val runId = requireNotNull(optional("id")) { "Scheduled receipt has no run id" }
        val timestamp = run.optLong("scheduledAt", 0L)
        CronRunRecord(
            runId = "sdk:${scope.scopeId}:$taskId:$runId", taskId = taskId, scopeId = scope.scopeId,
            projectId = scope.projectId, projectName = scope.projectName, prompt = prompt,
            scheduledAtMs = timestamp, triggeredAtMs = timestamp,
            finishedAtMs = run.optLong("finishedAt", 0L).takeIf { it > 0L },
            status = when (run.optString("status")) {
                "succeeded" -> CronRunStatus.Succeeded
                "failed" -> CronRunStatus.Failed
                "cancelled" -> CronRunStatus.Cancelled
                "interrupted" -> CronRunStatus.Interrupted
                "outcome_unknown" -> CronRunStatus.OutcomeUnknown
                else -> CronRunStatus.Running
            },
            sessionId = sessionId, sessionTranscriptPath = path, sessionMode = mode, sessionScopeKey = scopeKey,
        )
    }
}

internal fun validateCronOwnerIdentity(identity: CronOwnerIdentity): SessionMode =
    validateCronOwnerCandidate(CronOwnerCandidate(identity.sessionId, identity.transcriptPath, identity.mode, identity.scopeKey))

internal fun validateCronOwnerCandidate(candidate: CronOwnerCandidate): SessionMode {
    val canonical = com.lingxi.code.model.canonicalSessionId(candidate.sessionId)
    require(runCatching { java.util.UUID.fromString(canonical).toString() == canonical }.getOrDefault(false)) {
        "Scheduled receipt has an invalid session id"
    }
    require(candidate.transcriptPath == null ||
        (candidate.transcriptPath.isNotBlank() && java.io.File(candidate.transcriptPath).isAbsolute)) {
        "Scheduled owner has an invalid transcript path"
    }
    val mode = requireNotNull(SessionMode.entries.firstOrNull { it.wireValue == candidate.mode }) {
        "Scheduled receipt has an invalid mode"
    }
    val scope = requireNotNull(com.lingxi.code.model.conversationScopeFromKey(candidate.scopeKey)) {
        "Scheduled receipt has an invalid logical owner"
    }
    if (scope is com.lingxi.code.model.ConversationScope.Project) {
        require(runCatching { java.util.UUID.fromString(scope.projectId).toString() == scope.projectId }.getOrDefault(false)) {
            "Scheduled receipt has an invalid project owner"
        }
    }
    return mode
}
