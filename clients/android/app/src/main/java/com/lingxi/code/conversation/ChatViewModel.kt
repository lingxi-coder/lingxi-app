package com.lingxi.code.conversation

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.lingxi.code.R
import com.lingxi.code.bindings.AskUserQuestionRequestDto
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.ImageRefDto
import com.lingxi.code.bindings.PermissionResponseDto
import com.lingxi.code.bindings.MobileLinuxEventFfi
import com.lingxi.code.bindings.MobileLinuxEventKindFfi
import com.lingxi.code.bindings.MobileLinuxTaskSnapshotFfi
import com.lingxi.code.bindings.MobileLinuxTaskStateFfi
import com.lingxi.code.bindings.TaskStatusDto
import com.lingxi.code.bindings.TurnRecoveryStateDto
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.Message
import com.lingxi.code.model.ModelOption
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.SessionRow
import com.lingxi.code.model.canonicalSessionId
import kotlinx.coroutines.Job
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.security.SecureRandom
import java.util.Locale
import java.util.concurrent.atomic.AtomicLong

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

private const val ANDROID_COMPUTER_USE_TOOL = "android_use"

private object DurableConversationTurnIds {
    private val next = AtomicLong(
        SecureRandom().nextLong().and(Long.MAX_VALUE).coerceAtLeast(1L),
    )

    fun next(): Long = next.updateAndGet { current ->
        if (current == Long.MAX_VALUE) 1L else current + 1L
    }
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

private fun formatByteCount(bytes: Long): String = when {
    bytes >= 1024L * 1024L ->
        String.format(Locale.ROOT, "%.1f MiB", bytes / (1024.0 * 1024.0))
    bytes >= 1024L ->
        String.format(Locale.ROOT, "%.1f KiB", bytes / 1024.0)
    else -> "$bytes B"
}

/**
 * Conversation ViewModel. Holds the conversation as a [StateFlow] and exposes
 * intent functions ([send], [newChat], [openSession], [selectModel]) the
 * composables call. All engine/network concerns sit behind the injected
 * [ConversationSource], so swapping in the real UniFFI source later requires no
 * changes here beyond the constructor argument.
 *
 * The optional [savedState] persists only lightweight navigation state: session
 * id/title, composer draft, and the new-chat flag. Transcripts remain in the
 * engine's session store and are restored exclusively through ResumeSession.
 * This keeps Android's saved-state Bundle small and prevents cached UI messages
 * from impersonating engine context after process death.
 */
class ChatViewModel(
    private var source: ConversationSource = UnavailableConversationSource(),
    private val savedState: SavedStateHandle? = null,
    private var sourceGeneration: Int = 0,
    /**
     * Resolves user-facing copy from this non-`@Composable` ViewModel. Defaults
     * to [DefaultConversationStrings] (the exact zh-Hans literals) so the many
     * JVM unit tests that construct [ChatViewModel] directly — with no Android
     * `Context` — keep passing unmodified; the production call site
     * ([com.lingxi.code.RootScreen]) passes one backed by a real `Context`.
     */
    private val strings: ConversationStrings = DefaultConversationStrings,
    private val backgroundExecution: ConversationBackgroundExecution =
        ConversationBackgroundExecution.None,
) : ViewModel() {
    /** Provider reconnect token is independent from workspace-source swaps. */
    private var reconnectGeneration: Int = sourceGeneration

    private val _state = MutableStateFlow(
        run {
            // Restore only the session identity. Its transcript is authoritative
            // only after ResumeSession returns SessionResumed.
            val sessionId = savedState?.get<String>(KEY_SESSION_ID)
            val sessionTitle = savedState?.get<String>(KEY_SESSION_TITLE)
            val session =
                if (sessionId != null && sessionTitle != null) SessionRef(sessionId, sessionTitle)
                else SessionRef(id = "new", title = strings.resolve(R.string.chat_new_conversation, "新对话"))
            val requiresResume = session.id != "new"
            val seededMessages = if (requiresResume) emptyList() else source.initialMessages()
            val isNew = savedState?.get<Boolean>(KEY_IS_NEW) ?: seededMessages.isEmpty()
            ChatState(
                session = session,
                messages = seededMessages,
                isNew = isNew,
                sessionTransitioning = requiresResume,
                sessionReady = !requiresResume,
                model = EngineModelCatalog.pending,
                statusLine = if (requiresResume) {
                    strings.resolve(R.string.chat_status_resuming_session, "正在恢复会话…")
                } else {
                    null
                },
            )
        },
    )
    val state: StateFlow<ChatState> = _state.asStateFlow()
    private val _turnCompletions = MutableSharedFlow<ConversationTurnCompletion>(extraBufferCapacity = 8)
    val turnCompletions = _turnCompletions.asSharedFlow()
    private var currentTurnOrigin: ConversationTurnOrigin = ConversationTurnOrigin.Ordinary
    private var lastTurnCompletionToken: Long = Long.MIN_VALUE

    /**
     * The engine's REAL resumable-session catalog, mirrored from the source's
     * OUT-OF-BAND [ConversationSource.sessionState] (sibling of the model
     * catalog). The drawer observes this single surface to render explicit
     * loading, ready-empty, ready-with-rows, and error states.
     */
    private val _sessions = MutableStateFlow(EngineSessionState.loading())
    val sessions: StateFlow<EngineSessionState> = _sessions.asStateFlow()
    private val _sourceProjectId = MutableStateFlow<String?>(null)
    val sourceProjectId: StateFlow<String?> = _sourceProjectId.asStateFlow()

    /**
     * The conversation scope the CURRENT engine source is bound to — Global,
     * a Project workspace, or a LocalApp workspace. The generalization of
     * [sourceProjectId] (which stays for the project-only flows): the drawer
     * and the scope-restore effects branch on this.
     */
    private val _sourceScope = MutableStateFlow<ConversationScope>(ConversationScope.Global)
    val sourceScope: StateFlow<ConversationScope> = _sourceScope.asStateFlow()

    /**
     * Current engine connection for profile-global feature stores.
     *
     * Local Apps collects this flow with `collectLatest`; switching Project or
     * Provider therefore cancels the old event binding, attaches to the new
     * source, and requests authoritative app/template snapshots again.
     */
    private val _engineSource = MutableStateFlow(source)
    internal val engineSource: StateFlow<ConversationSource> = _engineSource.asStateFlow()

    /** Permission and MCP state mirrored from the same source this ViewModel owns. */
    private val _pendingPermission = MutableStateFlow(source.pendingPermission.value)
    val pendingPermission: StateFlow<PermissionPromptState?> = _pendingPermission.asStateFlow()
    private val _mcpServers = MutableStateFlow(source.mcpServers.value)
    val mcpServers: StateFlow<List<MCPServer>> = _mcpServers.asStateFlow()

    /**
     * The composer draft, persisted into [savedState] so an in-progress (unsent)
     * message survives process death. Hoisted UI owns the editable draft (see
     * `RootScreen`); this exposes the restored value + a setter the draft mirrors
     * into so the saved-state copy stays current.
     */
    val restoredDraft: String get() = savedState?.get<String>(KEY_DRAFT) ?: ""

    /** Mirror the live composer draft into saved state (called as the user types). */
    fun onDraftChanged(draft: String) {
        savedState?.set(KEY_DRAFT, draft)
    }

    /** Collector for the active reply stream. */
    private var turnJob: Job? = null
    private var backgroundTurnActive = false

    /** Best-effort explicit Stop operation that a later session switch must await. */
    private var explicitCancellation: Deferred<Result<Unit>>? = null

    /** Current Resume/New submission job. Confirmation arrives via activeSessionState. */
    private var sessionTransitionJob: Job? = null

    /** Parent job for all out-of-band flows of the currently owned source. */
    private var sourceBindingJob: Job? = null

    /** Structured runs remain partitioned by their immutable origin session. */
    private val workflowRunsBySession = mutableMapOf<String, Map<String, WorkflowRunUi>>()
    /** Task status can arrive before the first workflow callback on a separate bridge. */
    private val workflowStatusesBySession = mutableMapOf<String, Map<String, TaskStatusDto>>()

    /** Serializes Project/global Source transactions across rapid drawer taps. */
    private val workspaceSwitchMutex = Mutex()

    /** Monotonic reply-stream generation used to reject stale turn events. */
    private var turnToken: Long = 0L
    private var pendingAssistantIdentity: String? = null
    private val assistantRowsByIdentity = mutableMapOf<String, String>()
    private var reasoningBeforeResponse: String? = null
    private val assistantReasoningByIdentity = mutableMapOf<String, Pair<String, String>>()
    private var assistantIdentityTurnToken: Long = -1L
    private var durableTurnId: Long? = null
    /** Token used when a durable turn is attached without a local submit collector. */
    private var recoveredTurnToken: Long? = null
    /** Non-null only while AttachTurn is replaying retained history. */
    private var recoveryReplayTurnId: Long? = null
    /** UI-owner-local durable replay cursor. */
    private var durableTurnUiSequence: Long = 0L
    /** A recovered live event has rendered and awaits its replay envelope. */
    private var pendingRecoveredReplayAckTurnId: Long? = null
    /** Session whose SessionResumed transcript is currently authoritative. */
    private var restoredTranscriptSessionId: String? = null
    /**
     * A recovered checkpoint parked for user input while no local executor owns
     * it. Keep its durable identity until the host confirms a terminal state;
     * otherwise Send/NewSession can overwrite the only cancellation handle.
     */
    private var inactiveWaitingTurnId: Long? = null
    /** Durable id of a live local turn whose executor is parked for user input. */
    private var liveWaitingTurnId: Long? = null
    private var durableDiscardInFlightTurnId: Long? = null
    /**
     * Bounded wait for the terminal `TurnRecoveryState` that a submitted
     * discard is supposed to be confirmed by. See [discardRecoveredTurn] —
     * command acceptance is NOT terminal, and the host has an acceptance path
     * that emits no snapshot at all, so without this the latch below never
     * clears.
     */
    private var durableDiscardWatchdogJob: Job? = null
    /**
     * A cold SessionResumed transcript already contains the terminal turn
     * result.  Retained envelopes for that turn are checkpoint history, not
     * new output; remember the id until ResumeTurn's terminal confirmation so
     * an envelope that races past the source-side gate is still ignored here.
     */
    private var authoritativeTerminalTranscriptTurnId: Long? = null
    private var lastRuntimeEventSequence: ULong = 0u

    /** Monotonic session-operation generation used to reject stale failures. */
    private var sessionToken: Long = 0L

    /** Expected resume id; null while starting a new session or while idle. */
    private var pendingResumeId: String? =
        _state.value.session.id.takeIf { _state.value.sessionTransitioning }

    /** True while the current transition expects SessionStarted. */
    private var pendingNewSession: Boolean = false

    init {
        // Compatibility migration from builds that serialized every message into
        // SavedStateHandle. Never decode or display it; remove it before the next
        // state save so large legacy Bundles naturally shrink after one launch.
        savedState?.remove<ArrayList<String>>(LEGACY_KEY_TRANSCRIPT)
        bindSource()
        viewModelScope.launch {
            _state
                .map { it.requiresBackgroundExecution }
                .distinctUntilChanged()
                .collect(::setBackgroundTurnActive)
        }
        // Keep the lightweight navigation state in lock-step with the UI.
        if (savedState != null) {
            viewModelScope.launch {
                _state.collect { s -> persist(s) }
            }
        }
        // Process-death restoration is not complete until the new engine handle
        // has actually resumed the persisted session. The composer remains gated
        // until the authoritative SessionResumed transcript arrives.
        if (_state.value.sessionTransitioning) {
            beginSessionTransition(
                target = _state.value.session,
                newSession = false,
                resumeEmpty = _state.value.isNew,
                status = strings.resolve(R.string.chat_status_resuming_session, "正在恢复会话…"),
            )
        }
    }

    /**
     * Bind every out-of-band flow from the source currently owned by this
     * ViewModel. The generation check rejects a final late emission from a
     * source being replaced during provider reconnect.
     */
    private fun bindSource() {
        sourceBindingJob?.cancel()
        val boundSource = source
        val generation = sourceGeneration
        sourceBindingJob = viewModelScope.launch {
            launch {
                boundSource.modelState.collect { engine ->
                    if (sourceGeneration == generation) applyModelState(engine)
                }
            }
            launch {
                boundSource.sessionState.collect { engine ->
                    if (sourceGeneration == generation) _sessions.value = engine
                }
            }
            launch {
                boundSource.activeSessionState.collect { activated ->
                    if (sourceGeneration == generation) {
                        activated?.let {
                            applyActivatedSession(it)
                            runCatching {
                                boundSource.attachDurableTurnForUi(afterSequence = durableTurnUiSequence)
                            }.onFailure(::preserveDurableRecoveryAfterAttachFailure)
                        }
                    }
                }
            }
            launch {
                boundSource.pendingPermission.collect { prompt ->
                    if (sourceGeneration == generation) _pendingPermission.value = prompt
                }
            }
            launch {
                boundSource.workflowProgress.collect { update ->
                    if (sourceGeneration == generation) reduceWorkflowProgress(update)
                }
            }
            launch {
                boundSource.mcpServers.collect { servers ->
                    if (sourceGeneration == generation) _mcpServers.value = servers
                }
            }
            launch {
                // The generic out-of-band event stream: interactive questions
                // and background-task transitions ride here, NOT the per-turn
                // reply stream — a question can outlive its turn's stream and
                // a task can finish with no turn in flight at all.
                boundSource.clientEvents.collect { event ->
                    if (sourceGeneration == generation) reduceClientEvent(event)
                }
            }
        }
    }

    /**
     * Fold one out-of-band engine [ClientEvent] into [ChatState]. Extracted
     * (internal) so the pending-question queue semantics are unit-testable
     * with no engine:
     *
     *  - `AskUserQuestion` enqueues, deduped by `request_id` (an engine
     *    re-emit while parked must not duplicate the card).
     *  - `AskUserQuestionResolved` drops that id (answered elsewhere,
     *    cancelled, or auto-continued engine-side).
     *  - `SessionEnded` clears the queue — and ONLY it does among lifecycle
     *    events: a question deliberately survives `TurnEnded`, because the
     *    engine keeps the questionnaire parked across the turn boundary.
     *  - `TaskStatusChanged` surfaces as the transient status line.
     *  - `PlanUpdated` replaces the pinned plan checklist WHOLESALE (an empty
     *    list clears it). It rides here, not the per-turn reply stream, because
     *    the plan outlives the turn that rewrote it.
     */
    internal fun reduceClientEvent(event: ClientEvent) {
        planTasksFrom(event)?.let { tasks ->
            _state.update { it.copy(planTasks = tasks) }
            return
        }
        when (event) {
            is ClientEvent.AskUserQuestion -> {
                var inserted = false
                _state.update { s ->
                    if (s.pendingQuestions.any { it.requestId == event.request.requestId }) {
                        s
                    } else {
                        inserted = true
                        s.copy(pendingQuestions = s.pendingQuestions + event.request)
                    }
                }
                if (inserted) {
                    val sessionId = _state.value.session.id
                    if (sessionId.isNotBlank() && sessionId != "new") {
                        backgroundExecution.notifyWaitingForUser(
                            ConversationBackgroundSnapshot(
                                sessionId = sessionId,
                                turnId = durableTurnId,
                                statusText = null,
                                recoverySpec = source.recoverySpec,
                            ),
                        )
                    }
                }
            }
            is ClientEvent.AskUserQuestionResolved -> _state.update { s ->
                s.copy(pendingQuestions = s.pendingQuestions.filterNot { it.requestId == event.requestId })
            }
            is ClientEvent.SessionEnded -> _state.update {
                it.copy(pendingQuestions = emptyList(), compaction = null)
            }
            is ClientEvent.CompactionStatus -> _state.update {
                it.copy(compaction = reduceCompactionStatus(it.compaction, event.phase, event.error))
            }
            is ClientEvent.CompactionCompleted -> _state.update { state ->
                val previous = state.compaction
                state.copy(
                    compaction = CompactionProgressUi(
                        status = CompactionProgressStatus.Completed,
                        startedAtMillis = previous?.startedAtMillis ?: compactionClockMillis(),
                        messagesBefore = event.messagesBefore.toInt(),
                        messagesAfter = event.messagesAfter.toInt(),
                        bytesSaved = event.bytesSaved.toLong(),
                    ),
                )
            }
            is ClientEvent.Error -> _state.update { state ->
                val previous = state.compaction
                if (previous?.status != CompactionProgressStatus.Running ||
                    !event.message.startsWith("force_compact failed:", ignoreCase = true)) {
                    state
                } else {
                    state.copy(
                        compaction = previous.copy(
                            status = CompactionProgressStatus.Failed,
                            detail = compactFailureDetail(event.message),
                        ),
                    )
                }
            }
                is ClientEvent.TaskRow -> _state.update {
                    val task = event.task.toBackgroundTaskUi()
                    it.copy(
                        backgroundTasks = it.backgroundTasks + (task.taskId to task),
                        activeBackgroundTaskIds = it.activeBackgroundTaskIds.withTaskStatus(
                            task.taskId,
                            task.status,
                        ),
                    )
                }
            is ClientEvent.WorkflowResumed -> _state.update { state ->
                val origin = event.originSessionId?.let(::canonicalSessionId)
                if (origin != null && origin != canonicalSessionId(state.session.id)) return@update state
                val task = event.task.toBackgroundTaskUi()
                state.copy(
                    backgroundTasks = state.backgroundTasks - event.previousTaskId + (task.taskId to task),
                    activeBackgroundTaskIds = state.activeBackgroundTaskIds
                        .minus(event.previousTaskId)
                        .withTaskStatus(task.taskId, task.status),
                    workflowResumeState = WorkflowResumeUiState.Succeeded,
                    statusLine = "Workflow resumed: ${event.runId}",
                )
            }
            is ClientEvent.SessionAgentList -> _state.update { state ->
                if (canonicalSessionId(event.sessionId) != canonicalSessionId(state.session.id)) {
                    state
                } else {
                    state.copy(sessionAgents = event.agents.map { it.toSessionAgentUi() })
                }
            }
            is ClientEvent.SessionAgentUpdated -> _state.update { state ->
                if (canonicalSessionId(event.sessionId) != canonicalSessionId(state.session.id)) {
                    state
                } else {
                    val agent = event.agent.toSessionAgentUi()
                    state.copy(sessionAgents = state.sessionAgents.filterNot { it.agentId == agent.agentId } + agent)
                }
            }
            is ClientEvent.TaskStatusChanged -> {
                _state.update {
                    val belongsToVisibleSession = event.originSessionId
                        ?.let(::canonicalSessionId)
                        ?.let { origin -> origin == canonicalSessionId(it.session.id) }
                        ?: true
                    if (!belongsToVisibleSession) return@update it
                    it.copy(
                        statusLine = if (belongsToVisibleSession) {
                            // Name the task by its human description when the
                            // row list already supplied one; the bare id is the
                            // last resort, not the default.
                            taskStatusLine(
                                it.backgroundTasks[event.taskId]
                                    ?.description
                                    ?.takeIf { label -> label.isNotBlank() }
                                    ?: event.taskId,
                                event.status,
                                strings,
                                event.error,
                            )
                        } else {
                            it.statusLine
                        },
                        backgroundTasks = it.backgroundTasks.updateStatus(
                            event.taskId,
                            event.status,
                            event.error,
                        ),
                        activeBackgroundTaskIds = it.activeBackgroundTaskIds.withTaskStatus(
                            event.taskId,
                            event.status,
                        ),
                    )
                }
                event.originSessionId?.let { origin ->
                    updateWorkflowTaskStatus(origin, event.taskId, event.status)
                }
            }
            is ClientEvent.CoordinatorStatus -> updateCoordinatorWorkers(
                event.activeWorkers.toInt(),
                event.team,
            )
            is ClientEvent.TurnRecoveryState -> {
                val snapshot = event.snapshot
                if (
                    canonicalSessionId(snapshot.sessionId) !=
                        canonicalSessionId(_state.value.session.id)
                ) {
                    return
                }
                val recoveredId = snapshot.turnId.toLong()
                // Capture this before assigning the event's id below.  A
                // failed cold Resume clears the render token but deliberately
                // keeps the durable id/blocked marker so a matching terminal
                // can still release the checkpoint.
                val hadMatchingDurableIdentity = durableTurnId == recoveredId ||
                    inactiveWaitingTurnId == recoveredId ||
                    liveWaitingTurnId == recoveredId
                val isLiveTurn = turnJob != null &&
                    recoveredTurnToken == null &&
                    durableTurnId == recoveredId
                val startsRecoveredAttach = recoveredTurnToken == null &&
                    turnJob == null &&
                    !_state.value.streaming &&
                    durableTurnId != recoveredId
                if (startsRecoveredAttach) {
                    turnToken++
                    recoveredTurnToken = turnToken
                    recoveryReplayTurnId = recoveredId
                    durableTurnUiSequence = 0L
                    pendingRecoveredReplayAckTurnId = null
                    currentTurnOrigin = ConversationTurnOrigin.Ordinary
                } else if (
                    recoveredTurnToken != null &&
                    recoveryReplayTurnId == recoveredId
                ) {
                    // AttachTurn has returned and ResumeTurn has now emitted its
                    // own state. Subsequent sequenced envelopes are live copies;
                    // the following raw event is what the reducer should render.
                    recoveryReplayTurnId = null
                }
                durableTurnId = recoveredId

                val terminal = snapshot.state in setOf(
                    TurnRecoveryStateDto.COMPLETED,
                    TurnRecoveryStateDto.FAILED,
                    TurnRecoveryStateDto.CANCELLED,
                )
                val hasAuthoritativeRestoredTranscript =
                    restoredTranscriptSessionId == canonicalSessionId(snapshot.sessionId)
                val isInactiveRecoveredCheckpoint = recoveredTurnToken != null &&
                    turnJob == null &&
                    !_state.value.streaming
                // On the first attach snapshot, terminal history is replayed
                // immediately afterwards. Defer settling until ResumeTurn emits
                // the authoritative terminal state a second time.
                if (startsRecoveredAttach && terminal) {
                    if (hasAuthoritativeRestoredTranscript) {
                        // SessionResumed is the complete, persisted transcript
                        // for a terminal turn.  Do not create a synthetic live
                        // assistant row just to project the same checkpoints a
                        // second time.  Keep a marker for an event that was
                        // already queued before the source gate observed this
                        // terminal snapshot.
                        authoritativeTerminalTranscriptTurnId = recoveredId
                        recoveredTurnToken = null
                        recoveryReplayTurnId = null
                        pendingRecoveredReplayAckTurnId = null
                    }
                    return
                }

                if (authoritativeTerminalTranscriptTurnId == recoveredId && terminal) {
                    // ResumeTurn's second terminal snapshot confirms the
                    // restored transcript.  No reducer work is necessary.
                    authoritativeTerminalTranscriptTurnId = null
                    clearDurableRecoveryAfterTerminal(
                        recoveredId = recoveredId,
                        hadMatchingDurableIdentity = hadMatchingDurableIdentity,
                    )
                    return
                }

                if (terminal && liveWaitingTurnId == recoveredId) {
                    liveWaitingTurnId = null
                    _state.update { state -> state.copy(liveTurnWaitingForUser = false) }
                }

                val token = recoveredTurnToken
                when (snapshot.state) {
                    TurnRecoveryStateDto.RUNNING -> when {
                        isLiveTurn -> {
                            liveWaitingTurnId = null
                            _state.update {
                                it.copy(
                                    streaming = true,
                                    liveTurnWaitingForUser = false,
                                    durableRecoveryBlocked = false,
                                    statusLine = null,
                                )
                            }
                        }
                        token != null -> {
                            if (inactiveWaitingTurnId == recoveredId) {
                                inactiveWaitingTurnId = null
                                durableDiscardInFlightTurnId = null
                                cancelDurableDiscardWatchdog()
                            }
                            _state.update {
                                it.copy(
                                    streaming = true,
                                    liveTurnWaitingForUser = false,
                                    durableRecoveryBlocked = false,
                                    statusLine = null,
                                    agentRun = it.agentRun ?: AgentRunState(turnId = token),
                                )
                            }
                        }
                    }
                    TurnRecoveryStateDto.WAITING_FOR_USER -> {
                        if (isInactiveRecoveredCheckpoint) {
                            inactiveWaitingTurnId = recoveredId
                        } else if (isLiveTurn) {
                            liveWaitingTurnId = recoveredId
                        }
                        _state.update { current ->
                            val settled = if (isInactiveRecoveredCheckpoint) {
                                settleInactiveRecoveredTurn(current, token)
                            } else {
                                current
                            }
                            settled.copy(
                                streaming = false,
                                durableRecoveryBlocked = isInactiveRecoveredCheckpoint,
                                liveTurnWaitingForUser = isLiveTurn,
                                statusLine = strings.resolve(
                                    R.string.chat_background_waiting_text,
                                    "后台对话需要你的输入才能继续",
                                ),
                            )
                        }
                        backgroundExecution.notifyWaitingForUser(
                            ConversationBackgroundSnapshot(
                                sessionId = snapshot.sessionId,
                                turnId = recoveredId,
                                statusText = null,
                                recoverySpec = source.recoverySpec,
                            ),
                        )
                    }
                    TurnRecoveryStateDto.PAUSED_RECOVERABLE -> {
                        val keepInactiveRecovery = isInactiveRecoveredCheckpoint ||
                            inactiveWaitingTurnId == recoveredId
                        if (keepInactiveRecovery) {
                            inactiveWaitingTurnId = recoveredId
                        }
                        if (liveWaitingTurnId == recoveredId) liveWaitingTurnId = null
                        _state.update { current ->
                            settleInactiveRecoveredTurn(current, token).copy(
                                streaming = false,
                                durableRecoveryBlocked = keepInactiveRecovery,
                                liveTurnWaitingForUser = false,
                                statusLine = strings.resolve(
                                    R.string.chat_background_paused_text,
                                    "后台时间已结束，打开对话即可安全恢复",
                                ),
                            )
                        }
                    }
                    TurnRecoveryStateDto.COMPLETED -> {
                        token?.let {
                            reduce(ReplyEvent.End, it)
                            _state.update { state -> state.copy(durableRecoveryBlocked = false) }
                        }
                        clearDurableRecoveryAfterTerminal(
                            recoveredId = recoveredId,
                            hadMatchingDurableIdentity = hadMatchingDurableIdentity,
                        )
                        recoveredTurnToken = null
                        recoveryReplayTurnId = null
                    }
                    TurnRecoveryStateDto.FAILED -> {
                        token?.let {
                            reduce(
                                ReplyEvent.Error(
                                    snapshot.reason ?: strings.resolve(
                                        R.string.chat_background_failed_text,
                                        "点按返回对话查看并重试",
                                    ),
                                ),
                                it,
                            )
                            _state.update { state -> state.copy(durableRecoveryBlocked = false) }
                        }
                        clearDurableRecoveryAfterTerminal(
                            recoveredId = recoveredId,
                            hadMatchingDurableIdentity = hadMatchingDurableIdentity,
                        )
                        recoveredTurnToken = null
                        recoveryReplayTurnId = null
                    }
                    TurnRecoveryStateDto.CANCELLED -> {
                        token?.let {
                            _state.update { state ->
                                val settled = state.settleTurn(
                                    run = (state.agentRun ?: AgentRunState(turnId = it))
                                        .finish(AgentRunOutcome.Cancelled),
                                    settling = state.streamingMessage,
                                )
                                state.copy(
                                    streaming = false,
                                    durableRecoveryBlocked = false,
                                    messages = settled.messages,
                                    streamingMessage = null,
                                    statusLine = null,
                                    agentRun = settled.run,
                                    agentRunsByMessageId = settled.agentRunsByMessageId,
                                )
                            }
                        }
                        clearDurableRecoveryAfterTerminal(
                            recoveredId = recoveredId,
                            hadMatchingDurableIdentity = hadMatchingDurableIdentity,
                        )
                        recoveredTurnToken = null
                        recoveryReplayTurnId = null
                    }
                }
            }
            is ClientEvent.TurnEventReplay -> {
                if (authoritativeTerminalTranscriptTurnId == event.turnId.toLong()) {
                    // The terminal SessionResumed transcript is authoritative;
                    // retained checkpoint projection would duplicate assistant
                    // prose and tool blocks.
                    return
                }
                if (
                    canonicalSessionId(event.sessionId) ==
                        canonicalSessionId(_state.value.session.id) &&
                    recoveryReplayTurnId == event.turnId.toLong()
                ) {
                    val token = recoveredTurnToken ?: return
                    retainedTurnEventToReply(event.eventJson, strings)?.let {
                        reduce(it, token)
                        durableTurnUiSequence = maxOf(durableTurnUiSequence, event.sequence.toLong())
                        if (inactiveWaitingTurnId == event.turnId.toLong()) {
                            _state.update { state ->
                                settleInactiveRecoveredTurn(state, token).copy(
                                    streaming = false,
                                    durableRecoveryBlocked = true,
                                    liveTurnWaitingForUser = false,
                                )
                            }
                        }
                    }
                } else if (pendingRecoveredReplayAckTurnId == event.turnId.toLong()) {
                    durableTurnUiSequence = maxOf(durableTurnUiSequence, event.sequence.toLong())
                    pendingRecoveredReplayAckTurnId = null
                }
            }
            else -> Unit
        }

        // A reattached turn has no `source.submit(...).collect` coroutine. Its
        // live raw events therefore arrive only on the shared client event path.
        // Sequenced envelopes update the durable cursor; render each following
        // raw event exactly once through the ordinary reducer.
        val recoveredToken = recoveredTurnToken
        if (
            recoveredToken != null &&
            event !is ClientEvent.TurnRecoveryState &&
            event !is ClientEvent.TurnEventReplay
        ) {
            clientEventToReply(event, strings)?.let { reply ->
                reduce(reply, recoveredToken)
                if (
                    reply !is ReplyEvent.End &&
                    reply !is ReplyEvent.Error &&
                    reply !is ReplyEvent.Completed &&
                    durableTurnId != null
                ) {
                    pendingRecoveredReplayAckTurnId = durableTurnId
                }
                if (reply is ReplyEvent.End || reply is ReplyEvent.Error) {
                    recoveredTurnToken = null
                    recoveryReplayTurnId = null
                    pendingRecoveredReplayAckTurnId = null
                }
            }
        }
        if (inactiveWaitingTurnId != null && recoveredToken != null) {
            _state.update { state ->
                settleInactiveRecoveredTurn(state, recoveredToken).copy(
                    streaming = false,
                    durableRecoveryBlocked = true,
                    liveTurnWaitingForUser = false,
                )
            }
        }
    }

    internal fun reduceWorkflowProgress(update: WorkflowProgressUpdate) {
        val origin = canonicalSessionId(update.originSessionId)
        if (origin.isBlank()) return
        val sessionRuns = workflowRunsBySession[origin].orEmpty()
        var reduced = reduceWorkflowProgress(sessionRuns[update.taskId], update.copy(originSessionId = origin))
            ?: return
        workflowStatusesBySession[origin]?.get(update.taskId)?.let { status ->
            reduced = reduced.copy(status = status)
        }
        val nextRuns = sessionRuns + (update.taskId to reduced)
        workflowRunsBySession[origin] = nextRuns
        if (canonicalSessionId(_state.value.session.id) == origin) {
            _state.update { it.copy(workflowRuns = nextRuns) }
        }
    }

    /** Keep a failed cold resume actionable instead of reopening the composer. */
    private fun preserveDurableRecoveryAfterAttachFailure(error: Throwable) {
        val failure = error as? DurableAttachFailure ?: return
        durableTurnId = failure.turnId
        recoveredTurnToken = null
        recoveryReplayTurnId = null
        pendingRecoveredReplayAckTurnId = null
        inactiveWaitingTurnId = failure.turnId
        durableDiscardInFlightTurnId = null
        cancelDurableDiscardWatchdog()
        _state.update {
            it.copy(
                streaming = false,
                liveTurnWaitingForUser = false,
                durableRecoveryBlocked = true,
                statusLine = strings.resolve(
                    R.string.chat_background_paused_text,
                    // The fallback must be the resource's OWN zh-Hans copy —
                    // `DefaultConversationStrings` returns it verbatim, so a
                    // drifted fallback makes every JVM test assert copy the
                    // device never renders.
                    "后台时间已结束，打开对话即可安全恢复",
                ),
                error = ChatError(
                    strings.resolve(
                        // Was `chat_error_session_switch_failed` — whose real
                        // copy is "会话切换失败"/"Failed to switch session", which
                        // is what the DEVICE rendered for a failure to REATTACH
                        // a background turn. Only the (drifted) fallback below
                        // ever said "后台对话恢复失败", and only in JVM tests.
                        R.string.chat_error_background_resume_failed,
                        "后台对话恢复失败：%1\$s",
                        failure.cause?.message ?: failure.phase,
                    ),
                    classifyError(failure.cause?.message.orEmpty()),
                ),
            )
        }
    }

    /**
     * A recovered checkpoint has no local executor. Retained envelopes can
     * still arrive after its WaitingForUser/Paused state and leave the main
     * run or shell cards looking active. Settle only rows correlated through
     * this recovered run's token; workflow/background-task state has separate
     * ownership and must remain untouched.
     */
    private fun settleInactiveRecoveredTurn(
        state: ChatState,
        token: Long?,
    ): ChatState {
        if (token == null) return state
        val mainRun = state.agentRun?.takeIf { it.turnId == token }
        val correlatedShellIds = mainRun?.tools?.map { it.id }?.toSet().orEmpty()
        val settledRun = mainRun
            ?.finish(AgentRunOutcome.Finished)
            ?.updateWorkers(0, mainRun.teamName)
        val settledShellTools = if (correlatedShellIds.isEmpty()) {
            state.shellTools
        } else {
            state.shellTools.map { shell ->
                if (
                    shell.sessionId == state.session.id &&
                        shell.taskId in correlatedShellIds &&
                        shell.status == ShellToolStatus.Running
                ) {
                    shell.copy(status = ShellToolStatus.Cancelled)
                } else {
                    shell
                }
            }
        }
        val settledRuns = state.agentRunsByMessageId.mapValues { (_, run) ->
            if (run.turnId == token) {
                run.finish(AgentRunOutcome.Finished).updateWorkers(0, run.teamName)
            } else {
                run
            }
        }
        return state.copy(
            agentRun = settledRun ?: state.agentRun,
            agentRunsByMessageId = settledRuns,
            shellTools = settledShellTools,
        )
    }

    /**
     * Release durable recovery ownership after a terminal event even when a
     * failed Attach/Resume left no render token. The event id is correlated
     * against the identity captured before the reducer assigned its snapshot
     * id, so an unrelated terminal cannot reopen the composer.
     */
    private fun clearDurableRecoveryAfterTerminal(
        recoveredId: Long,
        hadMatchingDurableIdentity: Boolean,
    ) {
        if (!hadMatchingDurableIdentity) return
        val clearsInactive = inactiveWaitingTurnId == recoveredId
        val clearsLive = liveWaitingTurnId == recoveredId
        if (clearsInactive) {
            inactiveWaitingTurnId = null
            durableDiscardInFlightTurnId = null
            cancelDurableDiscardWatchdog()
        }
        if (clearsLive) liveWaitingTurnId = null
        if (durableTurnId == recoveredId) durableTurnId = null
        _state.update { state ->
            state.copy(
                streaming = if (clearsLive) false else state.streaming,
                durableRecoveryBlocked = false,
                liveTurnWaitingForUser = if (clearsLive) false else state.liveTurnWaitingForUser,
                statusLine = if (clearsInactive || clearsLive) null else state.statusLine,
            )
        }
    }

    internal fun durableReplayCursorForTesting(): Long = durableTurnUiSequence

    private fun updateWorkflowTaskStatus(
        originSessionId: String,
        taskId: String,
        status: TaskStatusDto,
    ) {
        val origin = canonicalSessionId(originSessionId)
        if (origin.isBlank()) return
        workflowStatusesBySession[origin] =
            workflowStatusesBySession[origin].orEmpty() + (taskId to status)
        val sessionRuns = workflowRunsBySession[origin] ?: return
        val run = sessionRuns[taskId] ?: return
        val nextRuns = sessionRuns + (taskId to run.copy(status = status))
        workflowRunsBySession[origin] = nextRuns
        if (canonicalSessionId(_state.value.session.id) == origin) {
            _state.update { it.copy(workflowRuns = nextRuns) }
        }
    }

    private fun updateCoordinatorWorkers(activeWorkers: Int, team: String?) {
        _state.update { state ->
            val updated = state.agentRun?.updateWorkers(activeWorkers, team)
            state.copy(
                agentRun = updated,
                agentRunsByMessageId = state.agentRunsByMessageId.mapValues { (_, run) ->
                    when {
                        activeWorkers == 0 && run.activeWorkers > 0 ->
                            run.updateWorkers(activeWorkers, team)
                        updated != null && run.turnId == updated.turnId -> updated
                        else -> run
                    }
                },
            )
        }
    }

    /**
     * Submit the answers for the parked questionnaire [requestId] and drop its
     * card immediately (the engine's `AskUserQuestionResolved` then no-ops).
     * [answers] maps each question's full text to the selected label(s)
     * comma-joined or the free-text entry — see `assembleAskAnswers`.
     */
    fun answerQuestion(requestId: ULong, answers: Map<String, String>) {
        _state.update { s ->
            s.copy(pendingQuestions = s.pendingQuestions.filterNot { it.requestId == requestId })
        }
        viewModelScope.launch {
            runCatching {
                source.submitClientCommand(ClientCommand.AnswerAskUserQuestion(requestId, answers))
            }.onFailure { reportHostError(it.message ?: it::class.simpleName.orEmpty()) }
        }
    }

    /** Cancel the parked questionnaire [requestId] and drop its card. */
    fun cancelQuestion(requestId: ULong) {
        _state.update { s ->
            s.copy(pendingQuestions = s.pendingQuestions.filterNot { it.requestId == requestId })
        }
        viewModelScope.launch {
            runCatching {
                source.submitClientCommand(ClientCommand.CancelAskUserQuestion(requestId))
            }.onFailure { reportHostError(it.message ?: it::class.simpleName.orEmpty()) }
        }
    }

    /**
     * Refuse an action that would abandon a parked durable checkpoint — AND SAY
     * SO. Returns true when the action must not proceed.
     *
     * These guards used to be bare `return`s. A silent refusal on
     * [switchWorkspaceSource] is precisely the failure `RootScreen`'s
     * created-app landing comment describes ("the app's agent rooted in the
     * wrong directory, which is the exact failure ... observed on device"), and
     * the same silence on [send] / [openSession] / [newChat] made a tap do
     * literally nothing. The pre-existing streaming refusal right below the
     * `switchWorkspaceSource` call raises a visible banner; this one now does
     * too, and names the thing the user has to do first.
     */
    private fun refuseWhileDurableTurnParked(): Boolean {
        if (inactiveWaitingTurnId == null && liveWaitingTurnId == null) return false
        _state.update {
            it.copy(
                error = ChatError(
                    message = strings.resolve(
                        R.string.chat_error_finish_background_turn_first,
                        "请先处理后台对话（继续或丢弃），再进行此操作。",
                    ),
                    kind = ChatErrorKind.GENERIC,
                ),
            )
        }
        return true
    }

    /**
     * Discard an inactive recovered checkpoint by its durable turn id. The
     * identity remains guarded until the source delivers the correlated
     * terminal TurnRecoveryState; command acceptance alone is not terminal.
     *
     * ACCEPTANCE CAN BE THE ONLY SIGNAL. `cancel_active_turn`'s inactive branch
     * maps `DurableTurnStoreError::NotFound` / `Terminal` to `snapshot = None`
     * and returns `Ok(())` — so a checkpoint that is already gone (or already
     * terminal) is discarded successfully and emits NOTHING. Waiting only for a
     * correlated terminal state then latched [durableDiscardInFlightTurnId]
     * forever: every further Discard tap returned at the guard below, the
     * status line stayed on "正在丢弃…" and the composer stayed blocked with no
     * timeout and no retry. [armDurableDiscardWatchdog] bounds that wait.
     */
    fun discardRecoveredTurn() {
        val turnId = inactiveWaitingTurnId ?: return
        if (durableDiscardInFlightTurnId == turnId) return
        durableDiscardInFlightTurnId = turnId
        _state.update {
            it.copy(
                // Was `chat_stopping` ("正在停止…"/"Stopping…") — the device told
                // the user the turn was being STOPPED while it was being
                // discarded; only the fallback ever said "正在丢弃…".
                statusLine = strings.resolve(
                    R.string.chat_discarding,
                    "正在丢弃…",
                ),
            )
        }
        armDurableDiscardWatchdog(turnId)
        viewModelScope.launch {
            runCatching { source.discardDurableTurn(turnId) }
                .onFailure { error ->
                    if (inactiveWaitingTurnId == turnId) {
                        durableDiscardInFlightTurnId = null
                        cancelDurableDiscardWatchdog()
                        _state.update {
                            it.copy(
                                statusLine = strings.resolve(
                                    R.string.chat_background_waiting_text,
                                    "后台对话需要你的输入才能继续",
                                ),
                                error = ChatError(
                                    strings.resolve(
                                        // Was `chat_error_cancel_generation_failed`
                                        // ("取消生成失败：%1\$s") — wrong verb for a
                                        // discard, and wrong on the device.
                                        R.string.chat_error_discard_background_failed,
                                        "丢弃后台对话失败：%1\$s",
                                        "${error.message ?: error::class.simpleName}",
                                    ),
                                    classifyError(error.message.orEmpty()),
                                ),
                            )
                        }
                    }
                }
        }
    }

    /**
     * Bound the wait for a submitted discard's terminal confirmation.
     *
     * The host can accept a discard and emit no `TurnRecoveryState` at all (see
     * [discardRecoveredTurn]). When the wait expires, release the recovery
     * ownership the same way a real terminal state would — the engine ACCEPTED
     * the discard, so it is not going to run this checkpoint — and say so, so
     * the composer never stays blocked on a confirmation that is never coming.
     *
     * A correlated terminal state that does arrive first cancels this job
     * through [cancelDurableDiscardWatchdog]; the identity check makes a late
     * fire a no-op even if a cancel is missed.
     */
    private fun armDurableDiscardWatchdog(turnId: Long) {
        cancelDurableDiscardWatchdog()
        durableDiscardWatchdogJob = viewModelScope.launch {
            delay(DISCARD_CONFIRMATION_TIMEOUT_MS)
            if (durableDiscardInFlightTurnId != turnId) return@launch
            if (inactiveWaitingTurnId != turnId) return@launch
            durableDiscardWatchdogJob = null
            clearDurableRecoveryAfterTerminal(
                recoveredId = turnId,
                hadMatchingDurableIdentity = true,
            )
            _state.update {
                it.copy(
                    error = ChatError(
                        strings.resolve(
                            R.string.chat_error_discard_unconfirmed,
                            "后台对话的丢弃未获确认，已解除该对话的锁定。",
                        ),
                        ChatErrorKind.GENERIC,
                    ),
                )
            }
        }
    }

    private fun cancelDurableDiscardWatchdog() {
        durableDiscardWatchdogJob?.cancel()
        durableDiscardWatchdogJob = null
    }

    /** Resume one paused workflow without creating a new conversation turn. */
    fun resumeWorkflow(taskId: String) {
        val task = _state.value.backgroundTasks[taskId] ?: return
        if (!task.canResume || task.status != TaskStatusDto.PAUSED) return
        _state.update { it.copy(workflowResumeState = WorkflowResumeUiState.Resuming) }
        viewModelScope.launch {
            runCatching {
                source.submitClientCommand(ClientCommand.ResumeWorkflow(taskId))
            }.onFailure { error ->
                _state.update {
                    it.copy(
                        workflowResumeState = WorkflowResumeUiState.Failed,
                        statusLine = error.message ?: "Workflow resume failed",
                    )
                }
            }
        }
    }

    /** Re-pull durable execution rows after an Android foreground transition. */
    fun refreshExecutionStatus() {
        viewModelScope.launch {
            runCatching { source.refreshExecutionStatus() }
        }
    }

    /**
     * Replace the engine after an explicit provider reconnect while retaining a
     * single Activity-scoped ViewModel across configuration changes.
     *
     * Recomposition/rotation reuses the same generation and creates nothing.
     * A new generation closes the prior native source immediately, binds all UI
     * state to the replacement, and re-establishes the visible session before
     * sending is enabled.
     */
    internal fun ensureSource(
        generation: Int,
        createSource: () -> ConversationSource,
    ) {
        if (generation <= reconnectGeneration) return

        val replacement = createSource()
        if (replacement is UnavailableConversationSource) {
            replacement.close()
            _state.update {
                it.copy(
                    statusLine = null,
                    error = ChatError(
                        message = strings.resolve(
                            R.string.chat_error_engine_reconnect_failed,
                            "引擎重连失败：%1\$s",
                            replacement.reason,
                        ),
                        kind = classifyError(replacement.reason),
                    ),
                )
            }
            return
        }
        val previous = source
        reconnectGeneration = generation
        sourceGeneration++

        abandonLocalTurn()?.cancel()
        explicitCancellation?.cancel()
        explicitCancellation = null
        sessionTransitionJob?.cancel()
        sessionTransitionJob = null
        sourceBindingJob?.cancel()

        source = replacement
        _engineSource.value = replacement
        _sessions.value = EngineSessionState.loading()
        _pendingPermission.value = null
        _mcpServers.value = emptyList()
        workflowRunsBySession.clear()
        workflowStatusesBySession.clear()
        // A questionnaire's request_id is a CONNECTION-scoped correlator, so a
        // replaced engine source invalidates every parked card.
        _state.update {
            it.copy(
                pendingQuestions = emptyList(),
                activeBackgroundTaskIds = emptySet(),
                backgroundTasks = emptyMap(),
                sessionAgents = emptyList(),
                workflowResumeState = WorkflowResumeUiState.Idle,
                workflowRuns = emptyMap(),
                liveTurnWaitingForUser = false,
            )
        }
        bindSource()
        previous.close()

        val visibleSession = _state.value.session
        beginSessionTransition(
            target = visibleSession,
            newSession = visibleSession.id == "new",
            resumeEmpty = visibleSession.id != "new" && _state.value.isNew,
            status = if (visibleSession.id == "new") {
                strings.resolve(R.string.chat_status_reconnecting, "正在重新连接…")
            } else {
                strings.resolve(R.string.chat_status_resuming_session, "正在恢复会话…")
            },
            allowInactiveWaitingRecovery = true,
        )
    }

    /**
     * Transactionally replace the Android engine when the active Project
     * workspace changes. The replacement is built before the current source is
     * touched; a failed build therefore leaves the current Project/session live.
     */
    suspend fun switchWorkspaceSource(
        projectId: String?,
        target: SessionRef = SessionRef(
            id = "new",
            title = strings.resolve(R.string.chat_new_conversation, "新对话"),
        ),
        newSession: Boolean = target.id == "new",
        resumeEmpty: Boolean = false,
        replacePendingTransition: Boolean = false,
        allowInactiveWaitingRecovery: Boolean = false,
        /**
         * The scope the replacement source is bound to. Defaults to the
         * project/global split [projectId] already implies so existing project
         * flows are untouched; local-app switches pass their scope explicitly.
         */
        scope: ConversationScope = projectId?.let { ConversationScope.Project(it) }
            ?: ConversationScope.Global,
        createSource: () -> ConversationSource,
        persistSelection: suspend () -> Unit = {},
        onCommitted: () -> Unit = {},
    ): Boolean = workspaceSwitchMutex.withLock {
        if (!allowInactiveWaitingRecovery && refuseWhileDurableTurnParked()) return@withLock false
        if (_state.value.streaming ||
            (_state.value.sessionTransitioning && !replacePendingTransition)
        ) {
            _state.update {
                it.copy(
                    error = ChatError(
                        message = strings.resolve(
                            R.string.chat_error_stop_before_switch_project,
                            "请先停止当前任务，再切换项目。",
                        ),
                        kind = ChatErrorKind.GENERIC,
                    ),
                )
            }
            return@withLock false
        }
        val replacement = runCatching(createSource).getOrElse { error ->
            _state.update {
                it.copy(
                    error = ChatError(
                        message = strings.resolve(
                            R.string.chat_error_project_engine_create_failed,
                            "项目引擎创建失败：%1\$s",
                            "${error.message ?: error::class.simpleName}",
                        ),
                        kind = classifyError(error.message.orEmpty()),
                    ),
                )
            }
            return@withLock false
        }
        if (replacement is UnavailableConversationSource) {
            replacement.close()
            _state.update {
                it.copy(
                    error = ChatError(
                        message = strings.resolve(
                            R.string.chat_error_project_engine_create_failed,
                            "项目引擎创建失败：%1\$s",
                            replacement.reason,
                        ),
                        kind = classifyError(replacement.reason),
                    ),
                )
            }
            return@withLock false
        }

        val persisted = runCatching { persistSelection() }
        if (persisted.isFailure) {
            replacement.close()
            val error = persisted.exceptionOrNull()
            _state.update {
                it.copy(
                    error = ChatError(
                        message = strings.resolve(
                            R.string.chat_error_persist_active_project_failed,
                            "无法保存活动项目：%1\$s",
                            "${error?.message ?: error?.let { it::class.simpleName }}",
                        ),
                        kind = classifyError(error?.message.orEmpty()),
                    ),
                )
            }
            return@withLock false
        }

        if (allowInactiveWaitingRecovery) {
            releaseDurableRecoveryForSessionChange()
        }

        if (replacePendingTransition && _state.value.sessionTransitioning) {
            abandonPendingSessionTransition()
        }

        val previous = source
        sourceGeneration++
        sourceBindingJob?.cancel()
        source = replacement
        _engineSource.value = replacement
        _sourceProjectId.value = projectId
        _sourceScope.value = scope
        _sessions.value = EngineSessionState.loading()
        _pendingPermission.value = null
        _mcpServers.value = emptyList()
        workflowRunsBySession.clear()
        workflowStatusesBySession.clear()
        // Parked questionnaires die with the connection they were parked on.
        _state.update {
            it.copy(
                pendingQuestions = emptyList(),
                activeBackgroundTaskIds = emptySet(),
                backgroundTasks = emptyMap(),
                sessionAgents = emptyList(),
                workflowResumeState = WorkflowResumeUiState.Idle,
                workflowRuns = emptyMap(),
            )
        }
        bindSource()
        onCommitted()
        previous.close()
        beginSessionTransition(
            target = target,
            newSession = newSession,
            resumeEmpty = resumeEmpty,
            status = if (newSession) {
                strings.resolve(R.string.chat_status_new_project_session, "正在新建项目会话…")
            } else {
                strings.resolve(R.string.chat_status_resuming_project_session, "正在恢复项目会话…")
            },
            allowInactiveWaitingRecovery = allowInactiveWaitingRecovery,
        )
        true
    }

    private fun abandonPendingSessionTransition() {
        sessionToken++
        sessionTransitionJob?.cancel()
        sessionTransitionJob = null
        pendingResumeId = null
        pendingNewSession = false
        _state.update {
            it.copy(
                sessionTransitioning = false,
                sessionReady = false,
                statusLine = null,
            )
        }
    }

    fun reportHostError(message: String) {
        _state.update {
            it.copy(error = ChatError(message, classifyError(message)))
        }
    }

    suspend fun approvePermission(requestId: ULong, response: PermissionResponseDto) {
        source.approvePermission(requestId, response)
    }

    suspend fun denyPermission(requestId: ULong) {
        source.denyPermission(requestId)
    }

    fun refreshMcpServers() {
        viewModelScope.launch { source.refreshMcpServers() }
    }

    /** Write only the small navigation slice into [savedState]. */
    private fun persist(s: ChatState) {
        val sv = savedState ?: return
        sv.remove<ArrayList<String>>(LEGACY_KEY_TRANSCRIPT)
        sv[KEY_SESSION_ID] = s.session.id
        sv[KEY_SESSION_TITLE] = s.session.title
        sv[KEY_IS_NEW] = s.isNew
    }

    /**
     * Fold the engine's [EngineModelState] into [ChatState]: build the picker
     * rows from the real wire ids and select the active one. An empty catalog
     * leaves the pending selection untouched. Extracted for reducer tests.
     */
    internal fun applyModelState(engine: EngineModelState) {
        if (!engine.hasCatalog) return
        val options = EngineModelCatalog.options(engine.available, engine.details)
        val active = options.firstOrNull { it.id == engine.active } ?: options.first()
        _state.update { it.copy(availableModels = options, model = active) }
    }

    /**
     * Apply a live session activation the engine confirmed (`SessionStarted` or
     * `SessionResumed`): adopt the real session id and, for resumed sessions,
     * replace the transcript with the rehydrated oldest-first scrollback so the
     * next turn continues from real persisted history. Any in-flight turn is
     * abandoned first (the orphaned-turn guard, so a late event from the
     * pre-activation turn can't mutate the updated transcript).
     *
     * The title is preserved from the drawer's optimistic local select when the
     * id matches (`resumeSession` set it before submitting `ResumeSession`), else
     * resolved from the session catalog, else the current title — so the title
     * bar never reverts to a placeholder on a real resume. Extracted (internal)
     * so the rehydration is exercised directly in unit tests with a fake source.
     */
    internal fun applyActivatedSession(restored: ActivatedSession) {
        if (_state.value.sessionTransitioning) {
            val matchesPendingResume =
                pendingResumeId != null &&
                    restored.kind == SessionActivationKind.Resumed &&
                    restored.sessionId == pendingResumeId
            val matchesPendingNew =
                pendingNewSession && restored.kind == SessionActivationKind.Started
            if (!matchesPendingResume && !matchesPendingNew) return
        }
        abandonLocalTurn()
        pendingResumeId = null
        pendingNewSession = false
        // The previous local/recovered turn belongs to the session being
        // replaced.  Let the next recovery checkpoint establish its own id;
        // otherwise a reconnect that resumes the same session could be
        // mistaken for the already-attached turn and skip replay setup.
        durableTurnId = null
        restoredTranscriptSessionId = restored.sessionId
            .takeIf { restored.kind == SessionActivationKind.Resumed }
            ?.let(::canonicalSessionId)
        val title = _state.value.session.takeIf { it.id == restored.sessionId }?.title
            ?: _sessions.value.rows.firstOrNull { it.uuid == restored.sessionId }?.title
            ?: _state.value.session.title
        _state.update {
            it.copy(
                session = SessionRef(id = restored.sessionId, title = title),
                messages = restored.transcript, // clear-then-restore (oldest-first)
                streamingMessage = null,
                isNew = restored.kind == SessionActivationKind.Started && restored.transcript.isEmpty(),
                streaming = false,
                liveTurnWaitingForUser = false,
                sessionTransitioning = false,
                sessionReady = true,
                statusLine = null,
                shellTools = emptyList(),
                agentRun = null,
                agentRunsByMessageId = reconstructTerminalAgentRuns(restored.transcript),
                error = null,
                activeBackgroundTaskIds = emptySet(),
                backgroundTasks = emptyMap(),
                sessionAgents = emptyList(),
                workflowResumeState = WorkflowResumeUiState.Idle,
                // The plan and every expansion belong to the session that was
                // just replaced; carrying them across would attribute one
                // session's checklist to another.
                planTasks = emptyList(),
                planExpanded = false,
                expandedToolCalls = emptySet(),
                workflowRuns = workflowRunsBySession[canonicalSessionId(restored.sessionId)].orEmpty(),
            )
        }
        reportBackgroundTurnState()
    }

    /** Switch to another session through the real engine. */
    fun openSession(ref: SessionRef, empty: Boolean = false) {
        if (refuseWhileDurableTurnParked()) return
        val canonicalRef = ref.copy(id = canonicalSessionId(ref.id))
        if (canonicalRef.id.isBlank() || canonicalRef.id == "new") return
        if (_state.value.sessionReady && _state.value.session.id == canonicalRef.id) return
        beginSessionTransition(
            canonicalRef,
            newSession = false,
            resumeEmpty = empty,
            status = strings.resolve(R.string.chat_status_resuming_session, "正在恢复会话…"),
        )
    }

    /**
     * Land the user on the conversation a background notification announced.
     *
     * This must NOT go through [openSession]. Every one of these notifications
     * is posted for a turn that is parked (`WaitingForUser` /
     * `PausedRecoverable`) or has just gone terminal, so
     * [refuseWhileDurableTurnParked] is true almost by construction —
     * `openSession` refused, the caller cleared the pending request anyway, and
     * the marquee "tap the notification to get back to your turn" affordance
     * did nothing at all, with no retry and no feedback.
     *
     * [turnId] is the durable turn the notification named, and it decides two
     * things. When it is a turn this ViewModel already holds for the SAME
     * session, the user is already looking at the announced turn and there is
     * nothing to do — true even before `sessionReady` flips, which is the cold
     * start case where a second Resume would abandon the attach in flight. When
     * the destination is a different session, the parked checkpoint belongs to
     * the session being LEFT, so its latch (and the composer block it owns) is
     * released before the transition: carrying it across would hand the
     * destination a composer blocked on a turn that is not in it.
     *
     * Returns false when the request could not be started — a blank id, or a
     * session transition already in flight — so the caller can retry rather
     * than silently dropping the request.
     */
    fun openSessionFromNotification(ref: SessionRef, turnId: Long? = null): Boolean {
        val canonicalRef = ref.copy(id = canonicalSessionId(ref.id))
        if (canonicalRef.id.isBlank() || canonicalRef.id == "new") return false
        val onAnnouncedSession =
            canonicalSessionId(_state.value.session.id) == canonicalRef.id
        val holdingAnnouncedTurn = turnId != null && (
            turnId == inactiveWaitingTurnId ||
                turnId == liveWaitingTurnId ||
                turnId == durableTurnId
            )
        if (onAnnouncedSession && (holdingAnnouncedTurn || _state.value.sessionReady)) {
            // Already on the announced conversation — and, when [turnId] says
            // so, already holding the announced TURN, which is true mid-attach
            // on a cold start before `sessionReady` flips. Switching would only
            // tear down the very transcript the notification asked the user to
            // look at, and would abandon the attach in flight.
            return true
        }
        // Do not enqueue a second ambiguous Resume while the first one's
        // SessionResumed/SessionStarted is still in flight — the same rule
        // `beginSessionTransition` enforces, reported here so the caller
        // retries instead of losing the route.
        if (_state.value.sessionTransitioning && sessionTransitionJob != null) return false
        if (!onAnnouncedSession) releaseDurableRecoveryForSessionChange()
        beginSessionTransition(
            canonicalRef,
            newSession = false,
            status = strings.resolve(R.string.chat_status_resuming_session, "正在恢复会话…"),
            allowInactiveWaitingRecovery = true,
        )
        return true
    }

    /**
     * Drop the parked-checkpoint latch owned by the session being LEFT.
     *
     * Only reached once the destination is known to be a different session, so
     * the latch cannot be the announced turn's own. Carrying it across would
     * hand the destination session a composer blocked on a checkpoint that is
     * not in it, with no affordance able to release it; the destination's own
     * AttachTurn re-establishes whatever checkpoint it has.
     */
    private fun releaseDurableRecoveryForSessionChange() {
        inactiveWaitingTurnId = null
        liveWaitingTurnId = null
        durableDiscardInFlightTurnId = null
        cancelDurableDiscardWatchdog()
        _state.update {
            it.copy(durableRecoveryBlocked = false, liveTurnWaitingForUser = false)
        }
    }

    /**
     * The notification route could not be honoured after every retry. Say so —
     * the previous behaviour cleared the request and left the user staring at
     * whatever conversation happened to be open.
     */
    fun reportConversationLaunchFailed() {
        _state.update {
            it.copy(
                error = ChatError(
                    message = strings.resolve(
                        R.string.chat_error_open_conversation_failed,
                        "无法从通知打开该对话，请在会话列表中选择。",
                    ),
                    kind = ChatErrorKind.GENERIC,
                ),
            )
        }
    }

    /** Start a fresh chat through the real engine. */
    fun newChat() {
        if (refuseWhileDurableTurnParked()) return
        beginSessionTransition(
            target = SessionRef(id = "new", title = strings.resolve(R.string.chat_new_conversation, "新对话")),
            newSession = true,
            status = strings.resolve(R.string.chat_status_new_session, "正在新建会话…"),
        )
    }

    /**
     * Detach the local collector immediately and invalidate every late event.
     * The returned job is still awaited after the real engine receives Cancel.
     */
    private fun abandonLocalTurn(): Job? {
        turnToken++
        recoveredTurnToken = null
        recoveryReplayTurnId = null
        durableTurnUiSequence = 0L
        pendingRecoveredReplayAckTurnId = null
        authoritativeTerminalTranscriptTurnId = null
        liveWaitingTurnId = null
        val job = turnJob
        turnJob = null
        return job
    }

    /**
     * Cancel a detached turn in the engine, then wait for local collection to
     * stop. Cancel is intentionally submitted before cancelAndJoin: otherwise
     * unsubscribing first could hide the terminal event while the engine keeps
     * running.
     */
    private suspend fun cancelEngineTurn(job: Job?, turnId: Long? = null) {
        if (job == null && turnId == null) return
        try {
            source.cancel(turnId)
        } finally {
            job?.cancelAndJoin()
        }
    }

    /**
     * Serialize session control: any explicit Stop completes first, then the
     * active engine turn is cancelled and joined, and only then is Resume/New
     * submitted. The UI is optimistic only about the selected title; transcript
     * and send capability stay unavailable until the engine confirmation event.
     */
    private fun beginSessionTransition(
        target: SessionRef,
        newSession: Boolean,
        resumeEmpty: Boolean = false,
        status: String,
        allowInactiveWaitingRecovery: Boolean = false,
    ) {
        if (!allowInactiveWaitingRecovery && refuseWhileDurableTurnParked()) return
        // Do not enqueue a second ambiguous Resume/New while the first command
        // is accepted but its SessionResumed/SessionStarted event is still in
        // flight. Those events do not carry a client operation id.
        if (_state.value.sessionTransitioning && sessionTransitionJob != null) return
        sessionToken++
        val token = sessionToken
        pendingResumeId = target.id.takeUnless { newSession }
        pendingNewSession = newSession
        val detachedTurn = abandonLocalTurn()
        _state.update {
            it.copy(
                session = target,
                messages = emptyList(),
                streamingMessage = null,
                isNew = newSession || resumeEmpty,
                streaming = false,
                liveTurnWaitingForUser = false,
                sessionTransitioning = true,
                sessionReady = false,
                statusLine = status,
                compaction = null,
                shellTools = emptyList(),
                agentRun = null,
                agentRunsByMessageId = emptyMap(),
                error = null,
                planTasks = emptyList(),
                planExpanded = false,
                expandedToolCalls = emptySet(),
                workflowRuns = workflowRunsBySession[canonicalSessionId(target.id)].orEmpty(),
            )
        }
        sessionTransitionJob = viewModelScope.launch {
            try {
                explicitCancellation?.await()?.getOrThrow()
                explicitCancellation = null
                cancelEngineTurn(detachedTurn)
                when {
                    newSession -> source.newSession()
                    resumeEmpty -> source.resumeEmptySession(target.id, target.title)
                    else -> source.resumeSession(target.id)
                }
                // Success here means the command was accepted. sessionReady remains
                // false until SessionStarted/SessionResumed is observed.
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (t: Throwable) {
                if (token == sessionToken) {
                    pendingResumeId = null
                    pendingNewSession = false
                    _state.update {
                        it.copy(
                            streaming = false,
                            sessionTransitioning = false,
                            sessionReady = false,
                            statusLine = null,
                            error = ChatError(
                                message = strings.resolve(
                                    R.string.chat_error_session_switch_failed,
                                    "会话切换失败：%1\$s",
                                    "${t.message ?: t::class.simpleName}",
                                ),
                                kind = classifyError(t.message.orEmpty()),
                            ),
                        )
                    }
                }
            }
        }
    }

    /**
     * Change the active model. Reflects the pick locally immediately (snappy
     * chip), then submits `SetModel(id)` to the engine with the REAL wire id —
     * the engine confirms with `ModelChanged`, which re-selects the row via
     * [applyModelState].
     */
    fun selectModel(model: ModelOption) {
        if (model.id.isBlank()) return
        _state.update { it.copy(model = model) }
        viewModelScope.launch { source.setModel(model.id) }
    }

    /**
     * Ask the source to (re)report its resumable-session catalog (the drawer's
     * open trigger). Drives `ListSessions`; the reply updates [sessions]
     * out-of-band.
     */
    fun refreshSessions() {
        viewModelScope.launch { source.refreshSessions() }
    }

    /**
     * Resume a REAL engine session the user tapped in the drawer. Selects it
     * LOCALLY immediately (snappy title swap + transcript reset, so the UI
     * reflects the choice even if engine-side resume is still a follow-up), then
     * submits `ResumeSession(uuid)` so the engine swaps its inner orchestrator
     * (confirmed by `SessionResumed`, after which the resumed transcript streams).
     * Routed through [openSession] so the in-flight turn is abandoned and the
     * orphaned-turn guard holds.
     */
    fun resumeSession(row: SessionRow) {
        openSession(
            SessionRef(id = row.uuid, title = row.title),
            empty = row.messageCount == 0,
        )
    }

    /**
     * Start a fresh engine session. [newChat] clears the visible transcript and
     * submits `NewSession`; `SessionStarted` confirms the new session id.
     */
    fun startNewSession() {
        newChat()
    }

    /**
     * Submit a user turn. Appends the user message, flips [ChatState.streaming]
     * on, and collects the [ConversationSource] reply stream — appending the
     * completed assistant message and clearing the streaming flag.
     *
     * While a turn is streaming, a submit is appended to Rust's canonical
     * pending-message queue. The existing collector remains the sole owner of
     * reply events for the whole turn loop.
     */
    fun send(
        text: String,
        images: List<ImageRefDto> = emptyList(),
        origin: ConversationTurnOrigin = ConversationTurnOrigin.Ordinary,
    ) {
        val trimmed = text.trim()
        if (trimmed.isEmpty()) return
        if (refuseWhileDurableTurnParked()) return
        if (!_state.value.sessionReady || _state.value.sessionTransitioning) return
        if (explicitCancellation?.isActive == true) return
        if (_state.value.compaction?.status == CompactionProgressStatus.Running) return

        if (isManualCompactCommand(trimmed)) {
            if (images.isNotEmpty() || _state.value.streaming) return
            if (_sourceScope.value !is ConversationScope.LocalApp) savedState?.set(KEY_DRAFT, "")
            _state.update {
                it.copy(
                    isNew = false,
                    statusLine = null,
                    error = null,
                    messages = it.messages + Message(role = Role.User, text = trimmed),
                    compaction = CompactionProgressUi(
                        status = CompactionProgressStatus.Running,
                    ),
                )
            }
            viewModelScope.launch {
                runCatching { source.submitClientCommand(ClientCommand.ForceCompact) }
                    .onFailure { failure ->
                        _state.update { state ->
                            val active = state.compaction
                            if (active?.status != CompactionProgressStatus.Running) state else state.copy(
                                compaction = active.copy(
                                    status = CompactionProgressStatus.Failed,
                                    detail = failure.message ?: failure::class.simpleName.orEmpty(),
                                ),
                            )
                        }
                    }
            }
            return
        }

        if (_state.value.streaming) {
            if (_sourceScope.value !is ConversationScope.LocalApp) {
                savedState?.set(KEY_DRAFT, "")
            }
            _state.update {
                it.copy(
                    isNew = false,
                    error = null,
                    messages = it.messages + Message(
                        role = Role.User,
                        text = trimmed,
                        images = images,
                    ),
                )
            }
            viewModelScope.launch {
                runCatching {
                    source.submitClientCommand(
                        ClientCommand.SendPrompt(
                            text = trimmed,
                            promptMode = null,
                            images = images,
                            turnId = null,
                        ),
                    )
                }.onFailure { failure ->
                    _state.update {
                        it.copy(
                            error = ChatError(
                                failure.message ?: failure::class.simpleName.orEmpty(),
                                ChatErrorKind.GENERIC,
                            ),
                        )
                    }
                }
            }
            return
        }

        // A new turn supersedes any prior (e.g. just-cancelled) one — bump the
        // token so a lingering old coroutine's events are dropped by `reduce`, and
        // capture this turn's token so its own events are accepted.
        turnToken++
        val token = turnToken
        val submittedTurnId = DurableConversationTurnIds.next()
        durableTurnId = submittedTurnId
        recoveredTurnToken = null
        recoveryReplayTurnId = null
        durableTurnUiSequence = 0L
        pendingRecoveredReplayAckTurnId = null
        currentTurnOrigin = origin
        if (_sourceScope.value !is ConversationScope.LocalApp) {
            // The draft was just sent — clear the persisted copy. App scopes
            // keep their drafts in the per-scope store (RootScreen wires it),
            // so an app-scope send must not clear the project/global slot.
            savedState?.set(KEY_DRAFT, "")
        }
        _state.update {
            it.copy(
                isNew = false,
                statusLine = null,
                compaction = null,
                error = null, // a fresh turn clears the prior turn's error banner
                streaming = true, // gate the composer immediately, before the first event
                liveTurnWaitingForUser = false,
                messages = it.messages + Message(role = Role.User, text = trimmed, images = images),
                streamingMessage = null,
                agentRun = AgentRunState(turnId = token),
            )
        }
        reportBackgroundTurnState()
        // Start while this user action still has a visible Activity. Android 12+
        // generally rejects foreground-service starts attempted only after the
        // process has already crossed into the background.
        setBackgroundTurnActive(true)

        turnJob = viewModelScope.launch {
            source.submit(trimmed, images, submittedTurnId).collect { event ->
                reduce(event, token)
                if (
                    event is ReplyEvent.DurableTurnReplayAcknowledged &&
                    token == turnToken &&
                    durableTurnId == submittedTurnId
                ) {
                    durableTurnUiSequence = maxOf(durableTurnUiSequence, event.sequence)
                }
            }
        }
    }

    /**
     * Cancel the in-flight turn (or discard a recovered parked checkpoint) from
     * the composer's Stop affordance. A recovered WaitingForUser checkpoint is
     * not active execution, so it remains blocked until its correlated terminal
     * TurnRecoveryState arrives from the host.
     */
    fun cancel() {
        if (inactiveWaitingTurnId != null) {
            discardRecoveredTurn()
            return
        }
        if (!_state.value.streaming && !_state.value.liveTurnWaitingForUser) return
        val cancelledToken = turnToken
        val cancelledText = _state.value.streamingMessage?.text.orEmpty()
        // Supersede the turn: a late event arriving after the engine's Cancel
        // round-trip must not re-open streaming on the now-idle transcript.
        val job = abandonLocalTurn()
        _state.update {
            val settled = it.settleTurn(
                run = it.agentRun?.finish(AgentRunOutcome.Cancelled),
                settling = it.streamingMessage,
            )
            it.copy(
                streaming = false,
                liveTurnWaitingForUser = false,
                messages = settled.messages,
                streamingMessage = null,
                statusLine = strings.resolve(R.string.chat_stopping, "正在停止…"),
                agentRun = settled.run,
                agentRunsByMessageId = settled.agentRunsByMessageId,
            )
        }
        emitTurnCompletion(cancelledToken, ConversationTurnOutcome.Cancelled, cancelledText)
        val cancellation = viewModelScope.async {
            runCatching { cancelEngineTurn(job, durableTurnId) }
        }
        explicitCancellation = cancellation
        viewModelScope.launch {
            val result = cancellation.await()
            if (explicitCancellation === cancellation) explicitCancellation = null
            result.fold(
                onSuccess = {
                    if (!_state.value.sessionTransitioning) {
                        _state.update { it.copy(statusLine = null) }
                    }
                },
                onFailure = { t ->
                    _state.update {
                        it.copy(
                            statusLine = null,
                            error = ChatError(
                                strings.resolve(
                                    R.string.chat_error_cancel_generation_failed,
                                    "取消生成失败：%1\$s",
                                    "${t.message ?: t::class.simpleName}",
                                ),
                                classifyError(t.message.orEmpty()),
                            ),
                        )
                    }
                },
            )
        }
    }

    /** Dismiss the persistent error banner (its × affordance). */
    fun dismissError() {
        _state.update { it.copy(error = null) }
    }

    /**
     * Expand / collapse one tool call's result body or diff, keyed by its stable
     * tool-use id.
     *
     * Deliberately a ViewModel intent rather than row-local `rememberSaveable`:
     * every list that renders a tool call recycles its rows, so row-local state
     * is lost on scroll. Keeping it in [ChatState] also makes the toggle
     * assertable from a plain JVM reducer test.
     */
    fun toggleToolCall(id: String) {
        if (id.isEmpty()) return
        _state.update { s ->
            s.copy(
                expandedToolCalls = if (id in s.expandedToolCalls) {
                    s.expandedToolCalls - id
                } else {
                    s.expandedToolCalls + id
                },
            )
        }
    }

    /** Show the whole plan instead of the capped window (and back). */
    fun togglePlanExpanded() {
        _state.update { it.copy(planExpanded = !it.planExpanded) }
    }

    /**
     * Re-send the last user turn (the offline banner's "重试" affordance). Finds
     * the most recent user message and routes it back through [send] — which
     * appends a fresh turn rather than mutating history, and is itself guarded
     * against overlapping submits while streaming. A no-op when there is no prior
     * user turn or a turn is already in flight.
     */
    fun resendLast() {
        if (_state.value.streaming) return
        val lastUser = _state.value.messages.lastOrNull { it.role == Role.User } ?: return
        send(lastUser.text, images = lastUser.images)
    }

    /**
     * Reduce one [ReplyEvent] into [ChatState]. Extracted from [send] so it is
     * unit-testable with a fake source (no engine). Mirrors the iOS
     * `EngineConversationSource.apply(_:)` switch.
     *
     * [token] is the generation of the turn that produced [event] (captured when
     * its collecting coroutine launched). An event whose token no longer matches
     * the live [turnToken] is from an ABANDONED turn (session switched / new chat
     * / cancelled mid-stream) and is DROPPED — this is the orphaned-turn guard
     * that keeps a stale `Delta`/`End` from mutating the new session's transcript.
     * Defaults to the live token so direct reducer unit tests (and any in-turn
     * call) are always treated as current.
     */
    internal fun reduce(event: ReplyEvent, token: Long = turnToken) {
        if (token != turnToken) return // stale turn — its session was abandoned
        if (assistantIdentityTurnToken != token) {
            assistantIdentityTurnToken = token
            pendingAssistantIdentity = null
            reasoningBeforeResponse = null
            assistantRowsByIdentity.clear()
            assistantReasoningByIdentity.clear()
        }
        when (event) {
            is ReplyEvent.Thinking -> _state.update {
                it.copy(
                    streaming = true,
                    agentRun = (it.agentRun ?: AgentRunState(turnId = token))
                        .appendReasoning(""),
                )
            }

            is ReplyEvent.ReasoningDelta -> _state.update {
                if (reasoningBeforeResponse == null) reasoningBeforeResponse = it.agentRun?.reasoning.orEmpty()
                it.copy(
                    streaming = true,
                    agentRun = (it.agentRun ?: AgentRunState(turnId = token))
                        .appendReasoning(event.text),
                )
            }

            is ReplyEvent.Delta -> _state.update { s ->
                val run = (s.agentRun ?: AgentRunState(turnId = token)).markGenerating()
                val live = s.streamingMessage
                if (live != null) {
                    s.copy(
                        streaming = true,
                        streamingMessage = live.copy(text = live.text + event.text),
                        agentRun = run,
                    )
                } else {
                    // First delta of the turn: open a new assistant message.
                    s.copy(
                        streaming = true,
                        streamingMessage = Message(role = Role.Ai, text = event.text),
                        agentRun = run,
                    )
                }
            }

            is ReplyEvent.ToolActivity -> _state.update {
                it.copy(
                    statusLine = event.label,
                    computerUseRequestKey = if (
                        event.tool.equals(ANDROID_COMPUTER_USE_TOOL, ignoreCase = true) &&
                        event.id != null
                    ) {
                        "$token:${event.id}"
                    } else {
                        it.computerUseRequestKey
                    },
                    agentRun = (it.agentRun ?: AgentRunState(turnId = token))
                        .reduceTool(event),
                )
            }

            is ReplyEvent.ShellTool -> _state.update { current ->
                val sessionId = current.session.id
                val update = event.update
                val existing = current.shellTools.firstOrNull { it.taskId == update.taskId }
                val replacement = when (update) {
                    is ShellToolUpdate.Started -> ShellToolCardState(
                        sessionId = sessionId,
                        taskId = update.taskId,
                        command = update.command,
                        cwd = update.cwd,
                    )
                    is ShellToolUpdate.Heartbeat -> existing
                        ?.takeIf { it.sessionId == sessionId }
                        ?.copy(durationMs = update.elapsedMs)
                    is ShellToolUpdate.Finished -> existing
                        ?.takeIf { it.sessionId == sessionId }
                        ?.copy(
                            stdout = update.stdout,
                            stderr = update.stderr,
                            exitCode = update.exitCode,
                            durationMs = update.elapsedMs ?: existing.durationMs,
                            status = update.status,
                            truncated = update.truncated,
                        )
                }
                if (replacement == null) {
                    current
                } else {
                    val toolStatus = when (replacement.status) {
                        ShellToolStatus.Running -> AgentToolStatus.Running
                        ShellToolStatus.Completed -> AgentToolStatus.Completed
                        ShellToolStatus.Failed, ShellToolStatus.TimedOut -> AgentToolStatus.Failed
                        ShellToolStatus.Cancelled -> AgentToolStatus.Cancelled
                    }
                    val traceEvent = ReplyEvent.ToolActivity(
                        label = when (replacement.status) {
                            ShellToolStatus.Running -> strings.resolve(R.string.chat_shell_running, "Shell 运行中…")
                            ShellToolStatus.Completed -> strings.resolve(R.string.chat_shell_completed, "Shell 完成")
                            ShellToolStatus.Failed -> strings.resolve(R.string.chat_shell_failed, "Shell 失败")
                            ShellToolStatus.TimedOut -> strings.resolve(R.string.chat_shell_timed_out, "Shell 超时")
                            ShellToolStatus.Cancelled -> strings.resolve(R.string.chat_shell_cancelled, "Shell 已取消")
                        },
                        id = replacement.taskId,
                        tool = "Shell",
                        status = toolStatus,
                        inputSummary = replacement.command,
                        elapsedMs = replacement.durationMs,
                    )
                    current.copy(
                        statusLine = traceEvent.label,
                        shellTools = current.shellTools
                            .filterNot { it.taskId == replacement.taskId } + replacement,
                        agentRun = (current.agentRun ?: AgentRunState(turnId = token))
                            .reduceTool(traceEvent),
                    )
                }
            }

            is ReplyEvent.Notice -> _state.update {
                val kind = if (event.isError) AgentRunNoticeKind.Error else AgentRunNoticeKind.Info
                it.copy(
                    statusLine = event.message,
                    agentRun = (it.agentRun ?: AgentRunState(turnId = token))
                        .addNotice(AgentRunNotice(event.message, kind)),
                )
            }

            is ReplyEvent.Usage -> _state.update {
                it.copy(
                    agentRun = (it.agentRun ?: AgentRunState(turnId = token))
                        .updateUsage(event.usage),
                )
            }

            is ReplyEvent.Retry -> _state.update {
                val delaySeconds = event.delayMs / 1_000.0
                val label = strings.resolve(
                    R.string.chat_retry_status,
                    "请求重试 %1\$d/%2\$d（%3\$s）",
                    event.attempt,
                    event.maxRetries,
                    "${delaySeconds}s",
                )
                it.copy(
                    statusLine = label,
                    agentRun = (it.agentRun ?: AgentRunState(turnId = token))
                        .addNotice(
                            AgentRunNotice(
                                "$label：${event.message}",
                                AgentRunNoticeKind.Warning,
                            ),
                        ),
                )
            }

            is ReplyEvent.Cost -> _state.update {
                it.copy(
                    agentRun = (it.agentRun ?: AgentRunState(turnId = token))
                        .updateCost(event.formatted),
                )
            }

            is ReplyEvent.Compaction -> _state.update {
                val saved = formatByteCount(event.bytesSaved)
                it.copy(
                    agentRun = (it.agentRun ?: AgentRunState(turnId = token))
                        .addNotice(
                            AgentRunNotice(
                                strings.resolve(
                                    R.string.chat_compaction_status,
                                    "上下文已压缩：%1\$d → %2\$d 条，释放 %3\$s",
                                    event.messagesBefore,
                                    event.messagesAfter,
                                    saved,
                                ),
                            ),
                        ),
                )
            }

            is ReplyEvent.Coordinator -> {
                if (_state.value.agentRun == null) {
                    _state.update { it.copy(agentRun = AgentRunState(turnId = token)) }
                }
                updateCoordinatorWorkers(event.activeWorkers, event.team)
            }

            is ReplyEvent.MessageIdentity -> {
                pendingAssistantIdentity = event.messageId
                reasoningBeforeResponse?.let { before ->
                    assistantReasoningByIdentity[event.messageId] = before to _state.value.agentRun?.reasoning.orEmpty()
                }
                reasoningBeforeResponse = null
            }
            is ReplyEvent.MessageRetracted -> {
                val rowId = assistantRowsByIdentity.remove(event.messageId)
                val reasoning = assistantReasoningByIdentity.remove(event.messageId)
                if (rowId == null && reasoning == null) return
                _state.update { state ->
                    fun restoreReasoning(run: AgentRunState): AgentRunState {
                        if (reasoning == null) return run
                        val (before, after) = reasoning
                        val restored = when {
                            run.reasoning == after -> before
                            after.startsWith(before) && run.reasoning.startsWith(after) ->
                                before + run.reasoning.removePrefix(after)
                            else -> return run
                        }
                        return run.copy(reasoning = restored, reasoningActive = false, revision = run.revision + 1)
                    }
                    val messages = state.messages.mapNotNull { message ->
                        if (message.id != rowId || message.role != Role.Ai) message
                        else {
                            val tools = message.blocks.filterIsInstance<MessageContent.Tool>()
                            if (tools.isEmpty()) null else message.copy(text = "", blocks = tools)
                        }
                    }
                    state.copy(
                        messages = messages,
                        streamingMessage = state.streamingMessage?.takeUnless { it.id == rowId },
                        agentRun = state.agentRun?.let(::restoreReasoning),
                        agentRunsByMessageId = state.agentRunsByMessageId
                            .filterKeys { id -> id != rowId || messages.any { it.id == id } }
                            .mapValues { (_, run) -> restoreReasoning(run) },
                    )
                }
            }
            is ReplyEvent.MessageComplete -> {
                val message = event.message ?: return
                val identity = pendingAssistantIdentity
                pendingAssistantIdentity = null
                _state.update { state ->
                    val completed = state.streamingMessage
                        ?.let { live -> message.copy(id = live.id) }
                        ?: message
                    if (identity != null) assistantRowsByIdentity[identity] = completed.id
                    // Some hosts emit MessageComplete before TurnEnded. Settle
                    // the live run here so tool rows survive the later terminal
                    // event (and a subsequent turn replacing agentRun).
                    val settled = state.settleTurn(
                        run = state.agentRun,
                        settling = completed,
                    )
                    state.copy(
                        messages = settled.messages,
                        streamingMessage = null,
                        streaming = true,
                        agentRun = settled.run,
                        agentRunsByMessageId = settled.agentRunsByMessageId,
                    )
                }
            }
            is ReplyEvent.DurableTurnReplayAcknowledged -> Unit

            is ReplyEvent.Error -> {
                turnJob = null
                emitTurnCompletion(
                    token = token,
                    outcome = ConversationTurnOutcome.Failed,
                    finalAssistantText = _state.value.streamingMessage?.text.orEmpty(),
                )
                // Surface as the PERSISTENT, kind-aware banner — not the dim,
                // overwritable statusLine. Clear the status line so a stale tool
                // label doesn't linger beneath the error.
                _state.update {
                    // A failed turn still ran its tools; they settle the same way
                    // so the next turn cannot erase them either.
                    val settled = it.settleTurn(
                        run = (it.agentRun ?: AgentRunState(turnId = token))
                            .addNotice(AgentRunNotice(event.message, AgentRunNoticeKind.Error))
                            .finish(AgentRunOutcome.Failed),
                        settling = it.streamingMessage,
                    )
                    it.copy(
                        streaming = false,
                        messages = settled.messages,
                        streamingMessage = null,
                        statusLine = null,
                        error = ChatError(event.message, classifyError(event.message)),
                        agentRun = settled.run,
                        agentRunsByMessageId = settled.agentRunsByMessageId,
                    )
                }
            }

            is ReplyEvent.Completed -> {
                turnJob = null
                emitTurnCompletion(
                    token = token,
                    outcome = ConversationTurnOutcome.Completed,
                    finalAssistantText = (
                        _state.value.streamingMessage?.text
                            ?: event.message.text
                        ),
                )
                _state.update { s ->
                    val completed = s.streamingMessage
                        ?.let { live -> event.message.copy(id = live.id) }
                        ?: event.message
                    // `MessageComplete` carries the assistant's text blocks ONLY —
                    // the engine keeps this turn's ToolUse in a separate field — so
                    // the turn's tool calls have to come out of the live run trace
                    // or they die with it when the next turn starts.
                    val settled = s.settleTurn(
                        run = (s.agentRun ?: AgentRunState(turnId = token))
                            .finish(AgentRunOutcome.Completed),
                        settling = completed,
                    )
                    s.copy(
                        streaming = false,
                        messages = settled.messages,
                        streamingMessage = null,
                        agentRun = settled.run,
                        agentRunsByMessageId = settled.agentRunsByMessageId,
                    )
                }
            }

            is ReplyEvent.End -> {
                turnJob = null
                emitTurnCompletion(
                    token = token,
                    outcome = ConversationTurnOutcome.Completed,
                    finalAssistantText = _state.value.streamingMessage?.text.orEmpty(),
                )
                _state.update {
                    val run = it.agentRun ?: AgentRunState(turnId = token)
                    // The engine's mobile host never emits `MessageComplete`, so
                    // THIS is the settle point a real turn takes — it has to
                    // absorb the run's tool calls too, not just `Completed`.
                    val settled = it.settleTurn(
                        run = if (run.outcome == AgentRunOutcome.Running) {
                            run.finish(AgentRunOutcome.Completed)
                        } else {
                            run
                        },
                        settling = it.streamingMessage,
                    )
                    it.copy(
                        streaming = false,
                        messages = settled.messages,
                        streamingMessage = null,
                        agentRun = settled.run,
                        agentRunsByMessageId = settled.agentRunsByMessageId,
                    )
                }
            }
        }
        reportBackgroundTurnState()
    }

    /**
     * Fold the shared Rust MobileLinux event stream into the live shell card.
     * The sequence and current session checks make polling/reconnect idempotent;
     * once a card is terminal no later output/status can mutate it.
     */
    fun reduceMobileLinuxEvent(
        event: MobileLinuxEventFfi,
        snapshot: MobileLinuxTaskSnapshotFfi? = null,
    ) {
        if (event.sequence <= lastRuntimeEventSequence) return
        lastRuntimeEventSequence = event.sequence
        val taskId = event.taskId ?: return
        _state.update { current ->
            val direct = current.shellTools.firstOrNull { it.taskId == taskId }
            val correlated = direct ?: snapshot?.command?.let { command ->
                current.shellTools.lastOrNull {
                    it.status == ShellToolStatus.Running &&
                        (command.contains(it.command) || it.command.contains(command))
                }
            }
            val terminal = correlated?.status in setOf(
                ShellToolStatus.Completed,
                ShellToolStatus.Failed,
                ShellToolStatus.TimedOut,
                ShellToolStatus.Cancelled,
            )
            if (terminal) return@update current
            // A runtime task must correlate to a shell ToolUseStarted in this
            // conversation. Uncorrelated tasks may belong to a prior session or
            // the interactive terminal and must not leak into the chat card.
            val base = correlated ?: return@update current
            if (base.sessionId != current.session.id) return@update current
            val updated = when (event.kind) {
                MobileLinuxEventKindFfi.TASK_STATUS_CHANGED -> base.copy(
                    taskId = taskId,
                    exitCode = event.exitCode,
                    durationMs = snapshot?.let {
                        val start = it.startedAtMs
                        val finish = it.finishedAtMs
                        if (start != null && finish != null) {
                            finish.toLong() - start.toLong()
                        } else {
                            base.durationMs
                        }
                    } ?: base.durationMs,
                    status = when (event.status ?: snapshot?.status) {
                        MobileLinuxTaskStateFfi.COMPLETED -> ShellToolStatus.Completed
                        MobileLinuxTaskStateFfi.FAILED -> ShellToolStatus.Failed
                        MobileLinuxTaskStateFfi.TIMED_OUT -> ShellToolStatus.TimedOut
                        MobileLinuxTaskStateFfi.CANCELLED -> ShellToolStatus.Cancelled
                        else -> ShellToolStatus.Running
                    },
                )
                MobileLinuxEventKindFfi.STDOUT_LINE -> base.copy(
                    taskId = taskId,
                    stdout = base.stdout + event.text.orEmpty() + "\n",
                )
                MobileLinuxEventKindFfi.STDERR_CHUNK -> base.copy(
                    taskId = taskId,
                    stderr = base.stderr + event.data
                        ?.toString(Charsets.UTF_8)
                        .orEmpty(),
                )
                else -> return@update current
            }
            current.copy(
                shellTools = current.shellTools
                    .filterNot {
                        it.taskId == taskId ||
                            it.taskId == correlated.taskId
                    } + updated,
            )
        }
    }

    override fun onCleared() {
        val retained = currentBackgroundSnapshot()
            .takeIf { _state.value.requiresBackgroundExecution }
            ?.let { snapshot ->
                backgroundExecution.retainAfterUiDestroyed(source, snapshot)
            } == true
        if (retained) {
            // Drop only UI collectors. The process coordinator and foreground
            // service now own the source until terminal/attention state.
            turnJob?.cancel()
            turnJob = null
            sessionTransitionJob?.cancel()
            sourceBindingJob?.cancel()
            super.onCleared()
            return
        }
        setBackgroundTurnActive(false)
        abandonLocalTurn()?.cancel()
        sessionTransitionJob?.cancel()
        sourceBindingJob?.cancel()
        source.close()
        super.onCleared()
    }

    private fun setBackgroundTurnActive(active: Boolean) {
        if (backgroundTurnActive == active) return
        backgroundTurnActive = active
        if (!active) {
            backgroundExecution.updateTurn(null)
        } else {
            reportBackgroundTurnState()
        }
        backgroundExecution.setTurnActive(active)
    }

    private fun reportBackgroundTurnState() {
        backgroundExecution.updateTurn(currentBackgroundSnapshot())
    }

    private fun currentBackgroundSnapshot(): ConversationBackgroundSnapshot? {
        val state = _state.value
        val sessionId = state.session.id
            .takeIf { it.isNotBlank() && it != "new" }
        val turnId = durableTurnId
        val snapshot = if (
            state.requiresBackgroundExecution &&
                sessionId != null &&
                (turnId != null || state.streaming || state.activeBackgroundTaskIds.isNotEmpty())
        ) {
            ConversationBackgroundSnapshot(
                sessionId = sessionId,
                turnId = turnId,
                statusText = state.statusLine,
                recoverySpec = source.recoverySpec,
                activeTaskIds = state.activeBackgroundTaskIds,
                executorActive = state.streaming || state.liveTurnWaitingForUser,
            )
        } else {
            null
        }
        return snapshot
    }

    private fun emitTurnCompletion(
        token: Long,
        outcome: ConversationTurnOutcome,
        finalAssistantText: String,
    ) {
        if (lastTurnCompletionToken == token) return
        lastTurnCompletionToken = token
        if (outcome != ConversationTurnOutcome.Cancelled) {
            val sessionId = _state.value.session.id
            val turnId = durableTurnId
            if (sessionId.isNotBlank() && sessionId != "new" && turnId != null) {
                backgroundExecution.finishTurn(
                    ConversationBackgroundSnapshot(
                        sessionId = sessionId,
                        turnId = turnId,
                        statusText = null,
                        recoverySpec = source.recoverySpec,
                    ),
                    outcome,
                )
            }
        }
        _turnCompletions.tryEmit(
            ConversationTurnCompletion(
                token = token,
                origin = currentTurnOrigin,
                outcome = outcome,
                finalAssistantText = finalAssistantText,
            ),
        )
    }

    /** Stop action from the ongoing notification, including after UI reattachment. */
    fun cancelFromSystem(turnId: Long?) {
        if (
            inactiveWaitingTurnId != null &&
                (turnId == null || turnId == inactiveWaitingTurnId)
        ) {
            discardRecoveredTurn()
            return
        }
        if (
            (_state.value.streaming || _state.value.liveTurnWaitingForUser) &&
                (turnId == null || turnId == durableTurnId)
        ) {
            cancel()
            return
        }
        viewModelScope.launch {
            runCatching { source.cancel(turnId) }
        }
    }

    internal companion object {
        // SavedStateHandle keys for lightweight process-death navigation state.
        const val LEGACY_KEY_TRANSCRIPT = "chat.transcript" // removed on migration; never decoded
        const val KEY_DRAFT = "chat.draft" // String — unsent composer text
        const val KEY_SESSION_ID = "chat.session.id" // String
        const val KEY_SESSION_TITLE = "chat.session.title" // String
        const val KEY_IS_NEW = "chat.isNew" // Boolean — empty-state hero vs list

        /**
         * How long a submitted discard may wait for its terminal
         * `TurnRecoveryState` before the client releases the checkpoint itself.
         * Generous enough that an ordinary round-trip always confirms first;
         * short enough that a host acceptance which emits nothing does not
         * block the composer for the rest of the session.
         */
        const val DISCARD_CONFIRMATION_TIMEOUT_MS = 8_000L
    }
}

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

private fun Map<String, BackgroundTaskUi>.updateStatus(
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

private fun Set<String>.withTaskStatus(taskId: String, status: TaskStatusDto): Set<String> =
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
