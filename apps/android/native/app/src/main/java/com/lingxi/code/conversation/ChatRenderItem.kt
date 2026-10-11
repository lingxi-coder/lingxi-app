package com.lingxi.code.conversation

import androidx.compose.runtime.Immutable

/**
 * One row of the conversation transcript, in render order. The LazyColumn in
 * `ChatScreen` iterates exactly ONE list of these — built by
 * [buildChatRenderItems] — instead of stitching together separate `items()`
 * blocks, so ordering lives in one testable place. Message, shell, empty, and
 * streaming rows keep stable keys for scroll anchoring and animations. Running
 * agents and questions intentionally live outside this transcript
 * list; terminal Agent results are durable rows anchored after their message.
 */
sealed interface ChatRenderItem {
    /** LazyColumn item key — stable across recompositions. */
    val key: String

    /** LazyColumn contentType, for slot reuse across items of the same shape. */
    val contentType: String

    /** The empty-chat hero shown before any message exists. */
    data object Empty : ChatRenderItem {
        override val key: String get() = "empty"
        override val contentType: String get() = "empty"
    }

    /** A settled transcript message (user or assistant). */
    @Immutable
    data class Message(val message: com.lingxi.code.model.Message) : ChatRenderItem {
        override val key: String get() = message.id
        override val contentType: String get() = "message"
    }

    /**
     * The assistant message currently receiving deltas. Same contentType as
     * [Message] — it becomes one when the turn settles, keeping its id/key.
     */
    @Immutable
    data class Streaming(val message: com.lingxi.code.model.Message) : ChatRenderItem {
        override val key: String get() = message.id
        override val contentType: String get() = "message"
    }

    /**
     * An inline visualization slot. Keyed by its message and ordinal so the
     * same WebView survives the streaming row settling into a message.
     */
    @Immutable
    data class Visualization(
        val messageId: String,
        val ordinal: Int,
        val status: VisualizationSlotStatus,
        val reference: VisualizationRef?,
    ) : ChatRenderItem {
        override val key: String get() = "$messageId:visualization:$ordinal"
        override val contentType: String get() = "visualization"
    }

    /** Consecutive tools can span provider message envelopes within a turn. */
    @Immutable
    data class Tools(val calls: List<ToolCallUi>) : ChatRenderItem {
        override val key get() = "tool-group:${calls.first().id}"
        override val contentType get() = "tool_group"
    }

    /** A terminal Agent result anchored after the assistant message it produced. */
    @Immutable
    data class AgentRun(val run: AgentRunState) : ChatRenderItem {
        override val key: String get() = "agent-run-${run.turnId}"
        override val contentType: String get() = "agent_run"
    }

    /** A live/completed shell invocation's expandable terminal card. */
    @Immutable
    data class Shell(val shell: ShellToolCardState) : ChatRenderItem {
        override val key: String get() = "shell-${shell.taskId}"
        override val contentType: String get() = "shell"
    }

    /** Fallback typing indicator when a source streams without a run trace. */
    data object StreamingIndicator : ChatRenderItem {
        override val key: String get() = "streaming"
        override val contentType: String get() = "streaming"
    }
}

/**
 * Build the transcript rows in today's exact visual order: empty hero (new
 * chat only), settled messages oldest-first, the streaming message, shell
 * cards and the no-trace streaming indicator. The active agent is pinned above
 * the composer beside Tasks/Todos; terminal agents follow their associated
 * assistant message. `AskUserQuestion` uses a modal sheet. Pure — unit-tested
 * on the JVM.
 */
fun buildChatRenderItems(state: ChatState): List<ChatRenderItem> = buildList<ChatRenderItem> {
    val pinnedTurnId = agentRunForBottomPanel(state)?.turnId
    if (state.isNew && state.messages.isEmpty() && !state.streaming) {
        add(ChatRenderItem.Empty)
    }
    // A persisted ToolUse is not proof of current execution. Live/recovery
    // trace events are authoritative; incomplete historical records stay neutral.
    val authoritativeTools = buildMap<String, ToolCallUi> {
        val runs = state.agentRunsByMessageId.values + listOfNotNull(state.agentRun)
        runs.forEach { run ->
            run.tools.forEach { tool ->
                if (tool.status != AgentToolStatus.Running || run.active || run.activeWorkers > 0) {
                    put(tool.id, tool.toToolCall())
                }
            }
        }
    }
    var canMergeTools = true
    fun appendTools(calls: List<ToolCallUi>) {
        val previous = if (canMergeTools) lastOrNull() as? ChatRenderItem.Tools else null
        if (previous == null) add(ChatRenderItem.Tools(calls))
        else { removeAt(lastIndex); add(ChatRenderItem.Tools(previous.calls + calls)) }
        canMergeTools = true
    }
    val foldedMessageIds = state.messages.flatMap { it.loopFoldedItemIds }.toSet()
    state.messages.forEachIndexed { messageIndex, message ->
        if (message.id in foldedMessageIds) return@forEachIndexed
        if (message.loopWakeupStreak != null) canMergeTools = false
        if (message.role == com.lingxi.code.model.Role.Ai && message.blocks.isNotEmpty()) {
            val projectedBlocks = message.blocks.map { block ->
                if (block !is MessageContent.Tool) block else {
                    val call = block.call
                    val live = authoritativeTools[call.id]
                    MessageContent.Tool(when {
                        live != null -> call.copy(status = live.status, header = live.header ?: call.header,
                            display = live.display ?: call.display, questionAnswers = live.questionAnswers ?: call.questionAnswers)
                        call.status == AgentToolStatus.Running -> call.copy(status = AgentToolStatus.Unknown)
                        else -> call
                    })
                }
            }
            transcriptBlocks(projectedBlocks).forEachIndexed { index, block ->
                when (block) {
                    is TranscriptBlock.Plan -> {
                        canMergeTools = false
                        add(ChatRenderItem.Message(message.copy(id = "plan:${block.id}", text = block.markdown,
                            blocks = listOf(MessageContent.Tool(ToolCallUi(id = "plan:${block.id}", tool = "ExitPlanMode",
                                planMarkdown = block.markdown, status = if (block.writing) AgentToolStatus.Running else AgentToolStatus.Unknown))))))
                    }
                    is TranscriptBlock.Tools -> appendTools(block.calls)
                    is TranscriptBlock.Prose -> add(ChatRenderItem.Message(message.copy(
                        id = if (index == 0) message.id else "${message.id}:prose:$index",
                        text = block.text, blocks = listOf(MessageContent.Text(block.text)),
                    )))
                    is TranscriptBlock.Visualization -> {
                        canMergeTools = false
                        add(ChatRenderItem.Visualization(message.id, block.ordinal, block.status, block.reference))
                    }
                }
            }
        } else if (message.text.isNotBlank() || message.images.isNotEmpty() || message.blocks.isNotEmpty()) {
            add(ChatRenderItem.Message(message))
        }
        val run = state.agentRunsByMessageId[message.id]
        // Resumed JSONL has no turn IDs; Finished traces are reconstructed per
        // envelope. Only the final assistant envelope owns that synthetic footer.
        val syntheticInterior = run?.outcome == AgentRunOutcome.Finished &&
            state.messages.getOrNull(messageIndex + 1)?.role == com.lingxi.code.model.Role.Ai
        if (!syntheticInterior) {
            run?.takeUnless { it.turnId == pinnedTurnId }?.let { add(ChatRenderItem.AgentRun(it)) }
        }
        if (message.role == com.lingxi.code.model.Role.User || (run != null && !syntheticInterior)) {
            canMergeTools = false
        }
    }
    state.streamingMessage?.let { live ->
        // A visualization splits the live message into the same rows (and
        // keys) it will have once settled, so its WebView is not remounted.
        if (live.blocks.none { it is MessageContent.Visualization }) {
            add(ChatRenderItem.Streaming(live))
        } else {
            transcriptBlocks(live.blocks).forEachIndexed { index, block ->
                when (block) {
                    is TranscriptBlock.Visualization ->
                        add(ChatRenderItem.Visualization(live.id, block.ordinal, block.status, block.reference))
                    is TranscriptBlock.Prose -> add(ChatRenderItem.Streaming(live.copy(
                        id = if (index == 0) live.id else "${live.id}:prose:$index",
                        text = block.text, blocks = listOf(MessageContent.Text(block.text)),
                    )))
                    is TranscriptBlock.Plan, is TranscriptBlock.Tools -> Unit
                }
            }
        }
    }
    state.shellTools.forEach { add(ChatRenderItem.Shell(it)) }
    if (state.streaming && state.agentRun == null) {
        add(ChatRenderItem.StreamingIndicator)
    }
}.let { rows ->
    val hidden = state.messages.flatMap { it.loopFoldedItemIds }.toSet()
    if (hidden.isEmpty()) rows else rows.filterNot { it.key in hidden }
}

/** Live agent work belongs beside Tasks/Todos even if its parent turn ended. */
internal fun agentRunForBottomPanel(state: ChatState): AgentRunState? {
    state.agentRun?.takeIf { it.active || it.activeWorkers > 0 }?.let { return it }
    return state.agentRunsByMessageId.values
        .filter { it.activeWorkers > 0 }
        .maxByOrNull { it.turnId }
}

/** The one blocking question presented by the native sheet; later requests queue. */
internal fun pendingQuestionForSheet(state: ChatState) = state.pendingQuestions.firstOrNull()
