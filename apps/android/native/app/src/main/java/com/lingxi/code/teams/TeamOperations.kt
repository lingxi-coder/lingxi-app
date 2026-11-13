package com.lingxi.code.teams

import androidx.annotation.StringRes
import com.lingxi.code.R
import org.json.JSONObject

/** One action a task took outside the Chat, as `listOperations` returns it. Parameters and results are never part of it. */
internal data class TeamTaskOperation(
    val operationId: String, val logicalStepId: String, val category: String, val toolName: String, val toolVersion: String, val state: String, val epoch: Long,
    val dispatchedAt: Long, val settledAt: Long? = null, val resolvedBy: TeamMemberRef? = null, val resolvedAt: Long? = null,
) {
    /** Nobody knows whether it went through: it was sent and no result came back, or the result could not be read. */
    val unverified get() = category in setOf("queryable", "unqueryable") && state in setOf("dispatched", "unknown")

    companion object {
        fun list(v: JSONObject): List<TeamTaskOperation> = v.getJSONArray("operations").objects().map { row ->
            val tool = row.getJSONObject("tool")
            TeamTaskOperation(row.getString("operationId"), row.getString("logicalStepId"), row.getString("category"), tool.getString("name"), tool.getString("version"), row.getString("state"),
                row.getLong("epoch"), row.getLong("dispatchedAt"), if (row.has("settledAt")) row.getLong("settledAt") else null,
                row.optJSONObject("resolvedBy")?.let(TeamMemberRef::parse), if (row.has("resolvedAt")) row.getLong("resolvedAt") else null)
        }
    }
}

internal object TeamOperationRules {
    /** The requester and the Bot's maintainer may look at what a task that needs checking did outside the Chat. */
    fun canReview(task: TeamTask, viewer: TeamMemberRef?, maintainer: TeamMemberRef?) =
        viewer != null && task.state == "needs_reconciliation" && (task.requester == viewer || maintainer == viewer)
    /** Only the Bot's maintainer says what happened. */
    fun canResolve(viewer: TeamMemberRef?, maintainer: TeamMemberRef?) = viewer != null && maintainer == viewer
    /** Whether [viewer] maintains the Bot of the task [taskId], as far as the loaded tasks and cards say. */
    fun maintains(taskId: String, tasks: List<TeamTask>, bots: List<TeamBotCard>, viewer: TeamMemberRef?): Boolean {
        val task = tasks.firstOrNull { it.taskId == taskId } ?: return false
        return canResolve(viewer, bots.firstOrNull { it.sharedBotId == task.sharedBotId }?.maintainer)
    }
    @StringRes fun stateRes(operation: TeamTaskOperation): Int = when {
        operation.unverified -> if (operation.state == "unknown") R.string.teams_ops_state_unknown else R.string.teams_ops_state_unreported
        operation.state == "succeeded" -> R.string.teams_ops_state_happened
        operation.state == "failed" -> R.string.teams_ops_state_not_happened
        else -> R.string.teams_ops_state_repeatable
    }
    /** The text of a refusal on the review screen. The same codes mean something else for other commands. */
    @StringRes fun errorRes(code: String): Int = when (code) {
        "OPERATION_CONFLICT" -> R.string.teams_error_ops_conflict
        "OPERATION_REQUIRED", "OPERATION_READ_FORBIDDEN" -> R.string.teams_error_ops_unavailable
        "MAINTAINER_REQUIRED" -> R.string.teams_error_ops_maintainer
        "TASK_RUNNING" -> R.string.teams_error_ops_running
        else -> teamsErrorRes(code)
    }
}
