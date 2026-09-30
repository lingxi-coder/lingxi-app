package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.TaskStatusDto
import com.lingxi.code.bindings.client.WorkflowProgressDto

/** One structured, session-owned workflow update delivered outside ClientEvent. */
data class WorkflowProgressUpdate(
    val originSessionId: String,
    val taskId: String,
    val runId: String,
    val progress: WorkflowProgressDto,
)

enum class WorkflowAgentStatus(val wireValue: String) {
    Start("start"),
    Progress("progress"),
    Done("done"),
    Error("error"),
    Cached("cached");

    val isTerminal: Boolean get() = this == Done || this == Error || this == Cached

    companion object {
        fun fromWire(value: String?): WorkflowAgentStatus? = entries.firstOrNull { it.wireValue == value }
    }
}

data class WorkflowPhaseUi(
    val id: String,
    val index: UInt?,
    val title: String,
    val message: String?,
    val updatedAtMs: ULong,
)

data class WorkflowAgentUi(
    val index: ULong,
    val title: String?,
    val agentId: String?,
    val agentType: String?,
    val model: String?,
    val fallbackModel: String?,
    val status: WorkflowAgentStatus,
    val activity: String?,
    val error: String?,
    val phaseIndex: UInt?,
    val phaseTitle: String?,
    val lastProgressAtMs: ULong,
    val tokens: ULong?,
    val toolCalls: ULong?,
) {
    val displayTitle: String
        get() = title?.takeIf(String::isNotBlank)
            ?: agentType?.takeIf(String::isNotBlank)
            ?: agentId?.takeIf(String::isNotBlank)
            ?: "Agent ${index + 1u}"
}

data class WorkflowRunUi(
    val originSessionId: String,
    val taskId: String,
    val runId: String,
    val status: TaskStatusDto = TaskStatusDto.RUNNING,
    val phases: List<WorkflowPhaseUi> = emptyList(),
    val agents: List<WorkflowAgentUi> = emptyList(),
    val latestLog: String? = null,
    val lastUpdatedAtMs: ULong = 0u,
) {
    val currentPhaseTitle: String?
        get() = agents
            .filterNot { it.status.isTerminal }
            .sortedWith(compareBy<WorkflowAgentUi> { it.phaseIndex }.thenBy { it.index })
            .firstNotNullOfOrNull { agent ->
                agent.phaseTitle ?: phases.firstOrNull { it.index == agent.phaseIndex }?.title
            }
            ?: phases.maxWithOrNull(compareBy<WorkflowPhaseUi> { it.index }.thenBy { it.updatedAtMs })?.title

    val completedAgents: Int get() = agents.count { it.status.isTerminal }
    val runningAgents: Int get() = agents.size - completedAgents
}

/**
 * Fold one Claude-style workflow callback into an immutable UI snapshot.
 * Agent terminal states are sticky and event timestamps reject stale progress.
 */
internal fun reduceWorkflowProgress(
    existing: WorkflowRunUi?,
    update: WorkflowProgressUpdate,
    nowMs: ULong = System.currentTimeMillis().toULong(),
): WorkflowRunUi? {
    val progress = update.progress
    if (progress.kind !in WORKFLOW_PROGRESS_KINDS) return existing
    val eventMoment = progress.lastProgressAtMs
        ?: progress.startedAtMs
        ?: progress.queuedAtMs
        ?: nowMs
    if (existing != null && existing.runId != update.runId && eventMoment < existing.lastUpdatedAtMs) {
        return existing
    }
    var run = if (existing?.runId == update.runId) {
        existing
    } else {
        WorkflowRunUi(
            originSessionId = update.originSessionId,
            taskId = update.taskId,
            runId = update.runId,
        )
    }
    run = run.copy(lastUpdatedAtMs = maxOf(run.lastUpdatedAtMs, eventMoment))

    return when (progress.kind) {
        WORKFLOW_PHASE -> {
            val title = progress.phaseTitle
                ?: progress.title
                ?: progress.label
                ?: progress.message
                ?: "Workflow"
            val phaseId = progress.phaseIndex?.let { "phase-$it" }
                ?: "phase-${title.lowercase()}"
            val incoming = WorkflowPhaseUi(
                id = phaseId,
                index = progress.phaseIndex,
                title = title,
                message = progress.message,
                updatedAtMs = eventMoment,
            )
            val phases = run.phases.toMutableList()
            val at = phases.indexOfFirst { it.id == phaseId }
            if (at < 0) {
                phases += incoming
            } else if (incoming.updatedAtMs >= phases[at].updatedAtMs) {
                phases[at] = incoming.copy(message = incoming.message ?: phases[at].message)
            }
            run.copy(phases = phases)
        }

        WORKFLOW_LOG -> run.copy(
            latestLog = progress.message ?: progress.title ?: progress.label ?: run.latestLog,
        ).withPhaseFrom(progress, eventMoment)

        WORKFLOW_AGENT -> {
            val incomingStatus = WorkflowAgentStatus.fromWire(progress.state) ?: WorkflowAgentStatus.Progress
            val incoming = WorkflowAgentUi(
                index = progress.index,
                title = progress.title,
                agentId = progress.agentId,
                agentType = progress.agentType,
                model = progress.model,
                fallbackModel = progress.fallbackModel,
                status = incomingStatus,
                activity = progress.error
                    ?: progress.message
                    ?: progress.lastToolSummary
                    ?: progress.lastToolName
                    ?: progress.label,
                error = progress.error,
                phaseIndex = progress.phaseIndex,
                phaseTitle = progress.phaseTitle,
                lastProgressAtMs = eventMoment,
                tokens = progress.tokens,
                toolCalls = progress.toolCalls,
            )
            val agents = run.agents.toMutableList()
            val at = agents.indexOfFirst { it.index == incoming.index }
            if (at < 0) {
                agents += incoming
            } else {
                agents[at] = mergeWorkflowAgent(agents[at], incoming)
            }
            run.copy(agents = agents).withPhaseFrom(progress, eventMoment)
        }

        else -> run
    }
}

private fun WorkflowRunUi.withPhaseFrom(
    progress: WorkflowProgressDto,
    eventMoment: ULong,
): WorkflowRunUi {
    val title = progress.phaseTitle?.takeIf(String::isNotBlank) ?: return this
    val id = progress.phaseIndex?.let { "phase-$it" } ?: "phase-${title.lowercase()}"
    val phases = phases.toMutableList()
    val at = phases.indexOfFirst { it.id == id }
    val incoming = WorkflowPhaseUi(id, progress.phaseIndex, title, progress.message, eventMoment)
    if (at < 0) phases += incoming else if (eventMoment >= phases[at].updatedAtMs) phases[at] = incoming
    return copy(phases = phases)
}

private fun mergeWorkflowAgent(
    current: WorkflowAgentUi,
    incoming: WorkflowAgentUi,
): WorkflowAgentUi {
    if (current.status.isTerminal && !incoming.status.isTerminal) return current
    if (!incoming.status.isTerminal && incoming.lastProgressAtMs < current.lastProgressAtMs) return current
    val advances = incoming.status.isTerminal || incoming.lastProgressAtMs >= current.lastProgressAtMs
    return current.copy(
        title = incoming.title ?: current.title,
        agentId = incoming.agentId ?: current.agentId,
        agentType = incoming.agentType ?: current.agentType,
        model = incoming.model ?: current.model,
        fallbackModel = incoming.fallbackModel ?: current.fallbackModel,
        status = if (advances) incoming.status else current.status,
        activity = if (advances) incoming.activity ?: current.activity else current.activity,
        error = if (advances) incoming.error ?: current.error else current.error,
        phaseIndex = incoming.phaseIndex ?: current.phaseIndex,
        phaseTitle = incoming.phaseTitle ?: current.phaseTitle,
        lastProgressAtMs = maxOf(current.lastProgressAtMs, incoming.lastProgressAtMs),
        tokens = maxNullable(current.tokens, incoming.tokens),
        toolCalls = maxNullable(current.toolCalls, incoming.toolCalls),
    )
}

private fun maxNullable(left: ULong?, right: ULong?): ULong? = when {
    left == null -> right
    right == null -> left
    else -> maxOf(left, right)
}

internal fun TaskStatusDto.isActivelyExecutingWorkflow(): Boolean = when (this) {
    TaskStatusDto.PENDING, TaskStatusDto.RUNNING -> true
    TaskStatusDto.PAUSED,
    TaskStatusDto.COMPLETED,
    TaskStatusDto.FAILED,
    TaskStatusDto.CANCELLED,
    -> false
}

private const val WORKFLOW_PHASE = "workflow_phase"
private const val WORKFLOW_LOG = "workflow_log"
private const val WORKFLOW_AGENT = "workflow_agent"
private val WORKFLOW_PROGRESS_KINDS = setOf(WORKFLOW_PHASE, WORKFLOW_LOG, WORKFLOW_AGENT)
