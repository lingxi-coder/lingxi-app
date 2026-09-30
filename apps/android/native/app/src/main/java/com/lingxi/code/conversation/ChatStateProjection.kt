package com.lingxi.code.conversation

import com.lingxi.code.R
import com.lingxi.code.bindings.TaskStatusDto
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import kotlinx.coroutines.flow.map

/** A settled turn: the transcript it produced, and what is left of its run trace. */
internal data class SettledTurn(
    val messages: List<Message>,
    val run: AgentRunState?,
    val agentRunsByMessageId: Map<String, AgentRunState>,
)

/**
 * Restore the durable terminal row that follows each assistant transcript
 * message. Live-only trace details are not part of SessionResumed, but the
 * persisted assistant result is sufficient to reconstruct its terminal status
 * without letting the row disappear after reconnect/process restoration.
 */
internal fun reconstructTerminalAgentRuns(messages: List<Message>): Map<String, AgentRunState> =
    buildMap {
        var restoredTurn = -1L
        messages.forEach { message ->
            if (message.role != Role.Ai) return@forEach
            // SessionResumed persists the assistant result but not the overall
            // turn outcome. A failed child tool does not imply a failed Agent —
            // the model may have recovered — so use the neutral Finished state
            // until the wire carries an explicit outcome.
            put(
                message.id,
                AgentRunState(turnId = restoredTurn--).finish(AgentRunOutcome.Finished),
            )
        }
    }

internal fun Map<String, BackgroundTaskUi>.updateStatus(
    taskId: String,
    status: TaskStatusDto,
    error: String? = null,
): Map<String, BackgroundTaskUi> = mapNotNull { (id, task) ->
    if (id == taskId) {
        // Never clear a reason already learned from a `TaskRow` backfill — the
        // push and the row list are two sources for the same field.
        id to task.copy(status = status, error = error?.takeIf { it.isNotBlank() } ?: task.error)
    } else {
        id to task
    }
}.toMap()

internal fun Set<String>.withTaskStatus(taskId: String, status: TaskStatusDto): Set<String> =
    when (status) {
        TaskStatusDto.PENDING, TaskStatusDto.RUNNING -> this + taskId
        TaskStatusDto.PAUSED,
        TaskStatusDto.COMPLETED,
        TaskStatusDto.FAILED,
        TaskStatusDto.CANCELLED,
        -> this - taskId
    }

/**
 * Settle a finished turn: MOVE its tool calls out of the transient run trace and
 * into the transcript message that settles with it.
 *
 * ### Why they have to move
 *
 * A live turn's tool calls exist in exactly ONE place — [ChatState.agentRun] —
 * and [ChatViewModel.send] overwrites that with a fresh [AgentRunState] the
 * instant the next turn starts. So turn N-1's tool rows were destroyed by turn
 * N, and the same conversation read completely differently before and after a
 * restart (a resumed transcript rebuilds every call inline, via
 * [transcriptFromDtos]).
 *
 * ### Why they must be APPENDED, not merged
 *
 * The engine SPLITS the model stream (`orchestrator/src/streaming_loop.rs`):
 * text and thinking accumulate into `PumpedTurn::assistant_blocks`, while a
 * `ToolUse` goes to a separate `tool_uses` field and is explicitly NOT pushed to
 * `assistant_blocks`. `MessageComplete`'s payload is
 * `synthesize_message(turn)` over `assistant_blocks` alone
 * (`client-adapter/src/turn.rs`), so its `blocks` NEVER contain a `ToolUse` and
 * [messageDtoToMessage] never yields a [MessageContent.Tool]. A merge keyed on
 * an existing tool block therefore matched nothing, ever. The rows themselves
 * must be added. (The by-id merge is still done first, so the day the engine
 * does ship `ToolUse` in `assistant_blocks` its header/display are honored in
 * place instead of being duplicated.)
 *
 * ### Why it is a MOVE
 *
 * Copying would draw every row twice — once in the bubble, once in the run card
 * that stays on screen until the next turn. Removing what was absorbed also
 * makes this idempotent: `MessageComplete` followed by `TurnEnded` settles once.
 *
 * Shell calls are the exception and are left in the run trace: they already own
 * a persistent [ChatRenderItem.Shell] terminal card of their own, so absorbing
 * them would be a third copy.
 *
 * PURE, for JVM tests.
 */
internal fun ChatState.settleTurn(run: AgentRunState?, settling: Message?): SettledTurn {
    val shellBacked = shellTools.mapTo(mutableSetOf()) { it.taskId }
    val absorbed = run?.tools.orEmpty().filterNot { it.id in shellBacked }
    val terminalRun = run?.takeUnless { it.active || it.outcome == AgentRunOutcome.Running }
    val terminalAlreadySettled = terminalRun != null &&
        agentRunsByMessageId.values.any { it.turnId == terminalRun.turnId }
    if (absorbed.isEmpty() && settling == null) {
        return SettledTurn(messages, run, agentRunsByMessageId)
    }
    // A turn can end with tools or status and no prose at all. Mint a hidden
    // anchor message so the terminal result still has a stable transcript slot.
    val base = settling ?: Message(role = Role.Ai, text = "")
    val settled = if (absorbed.isEmpty()) base else base.absorbToolCalls(absorbed)
    val remainingRun = run?.let { existing ->
        if (absorbed.isEmpty()) existing else existing.copy(
            tools = existing.tools.filter { it.id in shellBacked },
            revision = existing.revision + 1,
        )
    }
    val settledRuns = if (terminalRun != null && !terminalAlreadySettled) {
        agentRunsByMessageId + (settled.id to (remainingRun ?: terminalRun))
    } else {
        agentRunsByMessageId
    }
    return SettledTurn(
        messages = messages + settled,
        run = remainingRun,
        agentRunsByMessageId = settledRuns,
    )
}

/** Merge [rows] into this message's own tool blocks by id, then append the rest. */
private fun Message.absorbToolCalls(rows: List<AgentToolRunState>): Message {
    val byId = rows.associateBy { it.id }
    val mergedIds = mutableSetOf<String>()
    val existing = blocks.map { block ->
        val call = (block as? MessageContent.Tool)?.call ?: return@map block
        val live = byId[call.id] ?: return@map block
        mergedIds += call.id
        MessageContent.Tool(
            call.copy(
                header = call.header ?: live.header,
                display = call.display ?: live.display,
                status = if (call.display != null) call.status else live.status,
            ),
        )
    }
    // A streamed message carries prose in `text` and no blocks. Once blocks
    // exist the bubble renders THOSE and ignores `text`, so the prose has to be
    // seeded as a block or it would vanish behind the tool rows.
    val prose = existing.ifEmpty {
        if (text.isBlank()) emptyList() else listOf(MessageContent.Text(text))
    }
    val appended = rows.filterNot { it.id in mergedIds }
        .map { MessageContent.Tool(it.toToolCall()) }
    return copy(blocks = prose + appended)
}

/**
 * The transient one-line notice for a background task transition (`/tasks`
 * push events). Rides [ChatState.statusLine] — the same dim, auto-overwritten
 * row tool activity uses — deliberately NOT the persistent error banner; a
 * full task-progress surface is a later pass. PURE for JVM tests.
 */
internal fun taskStatusLine(
    taskId: String,
    status: TaskStatusDto,
    strings: ConversationStrings = DefaultConversationStrings,
    error: String? = null,
): String = when (status) {
    TaskStatusDto.PENDING ->
        strings.resolve(R.string.chat_task_status_pending, "后台任务 %1\$s 已排队", taskId)
    TaskStatusDto.RUNNING ->
        strings.resolve(R.string.chat_task_status_running, "后台任务 %1\$s 运行中", taskId)
    TaskStatusDto.PAUSED ->
        strings.resolve(R.string.chat_task_status_paused, "后台任务 %1\$s 已暂停", taskId)
    TaskStatusDto.COMPLETED ->
        strings.resolve(R.string.chat_task_status_completed, "后台任务 %1\$s 已完成", taskId)
    TaskStatusDto.FAILED -> error?.takeIf { it.isNotBlank() }?.let { reason ->
        strings.resolve(
            R.string.chat_task_status_failed_reason,
            "后台任务 %1\$s 已失败：%2\$s",
            taskId,
            reason,
        )
    } ?: strings.resolve(R.string.chat_task_status_failed, "后台任务 %1\$s 已失败", taskId)
    TaskStatusDto.CANCELLED ->
        strings.resolve(R.string.chat_task_status_cancelled, "后台任务 %1\$s 已取消", taskId)
}
