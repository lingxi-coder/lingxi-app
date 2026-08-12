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
fun buildChatRenderItems(state: ChatState): List<ChatRenderItem> = buildList {
    val pinnedTurnId = agentRunForBottomPanel(state)?.turnId
    if (state.isNew && state.messages.isEmpty() && !state.streaming) {
        add(ChatRenderItem.Empty)
    }
    state.messages.forEach { message ->
        if (message.text.isNotBlank() || message.blocks.isNotEmpty()) {
            add(ChatRenderItem.Message(message))
        }
        state.agentRunsByMessageId[message.id]
            ?.takeUnless { it.turnId == pinnedTurnId }
            ?.let { add(ChatRenderItem.AgentRun(it)) }
    }
    state.streamingMessage?.let { add(ChatRenderItem.Streaming(it)) }
    state.shellTools.forEach { add(ChatRenderItem.Shell(it)) }
    if (state.streaming && state.agentRun == null) {
        add(ChatRenderItem.StreamingIndicator)
    }
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
