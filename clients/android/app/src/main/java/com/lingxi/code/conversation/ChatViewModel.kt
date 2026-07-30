package com.lingxi.code.conversation

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.lingxi.code.bindings.PermissionResponseDto
import com.lingxi.code.bindings.MobileLinuxEventFfi
import com.lingxi.code.bindings.MobileLinuxEventKindFfi
import com.lingxi.code.bindings.MobileLinuxTaskSnapshotFfi
import com.lingxi.code.bindings.MobileLinuxTaskStateFfi
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
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.util.Locale

/**
 * Immutable UI state for the conversation surface. Hoisted out of the
 * composables and driven entirely by [ChatViewModel] / [ConversationSource].
 */
data class ChatState(
    val session: SessionRef,
    val messages: List<Message>,
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
    /** Live/completed shell calls for the current session, keyed by task id. */
    val shellTools: List<ShellToolCardState> = emptyList(),
    /** Latest android_use invocation, used to reopen setup guidance after dismissal. */
    val computerUseRequestKey: String? = null,
    /** The latest turn's live CLI-like execution trace (not persisted as chat). */
    val agentRun: AgentRunState? = null,
    /**
     * A persistent, dismissible turn error. Unlike [statusLine] (which the next
     * tool-activity event overwrites and a turn clears), this survives until the
     * user dismisses it ([ChatViewModel.dismissError]) or starts a new turn —
     * so a failed reply is never lost to a transient flash. `null` hides the
     * banner.
     */
    val error: ChatError? = null,
) {
    /** True while a turn is in flight — gates the composer (Stop vs Send). */
    val isStreaming: Boolean get() = streaming
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
                else SessionRef(id = "new", title = "新对话")
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
                statusLine = if (requiresResume) "正在恢复会话…" else null,
            )
        },
    )
    val state: StateFlow<ChatState> = _state.asStateFlow()

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

    /** Index of the assistant message currently receiving streamed deltas. */
    private var streamingIndex: Int? = null

    /** Collector for the active reply stream. */
    private var turnJob: Job? = null

    /** Best-effort explicit Stop operation that a later session switch must await. */
    private var explicitCancellation: Deferred<Result<Unit>>? = null

    /** Current Resume/New submission job. Confirmation arrives via activeSessionState. */
    private var sessionTransitionJob: Job? = null

    /** Parent job for all out-of-band flows of the currently owned source. */
    private var sourceBindingJob: Job? = null

    /** Serializes Project/global Source transactions across rapid drawer taps. */
    private val workspaceSwitchMutex = Mutex()

    /** Monotonic reply-stream generation used to reject stale turn events. */
    private var turnToken: Long = 0L
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
                status = "正在恢复会话…",
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
                        activated?.let { applyActivatedSession(it) }
                    }
                }
            }
            launch {
                boundSource.pendingPermission.collect { prompt ->
                    if (sourceGeneration == generation) _pendingPermission.value = prompt
                }
            }
            launch {
                boundSource.mcpServers.collect { servers ->
                    if (sourceGeneration == generation) _mcpServers.value = servers
                }
            }
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
                        message = "引擎重连失败：${replacement.reason}",
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
        _sessions.value = EngineSessionState.loading()
        _pendingPermission.value = null
        _mcpServers.value = emptyList()
        bindSource()
        previous.close()

        val visibleSession = _state.value.session
        beginSessionTransition(
            target = visibleSession,
            newSession = visibleSession.id == "new",
            resumeEmpty = visibleSession.id != "new" && _state.value.isNew,
            status = if (visibleSession.id == "new") "正在重新连接…" else "正在恢复会话…",
        )
    }

    /**
     * Transactionally replace the Android engine when the active Project
     * workspace changes. The replacement is built before the current source is
     * touched; a failed build therefore leaves the current Project/session live.
     */
    suspend fun switchWorkspaceSource(
        projectId: String?,
        target: SessionRef = SessionRef(id = "new", title = "新对话"),
        newSession: Boolean = target.id == "new",
        resumeEmpty: Boolean = false,
        replacePendingTransition: Boolean = false,
        createSource: () -> ConversationSource,
        persistSelection: suspend () -> Unit = {},
        onCommitted: () -> Unit = {},
    ): Boolean = workspaceSwitchMutex.withLock {
        if (_state.value.streaming ||
            (_state.value.sessionTransitioning && !replacePendingTransition)
        ) {
            _state.update {
                it.copy(
                    error = ChatError(
                        message = "请先停止当前任务，再切换项目。",
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
                        message = "项目引擎创建失败：${error.message ?: error::class.simpleName}",
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
                        message = "项目引擎创建失败：${replacement.reason}",
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
                        message = "无法保存活动项目：${error?.message ?: error?.let { it::class.simpleName }}",
                        kind = classifyError(error?.message.orEmpty()),
                    ),
                )
            }
            return@withLock false
        }

        if (replacePendingTransition && _state.value.sessionTransitioning) {
            abandonPendingSessionTransition()
        }

        val previous = source
        sourceGeneration++
        sourceBindingJob?.cancel()
        source = replacement
        _sourceProjectId.value = projectId
        _sessions.value = EngineSessionState.loading()
        _pendingPermission.value = null
        _mcpServers.value = emptyList()
        bindSource()
        onCommitted()
        previous.close()
        beginSessionTransition(
            target = target,
            newSession = newSession,
            resumeEmpty = resumeEmpty,
            status = if (newSession) "正在新建项目会话…" else "正在恢复项目会话…",
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
        val options = EngineModelCatalog.options(engine.available)
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
        val title = _state.value.session.takeIf { it.id == restored.sessionId }?.title
            ?: _sessions.value.rows.firstOrNull { it.uuid == restored.sessionId }?.title
            ?: _state.value.session.title
        _state.update {
            it.copy(
                session = SessionRef(id = restored.sessionId, title = title),
                messages = restored.transcript, // clear-then-restore (oldest-first)
                isNew = restored.kind == SessionActivationKind.Started && restored.transcript.isEmpty(),
                streaming = false,
                sessionTransitioning = false,
                sessionReady = true,
                statusLine = null,
                shellTools = emptyList(),
                agentRun = null,
                error = null,
            )
        }
    }

    /** Switch to another session through the real engine. */
    fun openSession(ref: SessionRef, empty: Boolean = false) {
        val canonicalRef = ref.copy(id = canonicalSessionId(ref.id))
        if (canonicalRef.id.isBlank() || canonicalRef.id == "new") return
        if (_state.value.sessionReady && _state.value.session.id == canonicalRef.id) return
        beginSessionTransition(
            canonicalRef,
            newSession = false,
            resumeEmpty = empty,
            status = "正在恢复会话…",
        )
    }

    /** Start a fresh chat through the real engine. */
    fun newChat() {
        beginSessionTransition(
            target = SessionRef(id = "new", title = "新对话"),
            newSession = true,
            status = "正在新建会话…",
        )
    }

    /**
     * Detach the local collector immediately and invalidate every late event.
     * The returned job is still awaited after the real engine receives Cancel.
     */
    private fun abandonLocalTurn(): Job? {
        turnToken++
        val job = turnJob
        turnJob = null
        streamingIndex = null
        return job
    }

    /**
     * Cancel a detached turn in the engine, then wait for local collection to
     * stop. Cancel is intentionally submitted before cancelAndJoin: otherwise
     * unsubscribing first could hide the terminal event while the engine keeps
     * running.
     */
    private suspend fun cancelEngineTurn(job: Job?) {
        if (job == null) return
        try {
            source.cancel()
        } finally {
            job.cancelAndJoin()
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
    ) {
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
                isNew = newSession || resumeEmpty,
                streaming = false,
                sessionTransitioning = true,
                sessionReady = false,
                statusLine = status,
                shellTools = emptyList(),
                agentRun = null,
                error = null,
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
                                message = "会话切换失败：${t.message ?: t::class.simpleName}",
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
     * OVERLAPPING-SUBMIT GUARD: a submit while a turn is already streaming is
     * IGNORED (the composer shows Stop, not Send, then — but a stale tap / IME
     * Send / programmatic call must not start a second concurrent turn). The
     * guard is here (not only in the UI) so the contract holds regardless of who
     * calls `send`.
     */
    fun send(text: String) {
        val trimmed = text.trim()
        if (trimmed.isEmpty()) return
        if (_state.value.streaming) return // ignore overlapping submit while streaming
        if (!_state.value.sessionReady || _state.value.sessionTransitioning) return
        if (explicitCancellation?.isActive == true) return

        // A new turn supersedes any prior (e.g. just-cancelled) one — bump the
        // token so a lingering old coroutine's events are dropped by `reduce`, and
        // capture this turn's token so its own events are accepted.
        turnToken++
        val token = turnToken
        streamingIndex = null
        savedState?.set(KEY_DRAFT, "") // the draft was just sent — clear the persisted copy
        _state.update {
            it.copy(
                isNew = false,
                statusLine = null,
                error = null, // a fresh turn clears the prior turn's error banner
                streaming = true, // gate the composer immediately, before the first event
                messages = it.messages + Message(role = Role.User, text = trimmed),
                agentRun = AgentRunState(turnId = token),
            )
        }

        turnJob = viewModelScope.launch {
            source.submit(trimmed).collect { event -> reduce(event, token) }
        }
    }

    /**
     * Cancel the in-flight turn (the composer's Stop affordance). Fires the
     * engine's `Cancel` command, stops collecting the local reply stream, and
     * resets streaming state immediately so the composer flips back to Send
     * without waiting for the engine's `TurnEnded` to round-trip. A no-op when no
     * turn is in flight.
     */
    fun cancel() {
        if (!_state.value.streaming) return
        // Supersede the turn: a late event arriving after the engine's Cancel
        // round-trip must not re-open streaming on the now-idle transcript.
        val job = abandonLocalTurn()
        _state.update {
            it.copy(
                streaming = false,
                statusLine = "正在停止…",
                agentRun = it.agentRun?.finish(AgentRunOutcome.Cancelled),
            )
        }
        val cancellation = viewModelScope.async {
            runCatching { cancelEngineTurn(job) }
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
                                "取消生成失败：${t.message ?: t::class.simpleName}",
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
     * Re-send the last user turn (the offline banner's "重试" affordance). Finds
     * the most recent user message and routes it back through [send] — which
     * appends a fresh turn rather than mutating history, and is itself guarded
     * against overlapping submits while streaming. A no-op when there is no prior
     * user turn or a turn is already in flight.
     */
    fun resendLast() {
        if (_state.value.streaming) return
        val lastUser = _state.value.messages.lastOrNull { it.role == Role.User } ?: return
        send(lastUser.text)
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
        when (event) {
            is ReplyEvent.Thinking -> _state.update {
                it.copy(
                    streaming = true,
                    agentRun = (it.agentRun ?: AgentRunState(turnId = token))
                        .appendReasoning(""),
                )
            }

            is ReplyEvent.ReasoningDelta -> _state.update {
                it.copy(
                    streaming = true,
                    agentRun = (it.agentRun ?: AgentRunState(turnId = token))
                        .appendReasoning(event.text),
                )
            }

            is ReplyEvent.Delta -> _state.update { s ->
                val run = (s.agentRun ?: AgentRunState(turnId = token)).markGenerating()
                val i = streamingIndex
                if (i != null && s.messages.indices.contains(i)) {
                    // Append into the in-flight assistant message.
                    val updated = s.messages.toMutableList()
                    val prev = updated[i]
                    updated[i] = prev.copy(text = prev.text + event.text)
                    s.copy(streaming = true, messages = updated, agentRun = run)
                } else {
                    // First delta of the turn: open a new assistant message.
                    val opened = s.messages + Message(role = Role.Ai, text = event.text)
                    streamingIndex = opened.size - 1
                    s.copy(streaming = true, messages = opened, agentRun = run)
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
                            ShellToolStatus.Running -> "Shell 运行中…"
                            ShellToolStatus.Completed -> "Shell 完成"
                            ShellToolStatus.Failed -> "Shell 失败"
                            ShellToolStatus.TimedOut -> "Shell 超时"
                            ShellToolStatus.Cancelled -> "Shell 已取消"
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
                val label = "请求重试 ${event.attempt}/${event.maxRetries}（${delaySeconds}s）"
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
                                "上下文已压缩：${event.messagesBefore} → ${event.messagesAfter} 条，释放 $saved",
                            ),
                        ),
                )
            }

            is ReplyEvent.Coordinator -> _state.update {
                it.copy(
                    agentRun = (it.agentRun ?: AgentRunState(turnId = token))
                        .updateWorkers(event.activeWorkers, event.team),
                )
            }

            is ReplyEvent.Error -> {
                streamingIndex = null
                turnJob = null
                // Surface as the PERSISTENT, kind-aware banner — not the dim,
                // overwritable statusLine. Clear the status line so a stale tool
                // label doesn't linger beneath the error.
                _state.update {
                    it.copy(
                        streaming = false,
                        statusLine = null,
                        error = ChatError(event.message, classifyError(event.message)),
                        agentRun = (it.agentRun ?: AgentRunState(turnId = token))
                            .addNotice(AgentRunNotice(event.message, AgentRunNoticeKind.Error))
                            .finish(AgentRunOutcome.Failed),
                    )
                }
            }

            is ReplyEvent.Completed -> {
                val completedIndex = streamingIndex
                streamingIndex = null
                turnJob = null
                _state.update { s ->
                    if (completedIndex != null && s.messages.indices.contains(completedIndex)) {
                        val updated = s.messages.toMutableList()
                        updated[completedIndex] = event.message
                        s.copy(
                            streaming = false,
                            messages = updated,
                            agentRun = (s.agentRun ?: AgentRunState(turnId = token))
                                .finish(AgentRunOutcome.Completed),
                        )
                    } else {
                        s.copy(
                            streaming = false,
                            messages = s.messages + event.message,
                            agentRun = (s.agentRun ?: AgentRunState(turnId = token))
                                .finish(AgentRunOutcome.Completed),
                        )
                    }
                }
            }

            is ReplyEvent.End -> {
                streamingIndex = null
                turnJob = null
                _state.update {
                    val run = it.agentRun ?: AgentRunState(turnId = token)
                    it.copy(
                        streaming = false,
                        agentRun = if (run.outcome == AgentRunOutcome.Running) {
                            run.finish(AgentRunOutcome.Completed)
                        } else {
                            run
                        },
                    )
                }
            }
        }
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
        abandonLocalTurn()?.cancel()
        sessionTransitionJob?.cancel()
        sourceBindingJob?.cancel()
        source.close()
        super.onCleared()
    }

    private companion object {
        // SavedStateHandle keys for lightweight process-death navigation state.
        const val LEGACY_KEY_TRANSCRIPT = "chat.transcript" // removed on migration; never decoded
        const val KEY_DRAFT = "chat.draft" // String — unsent composer text
        const val KEY_SESSION_ID = "chat.session.id" // String
        const val KEY_SESSION_TITLE = "chat.session.title" // String
        const val KEY_IS_NEW = "chat.isNew" // Boolean — empty-state hero vs list
    }
}
