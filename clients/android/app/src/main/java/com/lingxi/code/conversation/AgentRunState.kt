package com.lingxi.code.conversation

enum class AgentToolStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    /** Historical invocation with no recorded result or authoritative live execution. */
    Unknown,
}

enum class AgentRunOutcome {
    Running,
    Completed,
    Failed,
    Cancelled,
    /** Restored terminal turn whose exact live outcome was not persisted. */
    Finished;

    val tone: AgentRunTone
        get() = when (this) {
            Running -> AgentRunTone.Running
            Completed -> AgentRunTone.Completed
            Failed -> AgentRunTone.Failed
            Cancelled -> AgentRunTone.Cancelled
            Finished -> AgentRunTone.Finished
        }
}

/** Unit-testable semantic color role for each terminal/live run outcome. */
enum class AgentRunTone {
    Running,
    Completed,
    Failed,
    Cancelled,
    Finished,
}

enum class AgentRunNoticeKind {
    Info,
    Warning,
    Error,
}

data class AgentToolRunState(
    val id: String,
    val tool: String,
    /**
     * The LEGACY one-line input summary from [summarizeToolInput]. Retained
     * only as the fallback for an engine that predates [header]; the derived
     * header supersedes it entirely.
     */
    val summary: String? = null,
    val status: AgentToolStatus = AgentToolStatus.Running,
    val elapsedMs: Long? = null,
    /** Engine-derived call header (`ToolUseStarted.header`). Null on an older engine. */
    val header: ToolHeaderUi? = null,
    /** Engine-derived `⎿` block (`ToolUseResult.display`). Null until the call returns. */
    val display: ToolResultDisplayUi? = null,
    val planMarkdown: String? = null,
) {
    /** This row as the shared [ToolCallUi] both render surfaces consume. */
    fun toToolCall(): ToolCallUi = ToolCallUi(
        id = id,
        tool = tool,
        header = header,
        display = display,
        status = status,
        fallbackSummary = summary,
        planMarkdown = planMarkdown,
    )
}

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
 *
 * [tools] is the one part that does NOT stay here. A live turn's tool calls
 * exist nowhere else — the engine keeps `ToolUse` out of the assistant message
 * it sends back — so `ChatState.settleTurn` moves them into the transcript
 * message when the turn settles, which is exactly what a resumed transcript
 * shows. What is left here afterwards is the shell rows, which have their own
 * persistent terminal cards.
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
        planMarkdown = event.planMarkdown ?: existing?.planMarkdown,
        status = status,
        elapsedMs = event.elapsedMs ?: existing?.elapsedMs,
        // The header rides the CALL, the display rides the RESULT — two separate
        // wire events for one row. Each must survive the other's arrival, so
        // neither is ever overwritten with the null the other event carries.
        header = event.header ?: existing?.header,
        display = event.display ?: existing?.display,
    )
    // NOTHING is dropped here. `tools` is the ONLY record of this turn's tool
    // calls — `ChatState.settleTurn` moves it into the transcript message — so a
    // cap applied at this layer would DELETE transcript content, permanently and
    // unrecoverably (the engine keeps `ToolUse` out of the assistant message,
    // which is why `settleTurn` exists at all). The 50-row budget that used to
    // live here is a RENDER window now; see [toolDisplayWindow].
    val updated = if (existingIndex >= 0) {
        tools.toMutableList().apply { this[existingIndex] = replacement }
    } else {
        (tools + replacement).toMutableList()
    }
    return copy(
        active = true,
        outcome = AgentRunOutcome.Running,
        reasoningActive = false,
        tools = updated,
        revision = revision + 1,
    )
}

/**
 * The newest [MAX_TOOL_ROWS] rows — the LIVE card's display budget, and ONLY
 * that.
 *
 * The live trace is drawn into a plain `Column`, not a lazy list, so every row
 * it holds is composed; a 300-call turn would compose 300 rows on every
 * revision. Windowing here is safe precisely because it is not storage:
 * [AgentRunState.tools] still holds every call, `settleTurn` still absorbs
 * every call into the transcript, and a late `ToolUseResult` for a row that has
 * scrolled out of the window still finds it (`reduceTool` searches the FULL
 * list by id).
 */
internal fun AgentRunState.toolDisplayWindow(): List<AgentToolRunState> =
    if (tools.size <= MAX_TOOL_ROWS) tools else tools.takeLast(MAX_TOOL_ROWS)

/** How many rows [toolDisplayWindow] left out. They are in the transcript. */
internal fun AgentRunState.hiddenToolCount(): Int =
    (tools.size - MAX_TOOL_ROWS).coerceAtLeast(0)

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
        AgentRunOutcome.Finished -> AgentToolStatus.Completed
        AgentRunOutcome.Running -> AgentToolStatus.Running
    }
    return copy(
        active = outcome == AgentRunOutcome.Running,
        outcome = outcome,
        reasoningActive = false,
        // Coordinator workers can outlive the reply stream. Keep the last
        // reported count until a later Coordinator(0) arrives so Android does
        // not release its foreground-service lease while they are still busy.
        tools = tools.map {
            if (it.status == AgentToolStatus.Running) it.copy(status = terminalToolStatus) else it
        },
        revision = revision + 1,
    )
}

/**
 * LEGACY one-line input summary, scraped from `input_json` by regex.
 *
 * The engine now derives the real header once and ships it on
 * `ToolUseStarted.header` — client-side re-parsing of tool JSON is precisely the
 * four-way drift that change deletes. This survives ONLY as the fallback for an
 * engine build that predates the field, and must not grow new keys or callers.
 */
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

/**
 * How many tool rows the live run card DRAWS. Never how many it KEEPS — see
 * [toolDisplayWindow].
 */
internal const val MAX_TOOL_ROWS = 50
private const val MAX_NOTICE_ROWS = 20
private const val MAX_TOOL_SUMMARY_CHARS = 160
private const val AUTO_SCROLL_REASONING_CHARS = 256
