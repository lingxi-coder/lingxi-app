package com.lingxi.code.cron

import com.lingxi.code.bindings.runtime.CronTaskDto
import com.lingxi.code.bindings.client.ReasoningSelectionDto
import org.json.JSONObject

/** Versioned engine metadata. Preserve unknown fields when editing known settings. */
data class CronAutomation(val json: String = "{}") {
    /** Parsed once. Every property below used to re-parse the whole blob, and
     * the task list reads several of them per row inside a filter that re-runs
     * on each keystroke. `change` still takes a fresh copy, because `put`
     * mutates the object it is called on. */
    private val parsed: JSONObject by lazy { JSONObject(json) }
    private fun value() = JSONObject(json)
    val status: String get() = parsed.optString("status", "active")
    val statusReason: String? get() = parsed.optString("statusReason").takeIf { it.isNotBlank() && it != "null" }
    val name: String get() = parsed.optString("name", "").let { if (it == "null") "" else it }
    val model: String get() = parsed.optString("model", "")
    val runMode: String get() = parsed.optString("runMode", "new_session")
    val targetSessionId: String get() = parsed.optString("targetSessionId", "").let { if (it == "null") "" else it }
    val notificationPolicy: String get() = parsed.optString("notificationPolicy", "all")
    val reasoningJson: String get() {
        val source = parsed.optJSONObject("reasoning") ?: JSONObject().put("type", "automatic")
        val canonical = JSONObject().put("type", source.optString("type", "automatic"))
        when (source.optString("type")) {
            "level" -> canonical.put("id", source.optString("id"))
            "token_budget" -> canonical.put("tokens", source.optLong("tokens"))
        }
        return canonical.toString()
    }
    val reasoningLabel: String get() {
        val selection = JSONObject(reasoningJson)
        return when (selection.optString("type")) {
            "disabled" -> "Off"
            "enabled" -> "On"
            "level" -> selection.optString("id").replaceFirstChar { it.uppercase() }
            "token_budget" -> "${selection.optLong("tokens")} tokens"
            else -> "Automatic"
        }
    }
    // Keep a newer schema version rather than claiming the payload is v2: a
    // task written by a newer build still renders here (every getter has a
    // default), and stamping `2` over it would tell the engine a v3 document is
    // v2. iOS refuses to write such a task at all rather than downgrade it.
    fun change(key: String, newValue: Any?): CronAutomation =
        CronAutomation(value().put("version", maxOf(parsed.optInt("version", 2), 2)).put(key, newValue).toString())
    fun withReasoning(selection: ReasoningSelectionDto): CronAutomation = change("reasoning", reasoningJson(selection))
    fun copied(): CronAutomation = change("status", "active").change("statusReason", null)
        .change("ownedSessionId", null).change("targetSessionId", null).change("runMode", "new_session")
        .change("runs", org.json.JSONArray())
    companion object {
        fun from(task: CronTaskDto): CronAutomation = CronAutomation(task.automationJson ?: "{}")
        fun defaults(model: String): CronAutomation = CronAutomation().change("model", model)
            .change("status", "active").change("runMode", "new_session")
            .change("notificationPolicy", "all").withReasoning(ReasoningSelectionDto.Automatic)
    }
}

internal fun reasoningJson(selection: ReasoningSelectionDto): JSONObject = JSONObject().apply {
    when (selection) {
        is ReasoningSelectionDto.Automatic -> put("type", "automatic")
        is ReasoningSelectionDto.Disabled -> put("type", "disabled")
        is ReasoningSelectionDto.Enabled -> put("type", "enabled")
        is ReasoningSelectionDto.Level -> { put("type", "level"); put("id", selection.id) }
        is ReasoningSelectionDto.TokenBudget -> { put("type", "token_budget"); put("tokens", selection.tokens.toLong()) }
    }
}

internal fun CronTaskDto.isActive(): Boolean = CronAutomation.from(this).status == "active"

internal data class NativeCronTerminal(
    val status: CronRunStatus,
    val model: String?,
    val sessionId: String?,
    val summary: String?,
    val error: String?,
    val finishedAtMs: Long?,
)

/** A finished one-shot no longer appears in the due list; inspect its durable run journal. */
internal fun CronAutomation.terminalFor(record: CronRunRecord): NativeCronTerminal? {
    val runs = JSONObject(json).optJSONArray("runs") ?: return null
    return (runs.length() - 1 downTo 0).firstNotNullOfOrNull { index ->
        val run = runs.getJSONObject(index)
        val scheduledAt = run.optLong("scheduledAt", -1)
        val hasManualOccurrence = run.has("manualOccurrenceAt") && !run.isNull("manualOccurrenceAt")
        // 🚨 `hasManualOccurrence` is a property of the JOURNAL ENTRY, not of the
        // record, so on its own every SCHEDULED entry fell into the legacy `>=`
        // branch for a manual record. A manual run left Running (the busy handoff
        // parks it while releasing the serial chain) does not block the task's own
        // next occurrence, so that occurrence's terminal entry matched
        // `scheduledAt >= triggeredAtMs` and closed the manual run out with a
        // session and summary it never produced. The engine stamps every manual
        // run `<taskId>-manual-…` (`cron/src/automation.rs`), so a run id settles
        // it; the time comparison stays only for entries old enough to carry no
        // id at all. iOS guards the same ambiguity by id prefix.
        val runId = run.optString("id").takeIf { it.isNotBlank() && it != "null" }
        val manualById = runId?.startsWith("${record.taskId}-manual-")
        val matches = if (record.manual) {
            if (hasManualOccurrence) run.optLong("manualOccurrenceAt", -1) == record.scheduledAtMs
            else manualById != false && scheduledAt >= record.triggeredAtMs
        } else {
            !hasManualOccurrence && manualById != true && scheduledAt == record.scheduledAtMs
        }
        if (!matches) return@firstNotNullOfOrNull null
        val status = when (run.optString("status")) {
            "succeeded" -> CronRunStatus.Succeeded
            "failed" -> CronRunStatus.Failed
            "cancelled" -> CronRunStatus.Cancelled
            "interrupted" -> CronRunStatus.Interrupted
            else -> return@firstNotNullOfOrNull null
        }
        fun optional(key: String): String? = run.optString(key).takeIf { it.isNotBlank() && it != "null" }
        NativeCronTerminal(status, optional("model"), optional("sessionId"), optional("summary"), optional("error"),
            run.optLong("finishedAt").takeIf { it > 0 })
    }
}
