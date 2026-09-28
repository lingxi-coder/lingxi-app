package com.lingxi.code.conversation

import com.lingxi.code.bindings.AskUserQuestionRequestDto
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.model.Message
import com.lingxi.code.model.ModelOption
import com.lingxi.code.model.SessionRef

/**
 * Immutable UI state for the conversation surface. Hoisted out of the
 * composables and driven entirely by [ChatViewModel] / [ConversationSource].
 */
data class ChatState(
    val session: SessionRef,
    /** Completed/user transcript rows. The in-flight assistant row is separate. */
    val messages: List<Message>,
    /**
     * Assistant row currently receiving text deltas.
     *
     * Keeping it outside [messages] avoids copying the entire transcript for
     * every streamed token when a conversation has a large history.
     */
    val streamingMessage: Message? = null,
    /** A brand-new (empty) chat shows the empty-state hero instead of a list. */
    val isNew: Boolean = false,
    /** True while the assistant reply streams — keeps the run trace live. */
    val streaming: Boolean = false,
    /** True while ResumeSession/NewSession is awaiting engine confirmation. */
    val sessionTransitioning: Boolean = false,
    /** Cancel has stopped UI streaming but the engine has not acknowledged it yet. */
    val cancellationInFlight: Boolean = false,
    /**
     * False when the visible session has not been confirmed by the engine.
     * Sending is rejected in that state so a cached transcript can never be
     * presented as context that the engine does not actually hold.
     */
    val sessionReady: Boolean = true,
    /** The model selected in the composer chip (a real engine id once loaded). */
    val model: ModelOption,
    /**
     * The catalog the picker shows. Driven by the engine's REAL `ModelList`
     * (out-of-band, via [ConversationSource.modelState]). It remains empty until
     * the engine reports a catalog. The active row is [model].
     */
    val availableModels: List<ModelOption> = emptyList(),
    /**
     * A transient, user-visible status line (tool activity). `null` hides the
     * row. Mirrors the iOS `ConversationModel.statusLine`. Errors no longer ride
     * this dim line — they surface in [error] as a persistent banner.
     */
    val statusLine: String? = null,
    /** Latest manual `/compact` lifecycle; terminal state remains until the next action. */
    val compaction: CompactionProgressUi? = null,
    /** Live/completed shell calls for the current session, keyed by task id. */
    val shellTools: List<ShellToolCardState> = emptyList(),
    /** Latest android_use invocation, used to reopen setup guidance after dismissal. */
    val computerUseRequestKey: String? = null,
    /** The latest turn's live trace; terminal copies are anchored in the transcript. */
    val agentRun: AgentRunState? = null,
    /** Terminal Agent results, anchored after the assistant message they produced. */
    val agentRunsByMessageId: Map<String, AgentRunState> = emptyMap(),
    /** Out-of-band tasks that remain alive after their initiating turn ended. */
    val activeBackgroundTaskIds: Set<String> = emptySet(),
    /** Full task rows retained for the unified execution card. */
    val backgroundTasks: Map<String, BackgroundTaskUi> = emptyMap(),
    /** Session agent roster shown alongside workflows and tasks. */
    val sessionAgents: List<SessionAgentUi> = emptyList(),
    /** Direct resume feedback for the unified workflow row. */
    val workflowResumeState: WorkflowResumeUiState = WorkflowResumeUiState.Idle,
    /** Event-driven workflow/subagent progress for the visible session only. */
    val workflowRuns: Map<String, WorkflowRunUi> = emptyMap(),
    /**
     * A persistent, dismissible turn error. Unlike [statusLine] (which the next
     * tool-activity event overwrites and a turn clears), this survives until the
     * user dismisses it ([ChatViewModel.dismissError]) or starts a new turn —
     * so a failed reply is never lost to a transient flash. `null` hides the
     * banner.
     */
    val error: ChatError? = null,
    /**
     * Interactive `AskUserQuestion` requests awaiting the user, oldest first.
     * The FIRST one renders as a card at the transcript tail
     * ([buildChatRenderItems]); entries leave only on answer/cancel, on the
     * engine's `AskUserQuestionResolved`, on `SessionEnded`, or when the
     * engine connection itself is replaced — a question can outlive its
     * turn's stream, so `TurnEnded` deliberately does NOT clear it.
     */
    val pendingQuestions: List<AskUserQuestionRequestDto> = emptyList(),
    /**
     * The model-managed working plan, pinned above the composer. Driven by
     * `ClientEvent.PlanUpdated`, a FULL-LIST REPLACE emitted on the TodoWrite
     * CALL — an empty list is a real payload meaning "clear the panel".
     */
    val planTasks: List<PlanTaskUi> = emptyList(),
    /** Whether the plan panel shows every row instead of the capped window. */
    val planExpanded: Boolean = false,
    /**
     * A recovered durable turn is parked at WaitingForUser without a local
     * executor. The composer exposes Stop/Discard while this is true, but it
     * must not count as active work for the foreground-service lease.
     */
    val durableRecoveryBlocked: Boolean = false,
    /** A live local executor is parked at WaitingForUser; Stop still cancels it. */
    val liveTurnWaitingForUser: Boolean = false,
    /**
     * Tool-use ids whose result body/diff the user expanded.
     *
     * This lives in the MODEL layer on purpose. Both surfaces that render a tool
     * call — the transcript `LazyColumn` and the run timeline's row list —
     * RECYCLE their rows, so `rememberSaveable` inside the row would drop the
     * expansion the moment it scrolled out of view. Holding it here also makes
     * it assertable from a plain JVM reducer test.
     */
    val expandedToolCalls: Set<String> = emptySet(),
) {
    /** True while a turn is in flight or a live executor is waiting for input. */
    val isStreaming: Boolean get() = streaming || liveTurnWaitingForUser

    /** Every engine workload that needs the Android foreground-service lease. */
    val requiresBackgroundExecution: Boolean
        get() = streaming ||
            liveTurnWaitingForUser ||
            compaction?.status == CompactionProgressStatus.Running ||
            activeBackgroundTaskIds.isNotEmpty() ||
            shellTools.any { it.status == ShellToolStatus.Running } ||
            agentRun?.activeWorkers?.let { it > 0 } == true ||
            agentRunsByMessageId.values.any { it.activeWorkers > 0 } ||
            agentRun?.tools?.any { it.status == AgentToolStatus.Running } == true
}

/**
 * A user-facing turn error rendered as the dismissible banner. [message] is the
 * engine's failure reason; [kind] selects the banner's label/glyph so an auth
 * failure reads differently from a transport blip (kind-aware, per spec item 4).
 */
data class ChatError(
    val message: String,
    val kind: ChatErrorKind = ChatErrorKind.GENERIC,
)

/**
 * Coarse error classification for the banner. Derived from the engine error
 * MESSAGE (the reply stream carries a flat string, not the wire `ErrorKindDto`),
 * so the UI can lead with a kind-appropriate headline.
 */
enum class ChatErrorKind {
    /** Missing / rejected credentials (401 / "api key" / "unauthorized"). */
    AUTH,

    /** Network / transport blip (timeout, connection reset). */
    NETWORK,

    /** Anything else. */
    GENERIC,
}

enum class ConversationTurnOrigin {
    Ordinary,
    Flow,
}

enum class ConversationTurnOutcome {
    Completed,
    Failed,
    Cancelled,
}

data class ConversationTurnCompletion(
    val token: Long,
    val origin: ConversationTurnOrigin,
    val outcome: ConversationTurnOutcome,
    val finalAssistantText: String,
)

/** Classify a raw engine error message into a [ChatErrorKind] for the banner. */
internal fun classifyError(message: String): ChatErrorKind {
    val m = message.lowercase()
    return when {
        "401" in m || "api key" in m || "apikey" in m || "unauthorized" in m ||
            "authentication" in m || "未授权" in m || "密钥" in m -> ChatErrorKind.AUTH
        "timeout" in m || "timed out" in m || "connection" in m || "network" in m ||
            "transport" in m || "超时" in m || "网络" in m || "连接" in m -> ChatErrorKind.NETWORK
        else -> ChatErrorKind.GENERIC
    }
}
