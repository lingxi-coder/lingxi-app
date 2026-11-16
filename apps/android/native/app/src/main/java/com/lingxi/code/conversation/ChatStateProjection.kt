package com.lingxi.code.conversation

import com.lingxi.code.R
import com.lingxi.code.bindings.client.TaskStatusDto
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
    if (absorbed.isEmpty() && settling == null) {
        return SettledTurn(messages, run, agentRunsByMessageId)
    }
    // A turn can end with tools or status and no prose at all. Mint a hidden
    // anchor message so the terminal result still has a stable transcript slot.
    val base = settling ?: Message(role = Role.Ai, text = "")
    val incoming = if (absorbed.isEmpty()) base else base.absorbToolCalls(absorbed)
    val merged = mergeTranscriptMessage(messages, incoming)
    val settled = merged.message
    val remainingRun = run?.let { existing ->
        if (absorbed.isEmpty()) existing else existing.copy(
            tools = existing.tools.filter { it.id in shellBacked },
            revision = existing.revision + 1,
        )
    }
    val retainedRuns = agentRunsByMessageId.filterKeys { it !in merged.removedMessageIds }.toMutableMap()
    // A replay may join several persisted rows into one assistant bubble. Its
    // existing terminal result belongs to that same bubble after the join.
    val aliasedRun = merged.removedMessageIds.mapNotNull(agentRunsByMessageId::get)
        .firstOrNull { it.outcome != AgentRunOutcome.Finished }
        ?: merged.removedMessageIds.firstNotNullOfOrNull(agentRunsByMessageId::get)
    if (aliasedRun != null && (retainedRuns[settled.id] == null ||
            retainedRuns[settled.id]?.outcome == AgentRunOutcome.Finished)) {
        retainedRuns[settled.id] = aliasedRun
    }
    val terminalAlreadySettled = terminalRun != null &&
        retainedRuns.values.any { it.turnId == terminalRun.turnId }
    val settledRuns = if (terminalRun != null && !terminalAlreadySettled) {
        retainedRuns +
            (settled.id to (remainingRun ?: terminalRun))
    } else {
        retainedRuns
    }
    return SettledTurn(
        messages = merged.messages,
        run = remainingRun,
        agentRunsByMessageId = settledRuns,
    )
}

private data class MergedTranscriptMessage(
    val messages: List<Message>,
    val message: Message,
    val removedMessageIds: Set<String>,
)

private fun Message.persistedIdentities(): Set<String> =
    transcriptAliases + listOfNotNull(transcriptUuid)

/** Snapshot and replay overlap by durable identity, never by matching prose. */
internal fun mergeTranscriptHistory(history: List<Message>, updates: List<Message>): List<Message> =
    updates.fold(history) { rows, message -> mergeTranscriptMessage(rows, message).messages }

/** A display-only snapshot must not discard rows delivered while it was read. */
internal fun mergeHeadlessSnapshotMessages(
    base: List<Message>,
    live: List<Message>,
    baseline: List<Message> = emptyList(),
): List<Message> {
    val baselineById = baseline.associateBy { it.id }
    val oldTools = live.flatMap { it.blocks.filterIsInstance<MessageContent.Tool>() }
        .associate { it.call.id to it.call }
    val snapshot = base.map { message ->
        message.copy(blocks = message.blocks.map { block ->
            if (block !is MessageContent.Tool) block else oldTools[block.call.id]
                ?.let { MessageContent.Tool(block.call.mergeSnapshotTool(it)) } ?: block
        })
    }
    // A bootstrap row that did not change while the snapshot was read cannot
    // overwrite the newer disk projection. Keep genuinely new live rows/tails.
    val updates = live.filterNot { row ->
        baselineById[row.id] == row && snapshot.any { it.sameTranscriptRow(row) }
    }
    val retainedIds = mutableSetOf<String>()
    return mergeTranscriptHistory(snapshot, updates).map { merged ->
        // Snapshot decoding mints UI ids. Keep the id of a row already on
        // screen so its terminal run and optimistic-user bookkeeping survive.
        val retained = live.firstOrNull { row ->
            row.id !in retainedIds && row.sameTranscriptRow(merged)
        }
        if (retained == null) merged else {
            retainedIds += retained.id
            merged.copy(id = retained.id)
        }
    }
}

/** Re-anchor existing terminal results after snapshot rows have joined by UUID. */
internal fun mergeHeadlessSnapshotAgentRuns(
    previousMessages: List<Message>,
    previousRuns: Map<String, AgentRunState>,
    messages: List<Message>,
): Map<String, AgentRunState> = reconstructTerminalAgentRuns(messages).toMutableMap().apply {
    previousMessages.forEach { previous ->
        val run = previousRuns[previous.id] ?: return@forEach
        val retained = messages.firstOrNull { it.id == previous.id }
            ?: messages.firstOrNull { it.sameTranscriptRow(previous) }
            ?: return@forEach
        if (run.outcome != AgentRunOutcome.Finished || get(retained.id)?.outcome == AgentRunOutcome.Finished) {
            put(retained.id, run)
        }
    }
}

private fun Message.sameTranscriptRow(other: Message): Boolean {
    if (role != other.role) return false
    if (id == other.id) return true
    val identities = persistedIdentities()
    return identities.isNotEmpty() && other.persistedIdentities().any { it in identities }
}

private fun mergeTranscriptMessage(history: List<Message>, incoming: Message): MergedTranscriptMessage {
    val identities = incoming.persistedIdentities()
    val matches = history.withIndex().filter { (_, row) -> row.sameTranscriptRow(incoming) }
    if (matches.isEmpty()) return MergedTranscriptMessage(history + incoming, incoming, emptySet())
    val previous = matches.map { it.value }
    val retained = previous.first()
    val tools = previous.flatMap { it.blocks.filterIsInstance<MessageContent.Tool>() }
        .associate { it.call.id to it.call }
    val seenTools = mutableSetOf<String>()
    val blocks = incoming.blocks.map { block ->
        if (block !is MessageContent.Tool) block else {
            seenTools += block.call.id
            val old = tools[block.call.id]
            if (old == null) block else MessageContent.Tool(block.call.mergeSnapshotTool(old))
        }
    }
    val missingTools = tools.filterKeys { it !in seenTools }.values.map { MessageContent.Tool(it) }
    val combinedBlocks = if (missingTools.isEmpty()) blocks else
        blocks.ifEmpty { incoming.text.takeIf(String::isNotEmpty)?.let { listOf(MessageContent.Text(it)) }.orEmpty() } + missingTools
    val merged = incoming.copy(
        id = retained.id,
        transcriptUuid = incoming.transcriptUuid ?: retained.transcriptUuid,
        transcriptAliases = identities + previous.flatMap { it.persistedIdentities() },
        sentAtMs = incoming.sentAtMs ?: retained.sentAtMs,
        blocks = combinedBlocks,
        streamed = incoming.streamed || previous.any { it.streamed },
    )
    val indices = matches.mapTo(mutableSetOf()) { it.index }
    val rows = history.mapIndexedNotNull { index, row ->
        when {
            index == matches.first().index -> merged
            index in indices -> null
            else -> row
        }
    }
    return MergedTranscriptMessage(rows, merged, previous.drop(1).mapTo(mutableSetOf()) { it.id })
}

/** Earlier replay cannot turn an already persisted result back into a spinner. */
private fun ToolCallUi.mergeSnapshotTool(previous: ToolCallUi): ToolCallUi = copy(
    header = header ?: previous.header,
    display = display ?: previous.display,
    status = if (status in setOf(AgentToolStatus.Running, AgentToolStatus.Unknown) &&
        previous.status in setOf(AgentToolStatus.Completed, AgentToolStatus.Failed, AgentToolStatus.Cancelled)) previous.status else status,
    syntheticShellCommand = syntheticShellCommand ?: previous.syntheticShellCommand,
    planMarkdown = planMarkdown ?: previous.planMarkdown,
    questionAnswers = questionAnswers ?: previous.questionAnswers,
    localApp = localApp ?: previous.localApp,
    skillBody = skillBody ?: previous.skillBody,
    spawnedAgentId = spawnedAgentId ?: previous.spawnedAgentId,
)

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
