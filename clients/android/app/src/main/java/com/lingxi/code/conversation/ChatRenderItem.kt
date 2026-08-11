package com.lingxi.code.conversation

import androidx.compose.runtime.Immutable
import com.lingxi.code.bindings.AskUserQuestionRequestDto

/**
 * One row of the conversation transcript, in render order. The LazyColumn in
 * `ChatScreen` iterates exactly ONE list of these — built by
 * [buildChatRenderItems] — instead of stitching together separate `items()`
 * blocks, so ordering lives in one testable place. Every [key] is IDENTICAL to
 * the key the old per-block layout used (`message.id` / `"shell-<taskId>"` /
 * `"agent-run-<turnId>"` / `"empty"` / `"streaming"`), so item identity —
 * scroll anchoring, state, animations — is preserved with no visual change.
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

    /** A live/completed shell invocation's expandable terminal card. */
    @Immutable
    data class Shell(val shell: ShellToolCardState) : ChatRenderItem {
        override val key: String get() = "shell-${shell.taskId}"
        override val contentType: String get() = "shell"
    }

    /** The latest turn's CLI-like execution trace. */
    @Immutable
    data class AgentRun(val run: AgentRunState) : ChatRenderItem {
        override val key: String get() = "agent-run-${run.turnId}"
        override val contentType: String get() = "agent-run"
    }

    /** Fallback typing indicator when a source streams without a run trace. */
    data object StreamingIndicator : ChatRenderItem {
        override val key: String get() = "streaming"
        override val contentType: String get() = "streaming"
    }

    /**
     * The FIRST pending interactive `AskUserQuestion` request, rendered as a
     * card at the transcript tail. Later pending requests surface one at a
     * time as each is answered/cancelled/resolved.
     */
    @Immutable
    data class Question(val request: AskUserQuestionRequestDto) : ChatRenderItem {
        override val key: String get() = "question-${request.requestId}"
        override val contentType: String get() = "question"
    }
}

/**
 * Build the transcript rows in today's exact visual order: empty hero (new
 * chat only), settled messages oldest-first, the streaming message, shell
 * cards, the agent-run trace, the no-trace streaming indicator, and finally
 * the pending-question card at the tail. Pure — unit-tested on the JVM.
 */
fun buildChatRenderItems(state: ChatState): List<ChatRenderItem> = buildList {
    if (state.isNew && state.messages.isEmpty() && !state.streaming) {
        add(ChatRenderItem.Empty)
    }
    state.messages.forEach { add(ChatRenderItem.Message(it)) }
    state.streamingMessage?.let { add(ChatRenderItem.Streaming(it)) }
    state.shellTools.forEach { add(ChatRenderItem.Shell(it)) }
    state.agentRun?.let { add(ChatRenderItem.AgentRun(it)) }
    if (state.streaming && state.agentRun == null) {
        add(ChatRenderItem.StreamingIndicator)
    }
    state.pendingQuestions.firstOrNull()?.let { add(ChatRenderItem.Question(it)) }
}
