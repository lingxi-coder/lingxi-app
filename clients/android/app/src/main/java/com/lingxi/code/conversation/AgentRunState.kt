package com.lingxi.code.conversation

enum class AgentToolStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

enum class AgentRunOutcome {
    Running,
    Completed,
    Failed,
    Cancelled,
}

enum class AgentRunNoticeKind {
    Info,
    Warning,
    Error,
}

data class AgentToolRunState(
    val id: String,
    val tool: String,
    val summary: String? = null,
    val status: AgentToolStatus = AgentToolStatus.Running,
    val elapsedMs: Long? = null,
)

data class AgentRunNotice(
    val text: String,
    val kind: AgentRunNoticeKind = AgentRunNoticeKind.Info,
)

data class AgentRunUsage(
    val inputTokens: Long,
    val outputTokens: Long,
    val cacheReadTokens: Long,
    val cacheCreationTokens: Long,
)

/**
 * The latest turn's live execution trace.
 *
 * This is deliberately UI-only state: the engine JSONL remains authoritative
 * for the conversation transcript, while transient reasoning, heartbeats and
 * retries stay visible until the next turn without being written back as fake
 * chat messages.
 */
data class AgentRunState(
    val turnId: Long,
    val active: Boolean = true,
    val outcome: AgentRunOutcome = AgentRunOutcome.Running,
    val reasoning: String = "",
    val reasoningActive: Boolean = false,
    val reasoningTruncated: Boolean = false,
    val tools: List<AgentToolRunState> = emptyList(),
    val notices: List<AgentRunNotice> = emptyList(),
    val usage: AgentRunUsage? = null,
    val formattedCost: String? = null,
    val activeWorkers: Int = 0,
    val teamName: String? = null,
    val revision: Int = 0,
)

internal fun AgentRunState.appendReasoning(delta: String): AgentRunState {
    if (delta.isEmpty()) {
        return if (reasoningActive) {
            this
        } else {
            copy(active = true, reasoningActive = true, revision = revision + 1)
        }
    }
    val combined = reasoning + delta
    val truncated = combined.length > MAX_REASONING_CHARS
    val shouldFollowOutput =
        reasoning.isEmpty() ||
            reasoning.length / AUTO_SCROLL_REASONING_CHARS !=
            combined.length / AUTO_SCROLL_REASONING_CHARS
    return copy(
        active = true,
        outcome = AgentRunOutcome.Running,
        reasoning = if (truncated) combined.takeLast(MAX_REASONING_CHARS) else combined,
        reasoningActive = true,
        reasoningTruncated = reasoningTruncated || truncated,
        revision = revision + if (shouldFollowOutput) 1 else 0,
    )
}

internal fun AgentRunState.markGenerating(): AgentRunState =
    if (active && outcome == AgentRunOutcome.Running && !reasoningActive) {
        this
    } else {
        copy(
            active = true,
            outcome = AgentRunOutcome.Running,
            reasoningActive = false,
            revision = revision + 1,
        )
    }

internal fun AgentRunState.reduceTool(event: ReplyEvent.ToolActivity): AgentRunState {
    val id = event.id
    val tool = event.tool
    val status = event.status
    if (id == null || tool == null || status == null) {
        return addNotice(AgentRunNotice(event.label))
    }
    val existingIndex = tools.indexOfFirst { it.id == id }
    val existing = tools.getOrNull(existingIndex)
    val replacement = AgentToolRunState(
        id = id,
        tool = tool,
        summary = event.inputSummary ?: existing?.summary,
        status = status,
        elapsedMs = event.elapsedMs ?: existing?.elapsedMs,
    )
    val updated = if (existingIndex >= 0) {
        tools.toMutableList().apply { this[existingIndex] = replacement }
    } else {
        (tools + replacement).takeLast(MAX_TOOL_ROWS).toMutableList()
    }
    return copy(
        active = true,
        outcome = AgentRunOutcome.Running,
        reasoningActive = false,
        tools = updated,
        revision = revision + 1,
    )
}

internal fun AgentRunState.addNotice(notice: AgentRunNotice): AgentRunState = copy(
    notices = (notices + notice).takeLast(MAX_NOTICE_ROWS),
    revision = revision + 1,
)

internal fun AgentRunState.updateUsage(usage: AgentRunUsage): AgentRunState = copy(
    usage = usage,
    revision = revision + 1,
)

internal fun AgentRunState.updateCost(formatted: String): AgentRunState = copy(
    formattedCost = formatted,
    revision = revision + 1,
)

internal fun AgentRunState.updateWorkers(count: Int, team: String?): AgentRunState = copy(
    activeWorkers = count.coerceAtLeast(0),
    teamName = team,
    revision = revision + 1,
)

internal fun AgentRunState.finish(outcome: AgentRunOutcome): AgentRunState {
    val terminalToolStatus = when (outcome) {
        AgentRunOutcome.Failed -> AgentToolStatus.Failed
        AgentRunOutcome.Cancelled -> AgentToolStatus.Cancelled
        AgentRunOutcome.Completed -> AgentToolStatus.Completed
        AgentRunOutcome.Running -> AgentToolStatus.Running
    }
    return copy(
        active = outcome == AgentRunOutcome.Running,
        outcome = outcome,
        reasoningActive = false,
        activeWorkers = 0,
        tools = tools.map {
            if (it.status == AgentToolStatus.Running) it.copy(status = terminalToolStatus) else it
        },
        revision = revision + 1,
    )
}

internal fun summarizeToolInput(inputJson: String): String? {
    for (key in SAFE_TOOL_INPUT_KEYS) {
        val match = Regex(
            "\"${Regex.escape(key)}\"\\s*:\\s*\"((?:\\\\.|[^\"\\\\])*)\"",
        ).find(inputJson) ?: continue
        return match.groupValues[1]
            .decodeJsonString()
            .replace(Regex("\\s+"), " ")
            .trim()
            .take(MAX_TOOL_SUMMARY_CHARS)
            .takeIf(String::isNotEmpty)
    }
    return null
}

private fun String.decodeJsonString(): String {
    val output = StringBuilder(length)
    var index = 0
    while (index < length) {
        val char = this[index++]
        if (char != '\\' || index >= length) {
            output.append(char)
            continue
        }
        when (val escaped = this[index++]) {
            '"', '\\', '/' -> output.append(escaped)
            'n' -> output.append('\n')
            'r' -> output.append('\r')
            't' -> output.append('\t')
            else -> output.append(escaped)
        }
    }
    return output.toString()
}

private val SAFE_TOOL_INPUT_KEYS = listOf(
    "path",
    "file_path",
    "query",
    "pattern",
    "url",
    "command",
    "prompt",
    "description",
)
private const val MAX_REASONING_CHARS = 64 * 1024
private const val MAX_TOOL_ROWS = 50
private const val MAX_NOTICE_ROWS = 20
private const val MAX_TOOL_SUMMARY_CHARS = 160
private const val AUTO_SCROLL_REASONING_CHARS = 256
