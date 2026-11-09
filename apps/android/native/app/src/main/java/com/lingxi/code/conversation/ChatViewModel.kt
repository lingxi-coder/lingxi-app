package com.lingxi.code.conversation

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.lingxi.code.R
import com.lingxi.code.bindings.client.AskUserQuestionAnswersDto
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ClientTextDto
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.PromptModeDto
import com.lingxi.code.bindings.client.ListingKindDto
import com.lingxi.code.bindings.client.PermissionResponseDto
import com.lingxi.code.bindings.client.TaskStatusDto
import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import com.lingxi.code.model.ConversationScope
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.Message
import com.lingxi.code.model.ModelOption
import com.lingxi.code.model.NotifConfig
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.conversationScopeFromKey
import com.lingxi.code.model.persistenceKey
import com.lingxi.code.model.toDto
import com.lingxi.code.model.toUi
import com.lingxi.code.model.SessionRow
import com.lingxi.code.model.canonicalSessionId
import java.security.SecureRandom
import java.util.Locale
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
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
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

private const val ANDROID_COMPUTER_USE_TOOL = "android_use"

private object DurableConversationTurnIds {
    private val next = AtomicLong(
        SecureRandom().nextLong().and(Long.MAX_VALUE).coerceAtLeast(1L),
    )

    fun next(): Long = next.updateAndGet { current ->
        if (current == Long.MAX_VALUE) 1L else current + 1L
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
 * id/title, composer draft, the new-chat flag, and bounded disclosure preferences. Transcripts remain in the
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
    private data class PendingSlash(val raw: String, val prompt: Message, val turnId: Long, val echoed: Boolean)
    private var pendingSlash: PendingSlash? = null
    private var settledSlashToken: Long? = null
    private var selectedModeWrite: Job? = null
    private var pendingReconnectRelease: String? = null
    private val conversationDraftWriter = ConversationDraftWriter(viewModelScope)

    /** Persist explicit device defaults in selection order across UI recreation. */
    internal fun rememberModeSelection(store: com.lingxi.code.project.ScopeStateStore, mode: SessionMode) {
        val previous = selectedModeWrite
        selectedModeWrite = viewModelScope.launch {
            previous?.join()
            try {
                store.persistSelectedMode(mode)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                reportHostError(error.message.orEmpty())
            }
        }
    }

    internal suspend fun readSelectedMode(store: com.lingxi.code.project.ScopeStateStore): SessionMode? {
        while (true) {
            val pending = selectedModeWrite
            pending?.join()
            if (pending === selectedModeWrite) return store.readSelectedMode()
        }
    }

    /** Keep draft writes ordered even when their composing screen is recreated. */
    internal fun persistConversationDraft(
        store: com.lingxi.code.project.ScopeStateStore,
        ownerScope: ConversationScope,
        sessionId: String,
        text: String,
        onFailure: (Exception) -> Unit = {},
    ): Job = conversationDraftWriter.write(
        ConversationDraftWriter.Key(ownerScope.persistenceKey(), canonicalSessionId(sessionId)), text,
        persist = { key, value -> store.persistConversationDraft(key.scopeKey, key.sessionId, value) },
        onFailure = { error -> reportHostError(error.message.orEmpty()); onFailure(error) },
    )

    internal fun retryConversationDraftWrites(
        store: com.lingxi.code.project.ScopeStateStore,
        onFailure: (Exception) -> Unit = {},
    ) {
        conversationDraftWriter.retry(
            persist = { key, value -> store.persistConversationDraft(key.scopeKey, key.sessionId, value) },
            onFailure = { error -> reportHostError(error.message.orEmpty()); onFailure(error) },
        )
    }

    internal suspend fun awaitConversationDraftWrites() {
        conversationDraftWriter.await()
    }
    internal val sideChats = SideChatManager(viewModelScope)
    private var pendingAuthoringRetry: (suspend () -> Unit)? = null
    private var authoringRetryJob: Job? = null
    private val _authoringRetryAvailable = MutableStateFlow(false)
    internal val authoringRetryAvailable: StateFlow<Boolean> = _authoringRetryAvailable.asStateFlow()

    internal fun installAuthoringRetry(action: suspend () -> Unit) {
        pendingAuthoringRetry = action
        _authoringRetryAvailable.value = false
    }

    internal fun finishAuthoringBinding() {
        pendingAuthoringRetry = null
        _authoringRetryAvailable.value = false
    }

    internal fun reportAuthoringBindingFailure(error: Throwable) {
        _authoringRetryAvailable.value = pendingAuthoringRetry != null
        reportHostError(if (com.lingxi.code.apps.isAppDevelopmentCodeRequired(error))
            strings.resolve(R.string.local_app_development_requires_code, "Chat mode does not support Local App development. Switch to Code mode to continue.")
        else error.message ?: "Authoring registration failed")
    }

    internal fun retryAuthoringBinding() {
        val action = pendingAuthoringRetry ?: return
        if (authoringRetryJob?.isActive == true) return
        _authoringRetryAvailable.value = false
        authoringRetryJob = viewModelScope.launch {
            try {
                action()
                if (pendingAuthoringRetry === action) finishAuthoringBinding()
            } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
            catch (failure: Exception) { if (pendingAuthoringRetry === action) reportAuthoringBindingFailure(failure) }
        }
    }

    internal val supportsSideChats: Boolean get() = source is EngineConversationSource

    internal fun openSideChats() {
        if (sideChats.chats.value.isNotEmpty()) sideChats.visible.value = true
        else createSideChat()
    }

    internal fun createSideChat() {
        if (sessionOwnerRejected || sideChats.isCreating.value) return
        val parent = source as? EngineConversationSource ?: return
        if (!parent.turnAdmissionReady) {
            reportHostError(strings.resolve(R.string.chat_turn_preparing_retry, "会话正在准备，请稍后重试。"))
            return
        }
        sideChats.create(input = "", draftOnFailure = null, newSource = parent::newSideChatSource) {
            reportHostError(sideChats.error.value ?: "Unable to create side chat")
        }
    }

    /** Provider reconnect token is independent from workspace-source swaps. */
    private var reconnectGeneration: Int = sourceGeneration
    private var requestedSourceGeneration: Int? = null
    private var initialRestorePrepared = false
    private val restoredNavigation = run {
        val scope = conversationScopeFromKey(savedState?.get<String>(KEY_SESSION_SCOPE))
        val id = savedState?.get<String>(KEY_SESSION_ID)?.takeIf { it.isNotBlank() }
        if (scope != null && id != null) {
            scope to SessionRef(id, savedState?.get<String>(KEY_SESSION_TITLE).orEmpty())
        } else null
    }

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
                sessionReady = !requiresResume && source !is ConnectingConversationSource,
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
     * Scheduled, or a Project workspace. The generalization of
     * [sourceProjectId] (which stays for the project-only flows): the drawer
     * and the scope-restore effects branch on this.
     */
    private val _sourceScope = MutableStateFlow<ConversationScope>(ConversationScope.Global)
    val sourceScope: StateFlow<ConversationScope> = _sourceScope.asStateFlow()
    private var sessionOwnerAdmission: ((ConversationScope, String, SessionMode) -> SessionOwnerAdmission)? = null
    internal var sessionOwnerRejected: Boolean = false
        private set

    /** The host claims a confirmed identity before it is made visible or writable. */
    internal fun setSessionOwnerAdmission(admit: (ConversationScope, String, SessionMode) -> SessionOwnerAdmission) {
        sessionOwnerAdmission = admit
    }

    private fun rejectSessionOwner() {
        sessionOwnerRejected = true
        _pendingPermission.value = null
        _state.update { it.copy(sessionReady = false, sessionTransitioning = false, statusLine = null) }
        reportConversationLaunchFailed()
    }
    private var navigationOwnerScope = restoredNavigation?.first ?: ConversationScope.Global
    private var navigationOwnerSessionId = _state.value.session.id

    private fun setNavigationOwner(sessionId: String, scope: ConversationScope) {
        navigationOwnerScope = scope
        navigationOwnerSessionId = sessionId
    }

    /**
     * Current engine connection for profile-global feature stores.
     *
     * Feature stores collect this flow with `collectLatest`; switching Project
     * or Provider therefore cancels the old event binding and attaches to the
     * new source.
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

    /** SavedState's draft is usable only for the navigation owner it restored. */
    internal fun restoredDraftFor(scope: ConversationScope, sessionId: String): String? {
        val draftScope = savedState?.get<String>(KEY_DRAFT_SCOPE)?.let(::conversationScopeFromKey)
        val draftSession = savedState?.get<String>(KEY_DRAFT_SESSION_ID)
        val ownerMatches = if (draftScope != null && draftSession != null) {
            draftScope == scope && canonicalSessionId(draftSession) == canonicalSessionId(sessionId)
        } else {
            restoredNavigation?.let {
                it.first == scope && canonicalSessionId(it.second.id) == canonicalSessionId(sessionId)
            } == true
        }
        return restoredDraft.takeIf { ownerMatches }
    }

    /** Mirror the live composer draft into saved state (called as the user types). */
    fun onDraftChanged(
        draft: String,
        ownerScope: ConversationScope = navigationOwnerScope,
        ownerSessionId: String = navigationOwnerSessionId,
    ) {
        savedState?.set(KEY_DRAFT, draft)
        savedState?.set(KEY_DRAFT_SCOPE, ownerScope.persistenceKey())
        savedState?.set(KEY_DRAFT_SESSION_ID, canonicalSessionId(ownerSessionId))
    }

    /** Collector for the active reply stream. */
    private var turnJob: Job? = null
    private var pendingSubmission: NativeSubmissionGate? = null
    private var pendingNativePrompt: NativePromptSubmission? = null
    private val pendingFollowUps = mutableMapOf<Long, NativeSubmissionGate>()
    private val nativeTurnAdmission = NativeTurnAdmission()
    private var visualizationDraftRevision = 0L
    internal fun visualizationDraftRevision(): Long = visualizationDraftRevision

    internal fun restoreVisualizationDraft(chip: VisualizationContextChip?) {
        if (chip == null) return
        visualizationDraftRevision++
        _state.update { it.copy(visualizationChip = chip) }
    }
    private var backgroundTurnActive = false

    /** Best-effort explicit Stop operation that a later session switch must await. */
    private var explicitCancellation: Deferred<Result<Unit>>? = null

    /** Current Resume/New submission job. Confirmation arrives via activeSessionState. */
    private var sessionTransitionJob: Job? = null

    /** Parent job for all out-of-band flows of the currently owned source. */
    private var sourceBindingJob: Job? = null

    /** Structured runs remain partitioned by their immutable origin session. */
    private val disclosureBySession = decodeDisclosures(savedState?.get<String>(KEY_DISCLOSURES))
    private val workflowRunsBySession = mutableMapOf<String, Map<String, WorkflowRunUi>>()
    /** Task status can arrive before the first workflow callback on a separate bridge. */
    private val workflowStatusesBySession = mutableMapOf<String, Map<String, TaskStatusDto>>()

    /** Serializes Project/global Source transactions across rapid drawer taps. */
    private val workspaceSwitchMutex = Mutex()

    /** Monotonic reply-stream generation used to reject stale turn events. */
    private var turnToken: Long = 0L
    private var pendingAssistantIdentity: String? = null
    private val optimisticRowsByToken = mutableMapOf<String, String>()
    private val assistantTranscriptUuidsByIdentity = mutableMapOf<String, List<String>>()
    private val pendingAssistantTranscriptUuids = linkedSetOf<String>()
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
    private var replayThroughSequence: Long? = null
    private val replayBuffer = java.util.TreeMap<Long, ClientEvent.TurnEventReplay>()
    private var replayTerminalState: ClientEvent.TurnRecoveryState? = null
    /** Frontier for live callbacks on a turn reattached without submit(). */
    private val recoveredProjectionFrontier = DurableTurnProjectionFrontier(strings = strings)
    /** Raw live callbacks can overlap an AttachTurn replay. */
    private val pendingReplayRaw = mutableListOf<Pair<ClientEvent, ReplyEvent>>()
    /** Projection captured only after its matching durable event was consumed. */
    private var acknowledgedTurnProjection: RetainedConversationTranscript? = null
    private var awaitingRetainedAttachSnapshot = false
    private var retainedAttachSubmissionPending = false
    private var retainedAttachSnapshotSeen = false
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
    private var terminalReplaySettledTurnId: Long? = null

    /** Monotonic session-operation generation used to reject stale failures. */
    private var sessionToken: Long = 0L

    private fun captureTurnProjection(turnId: Long, sequence: Long) {
        val state = _state.value
        if (state.session.id == "new" || durableTurnId != turnId) return
        acknowledgedTurnProjection = retainedProjection(state, turnId, sequence)
    }

    private fun flushReplayRaw(token: Long) {
        val pending = pendingReplayRaw.toList()
        pendingReplayRaw.clear()
        for ((event, reply) in pending) {
            if (recoveredTurnToken != token) break
            recoveredProjectionFrontier.rendered(event, reply)
            reduce(reply, token)
            if (reply is ReplyEvent.End || reply is ReplyEvent.Error || reply is ReplyEvent.CommandOutput) {
                recoveredTurnToken = null
                break
            }
        }
    }

    private fun retainedProjection(state: ChatState, turnId: Long?, sequence: Long) = RetainedConversationTranscript(
            sessionId = state.session.id,
            turnId = turnId,
            state = state,
            afterSequence = sequence,
            turnToken = turnToken,
            turnOrigin = currentTurnOrigin,
            pendingAssistantIdentity = pendingAssistantIdentity,
            optimisticRowsByToken = optimisticRowsByToken.toMap(),
            assistantTranscriptUuidsByIdentity = assistantTranscriptUuidsByIdentity.toMap(),
            pendingAssistantTranscriptUuids = pendingAssistantTranscriptUuids.toSet(),
            assistantRowsByIdentity = assistantRowsByIdentity.toMap(),
            reasoningBeforeResponse = reasoningBeforeResponse,
            assistantReasoningByIdentity = assistantReasoningByIdentity.toMap(),
            assistantIdentityTurnToken = assistantIdentityTurnToken,
        )

    private fun openAfterRetainedAttachIfConfirmed() {
        if (!sessionOwnerRejected && !retainedAttachSubmissionPending && retainedAttachSnapshotSeen) {
            _state.update { it.copy(sessionReady = true) }
        }
    }

    private fun finishRetainedTerminal(snapshot: com.lingxi.code.bindings.client.TurnRecoverySnapshotDto, message: String) {
        val recoveredId = snapshot.turnId.toLong()
        recoveredTurnToken?.let { token ->
            when (snapshot.state) {
                TurnRecoveryStateDto.COMPLETED -> reduce(ReplyEvent.End, token)
                TurnRecoveryStateDto.FAILED -> reduce(ReplyEvent.Error(message), token)
                TurnRecoveryStateDto.CANCELLED -> _state.update { state ->
                    val settled = state.settleTurn(
                        run = (state.agentRun ?: AgentRunState(turnId = token)).finish(AgentRunOutcome.Cancelled),
                        settling = state.streamingMessage,
                    )
                    state.copy(streaming = false, messages = settled.messages, streamingMessage = null,
                        agentRun = settled.run, agentRunsByMessageId = settled.agentRunsByMessageId)
                }
                else -> Unit
            }
        }
        terminalReplaySettledTurnId = recoveredId
        clearDurableRecoveryAfterTerminal(recoveredId, durableTurnId == recoveredId)
        recoveredTurnToken = null
        recoveryReplayTurnId = null
        recoveredProjectionFrontier.reset(durableTurnUiSequence)
        pendingReplayRaw.clear()
        replayThroughSequence = null
        replayBuffer.clear()
        replayTerminalState = null
    }

    /** Expected resume id; null while starting a new session or while idle. */
    private var pendingResumeId: String? =
        _state.value.session.id.takeIf { _state.value.sessionTransitioning }

    /** True while the current transition expects SessionStarted. */
    private var pendingNewSession: Boolean = false
    init {
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
        pendingAuthoringRetry = null
        _authoringRetryAvailable.value = false
        optimisticRowsByToken.clear()
        assistantTranscriptUuidsByIdentity.clear()
        pendingAssistantTranscriptUuids.clear()
        sourceBindingJob?.cancel()
        _state.update { it.copy(slashCommands = emptyList()) }
        val boundSource = source
        val generation = sourceGeneration
        sourceBindingJob = viewModelScope.launch {
            boundSource.coldAuthoringRecoveryState?.let { recovery ->
                launch {
                    val retry: suspend () -> Unit = { boundSource.retryColdAuthoringRecovery() }
                    recovery.collect { status ->
                        if (sourceGeneration != generation) return@collect
                        when (status) {
                            ColdAuthoringRecoveryState.Failed -> if (pendingAuthoringRetry == null || pendingAuthoringRetry === retry) {
                                installAuthoringRetry(retry)
                                reportAuthoringBindingFailure(IllegalStateException(strings.resolve(
                                    R.string.chat_turn_preparing_retry, "会话正在准备，请稍后重试。")))
                            }
                            ColdAuthoringRecoveryState.Ready -> if (pendingAuthoringRetry === retry) finishAuthoringBinding()
                            else -> Unit
                        }
                    }
                }
            }
            val eventsSubscribed = CompletableDeferred<Unit>()
            launch(start = CoroutineStart.UNDISPATCHED) {
                try {
                    boundSource.collectClientEvents({ eventsSubscribed.complete(Unit) }) { event ->
                        if (sourceGeneration == generation) reduceClientEvent(event)
                    }
                } finally {
                    eventsSubscribed.complete(Unit)
                }
            }
            launch {
                boundSource.modelState.collect { engine ->
                    if (sourceGeneration == generation) applyModelState(engine)
                }
            }
            launch {
                boundSource.sessionState.collect { engine ->
                    if (sourceGeneration == generation) {
                        _sessions.value = engine
                        // The SDK catalog resolves custom title, AI title, summary and first prompt.
                        // Keep the header and drawer synchronized, including later renames.
                        _state.update { current ->
                            val row = engine.rows.firstOrNull { canonicalSessionId(it.uuid) == canonicalSessionId(current.session.id) }
                            if (row == null) current else current.copy(session = current.session.copy(title = row.title))
                        }
                    }
                }
            }
            launch {
                boundSource.workspaceState.collect { workspace ->
                    if (sourceGeneration == generation && workspace != null &&
                        canonicalSessionId(workspace.sessionId) == canonicalSessionId(_state.value.session.id) &&
                        backgroundTurnActive) {
                        reportBackgroundTurnState()
                    }
                }
            }
            launch {
                boundSource.activeSessionState.collect { activated ->
                    if (sourceGeneration == generation) {
                        activated?.let {
                            // A synchronous Attach may immediately emit a terminal
                            // or permission event. Subscribe before exposing or
                            // attaching this owner so the event cannot be lost.
                            eventsSubscribed.await()
                            if (sourceGeneration != generation) return@collect
                            val restoringOwner = _state.value.sessionTransitioning && pendingResumeId != null &&
                                canonicalSessionId(pendingResumeId.orEmpty()) == canonicalSessionId(it.sessionId)
                            val activation = if (restoringOwner) boundSource.retainedUiActivation(it) else it
                            while (sourceGeneration == generation && !applyActivatedSession(activation)) {
                                SessionAdmissionGate.awaitPublication(canonicalSessionId(activation.sessionId))
                            }
                            if (sourceGeneration != generation) return@collect
                            if (sessionOwnerRejected || _state.value.sessionTransitioning ||
                                canonicalSessionId(_state.value.session.id) != canonicalSessionId(activation.sessionId)) return@collect
                            val activationToken = sessionToken
                            try {
                                boundSource.attachDurableTurnForUi(afterSequence = durableTurnUiSequence)
                                if (sourceGeneration == generation && sessionToken == activationToken) {
                                    retainedAttachSubmissionPending = false
                                    openAfterRetainedAttachIfConfirmed()
                                }
                            } catch (cancelled: CancellationException) {
                                throw cancelled
                            } catch (failure: Throwable) {
                                if (sourceGeneration == generation && sessionToken == activationToken) {
                                    preserveDurableRecoveryAfterAttachFailure(failure)
                                }
                            }
                        }
                    }
                }
            }
            launch {
                boundSource.pendingPermission.collect { prompt ->
                    if (sourceGeneration != generation || sessionOwnerRejected) return@collect
                    val previous = _pendingPermission.value
                    _pendingPermission.value = prompt
                    // Upstream arms a 6s delay when the prompt APPEARS and
                    // clears it when the prompt settles, so a request answered
                    // promptly notifies nothing. Only a newly-arrived id arms;
                    // re-emitting the same prompt must not restart the clock.
                    if (prompt != null && prompt.requestId != previous?.requestId) {
                        val sessionId = _state.value.session.id
                        if (sessionId.isNotBlank() && sessionId != "new") {
                            backgroundExecution.notifyPermissionRequested(
                                ConversationBackgroundSnapshot(
                                    sessionId = sessionId,
                                    turnId = durableTurnId,
                                    statusText = null,
                                    recoverySpec = source.recoverySpec,
                                ),
                                prompt.requestId,
                                prompt.toolName
                                    ?: strings.resolve(R.string.permission_request_title, "请求权限"),
                            )
                        }
                    }
                    if (prompt == null && previous != null) {
                        backgroundExecution.settlePermission(previous.requestId)
                    }
                }
            }
            (boundSource as? ExternalTurnConversationSource)?.let { external ->
                // Subscribe before anything can be reported: the feed starts the
                // source following its session when it is collected.
                launch(start = CoroutineStart.UNDISPATCHED) {
                    external.externalFeed.collect { item ->
                        while (sourceGeneration == generation && !ingestExternal(item)) {
                            val id = (item as? ExternalFeedItem.Base)?.session?.sessionId ?: break
                            SessionAdmissionGate.awaitPublication(canonicalSessionId(id))
                        }
                    }
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
                // Subscribe before requesting: an in-process catalog can arrive
                // synchronously, and must belong to this workspace/source.
                runCatching {
                    boundSource.submitClientCommand(
                        ClientCommand.RefreshListings(listOf(ListingKindDto.SLASH_COMMANDS)),
                    )
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
        if (event is ClientEvent.Error && isCompactTurnRejection(event.message.displayText())) {
            showCompactRejection()
            return
        }
        if (source is ExternalTurnConversationSource && event is ClientEvent.SlashCommandResult) {
            // A remote configuration command has no locally owned turn. Its result
            // must also leave an unrelated running desktop turn intact.
            val output = event.display.displayText()
            _state.update {
                it.copy(
                    commandResult = output.takeIf { text -> text.isNotBlank() }?.let { text -> ConversationCommandOutput("/", text, event.isError) },
                )
            }
            return
        }
        planTasksFrom(event)?.let { tasks ->
            _state.update { it.copy(planTasks = tasks) }
            return
        }
        when (event) {
            is ClientEvent.SystemNotice -> {
                // Turn notices use the reply stream; keyless bootstrap errors have no turn.
                if (event.isError && !state.value.streaming) reportHostError(event.message.displayText())
            }
            is ClientEvent.UserTranscriptRowIdentity -> {
                if (!event.rowToken.startsWith("client-turn:")) return
                val rowIds = event.rowToken.removePrefix("client-turn:").split(',')
                    .mapNotNull { id -> id.takeIf { it.isNotEmpty() && it.all(Char::isDigit) }?.let { optimisticRowsByToken.remove("client-turn:$it") } }.toSet()
                if (rowIds.isEmpty()) return
                _state.update { state -> state.copy(messages = state.messages.map { message ->
                    if (message.id in rowIds) message.copy(transcriptUuid = event.uuid) else message
                }) }
            }
            is ClientEvent.AssistantBlockIdentity -> {
                pendingAssistantTranscriptUuids.add(event.messageUuid)
                _state.update { state -> state.copy(streamingMessage = state.streamingMessage?.let {
                    it.copy(transcriptUuid = it.transcriptUuid ?: event.messageUuid, transcriptAliases = it.transcriptAliases + event.messageUuid)
                }) }
            }
            is ClientEvent.AssistantTranscriptRowUuids -> {
                val uuids = event.uuids.filterNotNull()
                assistantTranscriptUuidsByIdentity[event.messageId] = uuids
                val rowId = assistantRowsByIdentity[event.messageId]
                if (rowId != null) _state.update { state -> state.copy(messages = state.messages.map { message ->
                    if (message.id == rowId) message.copy(transcriptUuid = uuids.firstOrNull() ?: message.transcriptUuid, transcriptAliases = message.transcriptAliases + uuids) else message
                }) }
            }
            is ClientEvent.SlashCommandCatalog -> _state.update { state ->
                state.copy(slashCommands = event.commands.map { command ->
                    ConversationSlashCommand(
                        name = command.name.displayText(),
                        aliases = command.aliases.map { it.displayText() },
                        argumentHint = command.argumentHint?.displayText(),
                        argumentNames = command.argumentNames.map { it.displayText() },
                        description = command.menuDescription ?: command.description.displayText(),
                        source = command.source,
                        hidden = command.hidden,
                    )
                })
            }
            is ClientEvent.TurnStarted -> {
                pendingSlash?.takeIf { event.turnId == null || event.turnId.toLong() == it.turnId }?.let { pending ->
                    if (!pending.echoed) {
                        _state.update { state -> state.copy(messages = state.messages + pending.prompt) }
                    }
                    pendingSlash = null // The real model turn now owns the lifecycle.
                }
                nativeTurnAdmission.confirm(event.turnId?.toLong())
                pendingAssistantTranscriptUuids.clear()
                _state.update { it.copy(promptSuggestion = null, promptSuggestionUuid = null, suggestionTurn = it.suggestionTurn + 1) }
                if (source is ExternalTurnConversationSource) beginExternalTurn()
            }
            is ClientEvent.PromptSuggestionState -> _state.update {
                it.copy(promptSuggestionEnabled = event.enabled, promptSuggestionNotice = event.notice)
            }
            is ClientEvent.PromptSuggestion -> _state.update { state ->
                if (state.session.id == event.sessionId
                    && !state.sessionTransitioning && state.pendingQuestions.isEmpty())
                    state.copy(promptSuggestion = event.suggestion, promptSuggestionUuid = event.uuid) else state
            }
            is ClientEvent.ScheduledTaskFire -> _state.update { current ->
                // Session notice arrives before TurnStarted and is not a loop-fold marker.
                current.copy(messages = current.messages + Message(role = Role.Ai, text = event.message))
            }
            is ClientEvent.LoopWakeup -> _state.update { current ->
                val rows = buildChatRenderItems(current)
                val lastWakeup = current.messages.lastOrNull { it.loopWakeupStreak != null }
                val start = rows.indexOfFirst { it.key == lastWakeup?.id }
                val folded = if (event.streak > 0u && start >= 0)
                    rows.drop(start).map { it.key }.filterNot { it in lastWakeup?.loopPreexistingItemIds.orEmpty() }.toSet()
                else emptySet()
                current.copy(messages = current.messages + Message(
                    role = Role.Ai, text = event.message,
                    loopWakeupStreak = event.streak, loopFoldedItemIds = folded,
                    loopPreexistingItemIds = rows.map { it.key }.toSet(),
                ) + listOfNotNull(event.companion?.let { Message(role = Role.Ai, text = it) }))
            }
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
                it.copy(pendingQuestions = emptyList(), compaction = null, promptSuggestion = null, promptSuggestionUuid = null)
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
            is ClientEvent.Error -> {
                val message = event.message.displayText()
                _state.update { state ->
                    val previous = state.compaction
                    if (previous?.status != CompactionProgressStatus.Running ||
                        !message.startsWith("force_compact failed:", ignoreCase = true)) {
                        state
                    } else {
                        state.copy(
                            compaction = previous.copy(
                                status = CompactionProgressStatus.Failed,
                                detail = compactFailureDetail(message),
                            ),
                        )
                    }
                }
            }
            is ClientEvent.TaskRow -> {
                _state.update {
                    val task = event.task.toBackgroundTaskUi()
                    it.copy(
                        backgroundTasks = it.backgroundTasks + (task.taskId to task),
                        activeBackgroundTaskIds = it.activeBackgroundTaskIds.withTaskStatus(task.taskId, task.status),
                    )
                }
                if (backgroundTurnActive) reportBackgroundTurnState()
            }
            is ClientEvent.WorkflowResumed -> {
                _state.update { state ->
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
                if (backgroundTurnActive) reportBackgroundTurnState()
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
                    state.copy(sessionAgents = if (state.sessionAgents.any { it.agentId == agent.agentId }) state.sessionAgents.map { if (it.agentId == agent.agentId) agent else it } else state.sessionAgents + agent)
                }
            }
            is ClientEvent.TaskStatusChanged -> {
                // `TaskStatusDto` is a UniFFI-generated Kotlin ENUM, not a
                // sealed class — `is` would not compile.
                val terminal = event.status == TaskStatusDto.COMPLETED || event.status == TaskStatusDto.FAILED
                if (terminal) {
                    val sessionId = _state.value.session.id
                    if (sessionId.isNotBlank() && sessionId != "new") {
                        backgroundExecution.notifyTaskFinished(
                            ConversationBackgroundSnapshot(
                                sessionId = sessionId,
                                turnId = durableTurnId,
                                statusText = null,
                                recoverySpec = source.recoverySpec,
                            ),
                            taskId = event.taskId,
                            label = _state.value.backgroundTasks[event.taskId]?.description,
                            failed = event.status == TaskStatusDto.FAILED,
                        )
                    }
                }
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
                if (backgroundTurnActive) reportBackgroundTurnState()
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
                val terminal = snapshot.state in setOf(
                    TurnRecoveryStateDto.COMPLETED,
                    TurnRecoveryStateDto.FAILED,
                    TurnRecoveryStateDto.CANCELLED,
                )
                if (terminal && terminalReplaySettledTurnId == recoveredId) {
                    terminalReplaySettledTurnId = null
                    return
                }
                val firstRetainedAttach = awaitingRetainedAttachSnapshot && durableTurnId == recoveredId
                if (firstRetainedAttach) {
                    awaitingRetainedAttachSnapshot = false
                    retainedAttachSnapshotSeen = true
                    if (snapshot.reason == "replay_history_truncated") {
                        val message = strings.resolve(R.string.chat_recovery_replay_truncated,
                            "部分回复暂未显示。任务结束后，重新打开对话可查看完整记录。")
                        _state.update { it.copy(error = ChatError(message, ChatErrorKind.GENERIC)) }
                        if (terminal) {
                            finishRetainedTerminal(snapshot, message)
                            openAfterRetainedAttachIfConfirmed()
                            return
                        }
                    }
                    if (terminal && snapshot.lastSequence.toLong() <= durableTurnUiSequence) {
                        finishRetainedTerminal(snapshot, snapshot.reason ?: strings.resolve(
                            R.string.chat_background_failed_text, "点按返回对话查看并重试"))
                        openAfterRetainedAttachIfConfirmed()
                        return
                    }
                    openAfterRetainedAttachIfConfirmed()
                }
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
                    recoveredProjectionFrontier.reset(0L)
                    pendingReplayRaw.clear()
                    currentTurnOrigin = ConversationTurnOrigin.Ordinary
                } else if (
                    !firstRetainedAttach && recoveredTurnToken != null &&
                    recoveryReplayTurnId == recoveredId &&
                    (replayThroughSequence == null || durableTurnUiSequence >= replayThroughSequence!!)
                ) {
                    // AttachTurn has returned and ResumeTurn has now emitted its
                    // own state. Subsequent sequenced envelopes are live copies;
                    // the following raw event is what the reducer should render.
                    recoveryReplayTurnId = null
                }
                durableTurnId = recoveredId
                if (startsRecoveredAttach || firstRetainedAttach) {
                    if (startsRecoveredAttach) replayBuffer.clear()
                    replayTerminalState = null
                    replayThroughSequence = maxOf(snapshot.lastSequence.toLong(), if (replayBuffer.isEmpty()) 0L else replayBuffer.lastKey())
                    if (snapshot.reason == "replay_history_truncated" || durableTurnUiSequence >= replayThroughSequence!!) {
                        replayBuffer.clear()
                        replayThroughSequence = null
                        recoveryReplayTurnId = null
                        recoveredTurnToken?.let(::flushReplayRaw)
                    }
                } else if (terminal && recoveryReplayTurnId == recoveredId &&
                    durableDiscardInFlightTurnId != recoveredId &&
                    replayThroughSequence?.let { durableTurnUiSequence < it } == true
                ) {
                    replayTerminalState = event
                    return
                }

                val hasAuthoritativeRestoredTranscript =
                    restoredTranscriptSessionId == canonicalSessionId(snapshot.sessionId)
                val isInactiveRecoveredCheckpoint = recoveredTurnToken != null &&
                    turnJob == null &&
                    !_state.value.streaming
                // On the first attach snapshot, terminal history is replayed
                // immediately afterwards. Defer settling until ResumeTurn emits
                // the authoritative terminal state a second time.
                if ((startsRecoveredAttach || firstRetainedAttach) && terminal) {
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
                        recoveredProjectionFrontier.reset(durableTurnUiSequence)
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
                if (startsRecoveredAttach) captureTurnProjection(recoveredId, durableTurnUiSequence)
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
                    if (event.sequence.toLong() <= durableTurnUiSequence) return
                    replayBuffer[event.sequence.toLong()] = event
                    while (true) {
                        val next = replayBuffer.remove(durableTurnUiSequence + 1L) ?: break
                        val reply = retainedTurnEventToReply(next.eventJson, strings)
                        if (reply != null) {
                            val duplicate = pendingReplayRaw.indexOfFirst { (_, raw) -> stableReplayReply(raw) == stableReplayReply(reply) }
                            if (duplicate >= 0) pendingReplayRaw.removeAt(duplicate)
                            reduce(reply, token)
                        }
                        durableTurnUiSequence = next.sequence.toLong()
                        if (reply is ReplyEvent.End || reply is ReplyEvent.Error || reply is ReplyEvent.Completed || reply is ReplyEvent.CommandOutput) {
                            captureTurnProjection(next.turnId.toLong(), durableTurnUiSequence)
                            val turnId = next.turnId.toLong()
                            terminalReplaySettledTurnId = turnId
                            clearDurableRecoveryAfterTerminal(turnId, durableTurnId == turnId || inactiveWaitingTurnId == turnId || liveWaitingTurnId == turnId)
                            recoveredTurnToken = null
                            recoveryReplayTurnId = null
                            recoveredProjectionFrontier.reset(durableTurnUiSequence)
                            pendingReplayRaw.clear()
                            replayThroughSequence = null
                            replayTerminalState = null
                            replayBuffer.clear()
                            break
                        }
                        if (inactiveWaitingTurnId == next.turnId.toLong()) {
                            _state.update { state ->
                                settleInactiveRecoveredTurn(state, token).copy(
                                    streaming = false,
                                    durableRecoveryBlocked = true,
                                    liveTurnWaitingForUser = false,
                                )
                            }
                        }
                        captureTurnProjection(next.turnId.toLong(), durableTurnUiSequence)
                        recoveredProjectionFrontier.reset(durableTurnUiSequence)
                    }
                    if (replayThroughSequence?.let { durableTurnUiSequence >= it } == true) {
                        recoveryReplayTurnId = null
                        replayThroughSequence = null
                        recoveredTurnToken?.let(::flushReplayRaw)
                        replayTerminalState?.let { terminalState ->
                            replayTerminalState = null
                            reduceClientEvent(terminalState)
                        }
                    }
                } else if (recoveredTurnToken != null && durableTurnId == event.turnId.toLong() &&
                    canonicalSessionId(event.sessionId) == canonicalSessionId(_state.value.session.id)) {
                    if (event.sequence.toLong() <= durableTurnUiSequence) return
                    if (!recoveredProjectionFrontier.hasMatchingRendered(event)) {
                        retainedTurnEventToReply(event.eventJson, strings)?.let { reply ->
                            recoveredProjectionFrontier.renderedEnvelope(event, reply)
                            recoveredTurnToken?.let { reduce(reply, it) }
                        }
                    }
                    recoveredProjectionFrontier.acknowledge(event)?.let { sequence ->
                        durableTurnUiSequence = sequence
                        captureTurnProjection(event.turnId.toLong(), sequence)
                    }
                }
            }
            else -> Unit
        }

        // A reattached turn has no `source.submit(...).collect` coroutine. Its
        // live raw events therefore arrive only on the shared client event path.
        // Sequenced envelopes update the durable cursor; render each following
        // raw event exactly once through the ordinary reducer.
        val recoveredToken = recoveredTurnToken
        if (recoveredToken != null && event !is ClientEvent.TurnRecoveryState && event !is ClientEvent.TurnEventReplay) {
            clientEventToReply(event, strings)?.let { reply ->
                if (recoveryReplayTurnId != null) {
                    pendingReplayRaw.add(event to reply)
                    return@let
                }
                recoveredProjectionFrontier.rendered(event, reply)
                reduce(reply, recoveredToken)
                if (reply is ReplyEvent.End || reply is ReplyEvent.Error || reply is ReplyEvent.CommandOutput) {
                    recoveredTurnToken = null
                    recoveryReplayTurnId = null
                    recoveredProjectionFrontier.reset(durableTurnUiSequence)
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

    /** Rows of the followed session's user prompts, by the turn the desktop assigned. */
    private val externalPromptRows = mutableMapOf<Long, String>()

    /** Provisional rows of prompts this device sent, by outgoing prompt id. */
    private val externalOutgoingRows = mutableMapOf<String, String>()
    private val externalFailedPrompts = mutableSetOf<String>()
    private val externalRetryPrompts = mutableMapOf<String, ExternalOutgoingPrompt>()
    private val _externalSendFailures = MutableStateFlow<List<ExternalSendFailure>>(emptyList())
    private val _externalSendInFlight = MutableStateFlow(0)

    /** Failed drafts to restore in the composer, retaining attachments for an idempotent retry. */
    internal val externalSendFailures = _externalSendFailures.asStateFlow()
    internal val externalSendInFlight = _externalSendInFlight.asStateFlow()

    internal fun acknowledgeExternalSendFailure(failure: ExternalSendFailure) {
        _externalSendFailures.update { queue ->
            if (queue.firstOrNull() == failure) queue.drop(1) else queue
        }
    }

    /**
     * Show [session] as loading until the source reports its base. Called once,
     * right after the view model of a followed session is created.
     */
    internal fun beginExternalLoading(session: SessionRef) {
        _state.update {
            it.copy(
                session = session,
                isNew = false,
                sessionReady = false,
                statusLine = strings.resolve(R.string.remote_session_loading, "正在加载会话…"),
            )
        }
    }

    /**
     * Fold one change of a followed session into [ChatState]. The turn the phone
     * did not start has no collector of its own, so its events take the same path
     * a re-attached turn does (the tail of [reduceClientEvent]).
     */
    internal fun ingestExternal(item: ExternalFeedItem): Boolean {
        if (sessionOwnerRejected) return true
        when (item) {
            is ExternalFeedItem.Base -> {
                if (!applyActivatedSession(item.session)) return false
                if (sessionOwnerRejected) return true
                externalPromptRows.clear()
                externalOutgoingRows.clear()
                item.title?.let { title ->
                    _state.update { it.copy(session = it.session.copy(title = title)) }
                }
            }
            is ExternalFeedItem.OlderMessages ->
                _state.update { it.copy(messages = item.messages + it.messages) }
            is ExternalFeedItem.UserPrompt -> {
                val row = Message(role = Role.User, text = item.text, originLabel = item.originLabel)
                externalPromptRows[item.turnId] = row.id
                _state.update { it.copy(isNew = false, messages = it.messages + row) }
            }
            is ExternalFeedItem.PromptImages -> externalPromptRows[item.turnId]?.let { rowId ->
                _state.update { state ->
                    state.copy(messages = state.messages.map { if (it.id == rowId) it.copy(images = item.images) else it })
                }
            }
            is ExternalFeedItem.Event -> reduceClientEvent(item.event)
            is ExternalFeedItem.Outgoing -> reduceExternalOutgoing(item.prompts)
            is ExternalFeedItem.PendingQuestions -> _state.update { it.copy(pendingQuestions = item.questions) }
            is ExternalFeedItem.Turn -> reconcileExternalTurn(item.running)
            is ExternalFeedItem.Readiness -> _state.update {
                it.copy(sessionReady = item.ready, statusLine = item.statusLine)
            }
            is ExternalFeedItem.Notice -> _state.update { it.copy(statusLine = item.text) }
        }
        return true
    }

    /** A turn of the followed session started; idle means nothing else owns the reducer. */
    private fun beginExternalTurn() {
        if (turnJob != null || recoveredTurnToken != null) return
        turnToken++
        recoveredTurnToken = turnToken
        recoveryReplayTurnId = null
        replayThroughSequence = null
        replayBuffer.clear()
        replayTerminalState = null
        durableTurnUiSequence = 0L
        recoveredProjectionFrontier.reset(0L)
        pendingReplayRaw.clear()
        currentTurnOrigin = ConversationTurnOrigin.Ordinary
        _state.update {
            it.copy(
                streaming = true,
                error = null,
                statusLine = null,
                agentRun = AgentRunState(turnId = turnToken),
            )
        }
    }

    /**
     * The source says whether a turn runs. Events normally settle it first; this
     * covers a turn whose start or end the events did not carry (a snapshot taken
     * mid-turn, an end the history page cut off).
     */
    private fun reconcileExternalTurn(running: Boolean) {
        val token = recoveredTurnToken
        if (running && token == null) {
            beginExternalTurn()
            recoveredTurnToken?.let { reduce(ReplyEvent.Thinking, it) }
        } else if (!running && token != null) {
            reduce(ReplyEvent.End, token)
            recoveredTurnToken = null
        }
    }

    private fun reduceExternalOutgoing(prompts: List<ExternalOutgoingPrompt>) {
        val live = prompts.mapTo(mutableSetOf()) { it.id }
        val gone = externalOutgoingRows.keys.filter { it !in live }
        gone.forEach { id ->
            val rowId = externalOutgoingRows.remove(id)
            _state.update { state -> state.copy(messages = state.messages.filterNot { it.id == rowId }) }
        }
        externalFailedPrompts.retainAll(live)
        externalRetryPrompts.keys.retainAll(live)
        for (prompt in prompts) {
            if (prompt.failed) {
                if (!externalFailedPrompts.add(prompt.id)) continue
                val rowId = externalOutgoingRows.remove(prompt.id)
                _state.update { state -> state.copy(messages = state.messages.filterNot { it.id == rowId }) }
                externalRetryPrompts[prompt.id] = prompt
                _externalSendFailures.update { it + ExternalSendFailure(prompt.text, prompt.images) }
                reportHostError(strings.resolve(R.string.remote_send_failed, "消息未能发送，请重试。"))
            } else {
                externalFailedPrompts.remove(prompt.id)
                externalRetryPrompts.remove(prompt.id)
                if (prompt.id !in externalOutgoingRows) {
                    val row = Message(role = Role.User, text = prompt.text, images = prompt.images)
                    externalOutgoingRows[prompt.id] = row.id
                    _state.update { it.copy(isNew = false, messages = it.messages + row) }
                }
            }
        }
    }

    /** Send from the composer of a followed session: the row appears when the desktop reports it. */
    private fun sendExternal(text: String, draftText: String, images: List<ImageRefDto>, external: ExternalTurnConversationSource) {
        if (!_state.value.sessionReady) {
            // The owner can leave Live after composer admission but before this call.
            _externalSendFailures.update { it + ExternalSendFailure(draftText, images) }
            return
        }
        savedState?.set(KEY_DRAFT, "")
        _state.update { it.copy(isNew = false, error = null) }
        val retry = externalRetryPrompts.values.firstOrNull { it.text == text && it.images == images }
        _externalSendInFlight.update { it + 1 }
        viewModelScope.launch(start = CoroutineStart.UNDISPATCHED) {
            try {
                if (retry != null) external.retryExternalPrompt(retry.id)
                else external.sendExternalPrompt(text, images)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Throwable) {
                _externalSendFailures.update { it + ExternalSendFailure(draftText, images) }
                reportHostError(error.message ?: error::class.simpleName.orEmpty())
            } finally {
                _externalSendInFlight.update { it - 1 }
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
        recoveredProjectionFrontier.reset(durableTurnUiSequence)
        pendingReplayRaw.clear()
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
    fun answerQuestion(requestId: ULong, answers: AskUserQuestionAnswersDto) {
        if (sessionOwnerRejected) return
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
        if (sessionOwnerRejected) return
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
    internal fun ensureSourceInBackground(
        generation: Int,
        dispatcher: CoroutineDispatcher = Dispatchers.IO,
        awaitRelease: suspend (String) -> Boolean = { true },
        awaitMediaDrain: suspend () -> Boolean = { true },
        canReplace: () -> Boolean = { true },
        beginReplacement: suspend () -> AutoCloseable? = { AutoCloseable { } },
        createSource: () -> ConversationSource,
    ) {
        if (generation <= reconnectGeneration || generation <= (requestedSourceGeneration ?: Int.MIN_VALUE)) return
        if (!canReplace()) return
        if (source !is ConnectingConversationSource && _state.value.blocksEngineReconnect(_pendingPermission.value != null)) return
        if (sideChats.hasLiveChats) {
            reportHostError(strings.resolve(R.string.side_chat_end_before_reconnect, "请先结束旁聊，再重新连接引擎。"))
            sideChats.whenVacant = { ensureSourceInBackground(generation, dispatcher, awaitRelease, awaitMediaDrain, canReplace, beginReplacement, createSource) }
            return
        }
        requestedSourceGeneration = generation
        val expectedSource = source
        viewModelScope.launch {
            workspaceSwitchMutex.withLock {
                var prepared: ConversationSource? = null
                var replacementLease: AutoCloseable? = null
                try {
                    if (requestedSourceGeneration != generation || generation <= reconnectGeneration || source !== expectedSource) return@launch
                    if (!canReplace()) return@launch
                    if (source !is ConnectingConversationSource && _state.value.blocksEngineReconnect(_pendingPermission.value != null)) return@launch
                    if (sideChats.hasLiveChats) {
                        sideChats.whenVacant = { ensureSourceInBackground(generation, dispatcher, awaitRelease, awaitMediaDrain, canReplace, beginReplacement, createSource) }
                        return@launch
                    }
                    // A Cron reservation can be in flight for this same UUID.
                    // Keep the old source alive while waiting and recheck work
                    // before replacing it.
                    replacementLease = beginReplacement() ?: return@launch
                    if (requestedSourceGeneration != generation || generation <= reconnectGeneration || source !== expectedSource || !canReplace()) return@launch
                    if (source !is ConnectingConversationSource && _state.value.blocksEngineReconnect(_pendingPermission.value != null)) return@launch
                    if (sideChats.hasLiveChats) {
                        sideChats.whenVacant = { ensureSourceInBackground(generation, dispatcher, awaitRelease, awaitMediaDrain, canReplace, beginReplacement, createSource) }
                        return@launch
                    }
                    if (source !is ConnectingConversationSource) {
                        val previous = source
                        val sessionId = canonicalSessionId(_state.value.session.id).takeUnless { it.isBlank() || it == "new" }
                        val connecting = ConnectingConversationSource(strings)
                        sourceBindingJob?.cancel()
                        source = connecting
                        _engineSource.value = connecting
                        _state.update { it.copy(sessionReady = false, sessionTransitioning = true,
                            statusLine = strings.resolve(R.string.chat_status_reconnecting, "正在重新连接…")) }
                        pendingReconnectRelease = sessionId
                        previous.close()
                    }
                    val releaseId = pendingReconnectRelease
                    if (releaseId != null) {
                        val released = awaitRelease(releaseId)
                        if (!released) {
                            if (requestedSourceGeneration == generation) _state.update {
                                it.copy(sessionTransitioning = false, sessionReady = false, statusLine = null,
                                    error = ChatError(strings.resolve(R.string.session_reconnect_still_releasing,
                                        "上一个会话仍在关闭，请稍后重试。"), ChatErrorKind.GENERIC))
                            }
                            return@launch
                        }
                        pendingReconnectRelease = null
                    }
                    // Session ownership and the host media attachment have
                    // independent async drains. Do not construct a replacement
                    // while the old attachment remains unconfirmed, including
                    // reconnects whose old source had no session ID yet.
                    if (!awaitMediaDrain()) {
                        if (requestedSourceGeneration == generation) _state.update {
                            it.copy(sessionTransitioning = false, sessionReady = false, statusLine = null,
                                error = ChatError(strings.resolve(R.string.chat_engine_unavailable,
                                    "移动端引擎当前不可用"), ChatErrorKind.GENERIC))
                        }
                        return@launch
                    }
                    if (requestedSourceGeneration != generation) return@launch
                    // UniFFI construction is synchronous and cannot be interrupted.
                    // Retain the returned handle even after cancellation so it can
                    // be closed rather than lost at withContext's dispatch boundary.
                    withContext(dispatcher + NonCancellable) { prepared = createSource() }
                    coroutineContext.ensureActive()
                    if (requestedSourceGeneration != generation || generation <= reconnectGeneration || source !is ConnectingConversationSource) return@launch
                    if (source !is ConnectingConversationSource && _state.value.blocksEngineReconnect(_pendingPermission.value != null)) return@launch
                    if (sideChats.hasLiveChats) {
                        sideChats.whenVacant = { ensureSourceInBackground(generation, dispatcher, awaitRelease, awaitMediaDrain, canReplace, beginReplacement, createSource) }
                        return@launch
                    }
                    val replacement = checkNotNull(prepared)
                    ensureSource(generation) { replacement }
                    prepared = null
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (failure: Exception) {
                    if (requestedSourceGeneration == generation && source is ConnectingConversationSource) ensureSource(generation) {
                        UnavailableConversationSource(failure.message ?: "Engine initialization failed", strings)
                    }
                } finally {
                    val abandoned = prepared
                    try {
                        if (abandoned != null && abandoned !== source) withContext(dispatcher + NonCancellable) { abandoned.close() }
                    } finally {
                        replacementLease?.close()
                        if (requestedSourceGeneration == generation) requestedSourceGeneration = null
                    }
                }
            }
        }
    }

    /** Fix the startup destination before a retained or newly built source can emit. */
    internal fun prepareInitialSessionRestore(target: SessionRef?, scope: ConversationScope): SessionRef? {
        if (source !is ConnectingConversationSource || initialRestorePrepared) {
            return _state.value.session.takeUnless { it.id == "new" }
        }
        initialRestorePrepared = true
        sessionToken++
        sessionTransitionJob?.cancel()
        sessionTransitionJob = null
        val chosen = restoredNavigation?.takeIf { it.first == scope }?.second ?: target
        val ref = chosen?.copy(id = canonicalSessionId(chosen.id))
            ?: SessionRef("new", strings.resolve(R.string.chat_new_conversation, "新对话"))
        pendingResumeId = ref.id.takeUnless { it == "new" }
        // Do not mistake the source's startup announcement for NewSession.
        pendingNewSession = ref.id == "new"
        _sourceScope.value = scope
        _sourceProjectId.value = (scope as? ConversationScope.Project)?.projectId
        setNavigationOwner(ref.id, scope)
        _state.update {
            it.copy(session = ref, messages = emptyList(), isNew = ref.id == "new",
                sessionTransitioning = true, sessionReady = false, error = null,
                statusLine = strings.resolve(R.string.chat_status_reconnecting, "正在重新连接…"))
        }
        return ref.takeUnless { it.id == "new" }
    }

    internal fun ensureSource(
        generation: Int,
        createSource: () -> ConversationSource,
    ) {
        if (generation <= reconnectGeneration) return
        if (source !is ConnectingConversationSource && _state.value.blocksEngineReconnect(_pendingPermission.value != null)) return
        if (sideChats.hasLiveChats) {
            reportHostError(strings.resolve(R.string.side_chat_end_before_reconnect, "请先结束旁聊，再重新连接引擎。"))
            sideChats.whenVacant = { ensureSource(generation, createSource) }
            return
        }

        val replacement = createSource()
        if (replacement is UnavailableConversationSource) {
            replacement.close()
            _state.update {
                it.copy(
                    statusLine = null,
                    sessionTransitioning = false,
                    sessionReady = if (source is ConnectingConversationSource) false else it.sessionReady,
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
        // Closing the source we were handed back would leave the chat on a
        // destroyed engine; there is nothing to replace.
        if (replacement === source) {
            reconnectGeneration = generation
            return
        }
        val previous = source
        reconnectGeneration = generation
        sourceGeneration++

        abandonLocalTurn()?.cancel()
        explicitCancellation?.cancel()
        explicitCancellation = null
        _state.update { it.copy(cancellationInFlight = false) }
        sessionTransitionJob?.cancel()
        sessionTransitionJob = null
        sourceBindingJob?.cancel()

        sideChats.release()
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
                sessionMode = null,
            )
        }
        // The replacement engine was built for this exact UUID and emits its
        // activation during construction. Never submit New/Resume a second time.
        val target = _state.value.session
        pendingResumeId = target.id.takeUnless { it == "new" }
        pendingNewSession = target.id == "new"
        _state.update { it.copy(sessionTransitioning = true, sessionReady = false) }
        bindSource()
        previous.close()
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
        newSessionDirectory: String? = null,
        resumeEmpty: Boolean = false,
        replacePendingTransition: Boolean = false,
        allowInactiveWaitingRecovery: Boolean = false,
        /**
         * The scope the replacement source is bound to. Defaults to the
         * project/global split [projectId] already implies; callers binding
         * the Scheduled scope pass it explicitly.
         */
        scope: ConversationScope = projectId?.let { ConversationScope.Project(it) }
            ?: ConversationScope.Global,
        createSource: suspend () -> ConversationSource,
        persistSelection: suspend () -> Unit = {},
        rollbackSelection: suspend () -> Unit = {},
        onCommitted: () -> Unit = {},
    ): Boolean = workspaceSwitchMutex.withLock {
        if (sideChats.hasLiveChats) {
            reportHostError(strings.resolve(R.string.side_chat_end_before_workspace, "请先结束旁聊，再切换工作区。"))
            return@withLock false
        }
        if (!allowInactiveWaitingRecovery && refuseWhileDurableTurnParked()) return@withLock false
        val previous = source
        fun replacementIsBlocked(): Boolean = sideChats.hasLiveChats || source !== previous ||
            _state.value.blocksWorkspaceReplacement(
                hasPendingPermission = _pendingPermission.value != null,
                replacePendingTransition = replacePendingTransition,
            )
        fun reportBlockedReplacement() {
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
        }
        if (replacementIsBlocked()) {
            reportBlockedReplacement()
            return@withLock false
        }
        val replacement = runCatching { createSource() }.getOrElse { error ->
            if (error is CancellationException) throw error
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

        // Construction can publish callbacks before returning. A task/agent
        // that became live during that work still belongs to the old source.
        if (replacementIsBlocked()) {
            if (replacement !== source) replacement.close()
            reportBlockedReplacement()
            return@withLock false
        }

        suspend fun rollbackPersistedSelection(): Throwable? = withContext(NonCancellable) {
            runCatching { rollbackSelection() }.exceptionOrNull()
        }
        val persisted = runCatching { persistSelection() }
        if (persisted.isFailure) {
            val rollbackError = rollbackPersistedSelection()
            if (replacement !== source) replacement.close()
            val error = persisted.exceptionOrNull()
            if (error is CancellationException) throw error
            _state.update {
                it.copy(
                    error = ChatError(
                        message = strings.resolve(
                            R.string.chat_error_persist_active_project_failed,
                            "无法保存活动项目：%1\$s",
                            listOfNotNull(error?.message ?: error?.let { it::class.simpleName }, rollbackError?.message)
                                .joinToString("; "),
                        ),
                        kind = classifyError(error?.message.orEmpty()),
                    ),
                )
            }
            return@withLock false
        }

        // Persistence yields to UI/event collectors. Recheck immediately
        // before releasing callbacks/runtime ownership, and undo only this
        // operation's durable selection if live work arrived while suspended.
        if (replacementIsBlocked()) {
            val rollbackError = rollbackPersistedSelection()
            if (replacement !== source) replacement.close()
            reportBlockedReplacement()
            rollbackError?.let { reportHostError(it.message.orEmpty()) }
            return@withLock false
        }

        if (allowInactiveWaitingRecovery) {
            releaseDurableRecoveryForSessionChange()
        }

        if (replacePendingTransition && _state.value.sessionTransitioning) {
            abandonPendingSessionTransition()
        }

        sourceGeneration++
        sourceBindingJob?.cancel()
        sideChats.release()
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
                sessionMode = null,
            )
        }
        bindSource()
        onCommitted()
        if (previous !== replacement) previous.close()
        beginSessionTransition(
            target = target,
            newSession = newSession,
            newSessionDirectory = newSessionDirectory,
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
        check(!sessionOwnerRejected) { "Conversation owner was rejected" }
        source.approvePermission(requestId, response)
    }

    suspend fun denyPermission(requestId: ULong) {
        check(!sessionOwnerRejected) { "Conversation owner was rejected" }
        source.denyPermission(requestId)
    }

    fun refreshMcpServers() {
        viewModelScope.launch { source.refreshMcpServers() }
    }

    /** Write only the small navigation slice into [savedState]. */
    private fun persist(s: ChatState) {
        val sv = savedState ?: return
        if (source is ConnectingConversationSource && !initialRestorePrepared) return
        if (canonicalSessionId(s.session.id) != canonicalSessionId(navigationOwnerSessionId)) return
        sv[KEY_SESSION_ID] = s.session.id
        sv[KEY_SESSION_TITLE] = s.session.title
        sv[KEY_IS_NEW] = s.isNew
        sv[KEY_SESSION_SCOPE] = navigationOwnerScope.persistenceKey()
    }

    /**
     * Fold the engine's [EngineModelState] into [ChatState]: build the picker
     * rows from the real wire ids and select the active one. An empty catalog
     * leaves the pending selection untouched. Extracted for reducer tests.
     */
    internal fun applyModelState(engine: EngineModelState) {
        _state.update { it.copy(reasoning = engine.reasoning) }
        if (!engine.hasCatalog) return
        val options = EngineModelCatalog.options(engine.available, engine.details)
        val activeRoute = options.firstOrNull { it.id == engine.active } ?: options.first()
        val active = engine.activeSourceDisplay?.let { activeRoute.copy(name = it) } ?: activeRoute
        _state.update { it.copy(availableModels = options, model = active) }
    }

    /**
     * Apply a live session activation the engine confirmed (`SessionStarted` or
     * `SessionResumed`): adopt the real session id and, for resumed sessions,
     * replace the transcript with the rehydrated oldest-first scrollback so the
     * next turn continues from real persisted history. Session switches abandon
     * the prior turn so its late events cannot mutate the replacement transcript;
     * the initial bootstrap announcement only binds the current turn's owner.
     *
     * The title is preserved from the drawer's optimistic local select when the
     * id matches (`resumeSession` set it before submitting `ResumeSession`), else
     * resolved from the session catalog, else the current title — so the title
     * bar never reverts to a placeholder on a real resume. Extracted (internal)
     * so the rehydration is exercised directly in unit tests with a fake source.
     */
    /** False only while this UUID is in a transient disk publication. */
    internal fun applyActivatedSession(restored: ActivatedSession): Boolean {
        if (sessionOwnerRejected) return true
        val current = _state.value
        val retained = restored.retainedProjection
        if (retained != null &&
            (canonicalSessionId(retained.sessionId) != canonicalSessionId(restored.sessionId) ||
                canonicalSessionId(retained.state.session.id) != canonicalSessionId(restored.sessionId) ||
                retained.state.sessionMode != restored.mode)) {
            rejectSessionOwner()
            return true
        }
        if (!current.sessionTransitioning && current.sessionMode != null &&
            canonicalSessionId(current.session.id) == canonicalSessionId(restored.sessionId) &&
            current.sessionMode != restored.mode) {
            rejectSessionOwner()
            return true
        }
        if (current.sessionTransitioning) {
            val matchesPendingResume =
                pendingResumeId != null &&
                    (restored.kind == SessionActivationKind.Resumed || retained != null) &&
                    canonicalSessionId(restored.sessionId) == canonicalSessionId(pendingResumeId.orEmpty())
            val matchesPendingNew =
                pendingNewSession && restored.kind == SessionActivationKind.Started
            if (!matchesPendingResume && !matchesPendingNew) return true
        }
        if (!current.sessionTransitioning && current.session.id != "new" &&
            (restored.kind == SessionActivationKind.Started ||
                canonicalSessionId(current.session.id) != canonicalSessionId(restored.sessionId))) return true
        val ownerId = canonicalSessionId(restored.sessionId)
        if (ownerId.isBlank() || ownerId == "new") {
            rejectSessionOwner()
            return true
        }
        when (sessionOwnerAdmission?.invoke(_sourceScope.value, ownerId, restored.mode) ?: SessionOwnerAdmission.Accepted) {
            SessionOwnerAdmission.Deferred -> return false
            SessionOwnerAdmission.Conflict -> {
                rejectSessionOwner()
                return true
            }
            SessionOwnerAdmission.Accepted -> Unit
        }
        if (restored.kind == SessionActivationKind.Started && !current.sessionTransitioning) {
            // The first prompt can already be streaming when engine bootstrap
            // announces its owner. Bind that identity without abandoning the
            // collector or clearing the optimistic prompt and widget slots.
            if (current.session.id == "new") {
                setNavigationOwner(restored.sessionId, _sourceScope.value)
                _state.update {
                    val catalogTitle = _sessions.value.rows.firstOrNull { row ->
                        canonicalSessionId(row.uuid) == canonicalSessionId(restored.sessionId)
                    }?.title
                    it.copy(session = it.session.copy(id = restored.sessionId, title = catalogTitle ?: it.session.title),
                        sessionReady = true, sessionMode = restored.mode)
                }
                reportBackgroundTurnState()
                return true
            }
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
            .takeIf { restored.kind == SessionActivationKind.Resumed && restored.transcriptComplete }
            ?.let(::canonicalSessionId)
        val title = _sessions.value.rows.firstOrNull { canonicalSessionId(it.uuid) == canonicalSessionId(restored.sessionId) }?.title
            ?: _state.value.session.takeIf { canonicalSessionId(it.id) == canonicalSessionId(restored.sessionId) }?.title
            ?: _state.value.session.title
        restored.retainedProjection?.let { retained ->
            val token = turnToken
            currentTurnOrigin = retained.turnOrigin
            pendingAssistantIdentity = retained.pendingAssistantIdentity
            optimisticRowsByToken.clear(); optimisticRowsByToken.putAll(retained.optimisticRowsByToken)
            assistantTranscriptUuidsByIdentity.clear(); assistantTranscriptUuidsByIdentity.putAll(retained.assistantTranscriptUuidsByIdentity)
            pendingAssistantTranscriptUuids.clear(); pendingAssistantTranscriptUuids.addAll(retained.pendingAssistantTranscriptUuids)
            assistantRowsByIdentity.clear(); assistantRowsByIdentity.putAll(retained.assistantRowsByIdentity)
            reasoningBeforeResponse = retained.reasoningBeforeResponse
            assistantReasoningByIdentity.clear(); assistantReasoningByIdentity.putAll(retained.assistantReasoningByIdentity)
            assistantIdentityTurnToken = if (retained.assistantIdentityTurnToken == retained.turnToken) token else retained.assistantIdentityTurnToken
            val activeTurnId = retained.turnId
            if (activeTurnId != null) {
                durableTurnId = activeTurnId
                recoveredTurnToken = token
                recoveryReplayTurnId = activeTurnId
                durableTurnUiSequence = retained.afterSequence
                recoveredProjectionFrontier.reset(retained.afterSequence)
                pendingReplayRaw.clear()
                awaitingRetainedAttachSnapshot = true
                retainedAttachSubmissionPending = true
                retainedAttachSnapshotSeen = false
            }
            setNavigationOwner(restored.sessionId, _sourceScope.value)
            _state.value = retained.state.copy(
                session = SessionRef(restored.sessionId, title),
                sessionMode = restored.mode,
                sessionTransitioning = false,
                sessionReady = activeTurnId == null,
                agentRun = retained.state.agentRun?.let { if (it.turnId == retained.turnToken) it.copy(turnId = token) else it },
                agentRunsByMessageId = retained.state.agentRunsByMessageId.mapValues { (_, run) ->
                    if (run.turnId == retained.turnToken) run.copy(turnId = token) else run
                },
            )
            acknowledgedTurnProjection = retainedProjection(_state.value, activeTurnId, retained.afterSequence)
            reportBackgroundTurnState()
            return true
        }
        setNavigationOwner(restored.sessionId, _sourceScope.value)
        visualizationDraftRevision++
        _state.update {
            it.copy(
                session = SessionRef(id = restored.sessionId, title = title),
                sessionMode = restored.mode,
                promptSuggestion = null, promptSuggestionUuid = null,
                messages = restored.transcript, // clear-then-restore (oldest-first)
                visualizationChip = null,
                visualizationFollowup = null,
                streamingMessage = null,
                isNew = restored.transcript.isEmpty() &&
                    (restored.kind == SessionActivationKind.Started || current.isNew),
                streaming = false,
                liveTurnWaitingForUser = false,
                sessionTransitioning = false,
                sessionReady = true,
                statusLine = null,
                shellTools = emptyList(),
                agentRun = null,
                contextUsage = null,
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
                expandedToolCalls = disclosureBySession[canonicalSessionId(restored.sessionId)].orEmpty(),
                workflowRuns = workflowRunsBySession[canonicalSessionId(restored.sessionId)].orEmpty(),
            )
        }
        reportBackgroundTurnState()
        return true
    }

    /** Confirm the ordinary engine's UUID and actual cwd before associating
     * an application with it. Acceptance of Resume/New alone is insufficient. */
    internal suspend fun awaitAppAuthoringSession(
        directory: String,
        expectedSessionId: String? = null,
    ): com.lingxi.code.project.SessionWorkspaceState {
        val owner = source
        return kotlinx.coroutines.withTimeout(20_000) {
            while (true) {
                check(source === owner) { "开发会话已切换，请重新打开应用源码。" }
                val current = _state.value
                val workspace = owner.workspaceState.value
                if (current.sessionReady && !current.sessionTransitioning && current.session.id != "new" &&
                    workspace != null && workspace.transcriptPath.isNotBlank() &&
                    canonicalSessionId(workspace.sessionId) == canonicalSessionId(current.session.id)) {
                    check(expectedSessionId == null || canonicalSessionId(workspace.sessionId) == canonicalSessionId(expectedSessionId)) { "引擎打开了其他会话，请重新打开应用源码。" }
                    check(workspace.cwd == directory) { "来源会话已切换目录，请刷新应用详情。" }
                    return@withTimeout workspace
                }
                check(current.error == null || current.sessionTransitioning) { current.error?.message ?: "无法打开开发会话。" }
                delay(50)
            }
            @Suppress("UNREACHABLE_CODE")
            error("unreachable")
        }
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
        pendingSlash = null
        _state.update { it.copy(commandResult = null) }
        turnToken++
        nativeTurnAdmission.clear()
        recoveredTurnToken = null
        recoveryReplayTurnId = null
        replayThroughSequence = null
        replayBuffer.clear()
        replayTerminalState = null
        durableTurnUiSequence = 0L
        recoveredProjectionFrontier.reset(0L)
        pendingReplayRaw.clear()
        authoritativeTerminalTranscriptTurnId = null
        terminalReplaySettledTurnId = null
        acknowledgedTurnProjection = null
        awaitingRetainedAttachSnapshot = false
        retainedAttachSubmissionPending = false
        retainedAttachSnapshotSeen = false
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
    private suspend fun cancelEngineTurn(
        job: Job?,
        turnId: Long? = null,
        owner: ConversationSource = source,
        submission: NativePromptSubmission? = null,
    ) {
        if (job == null && turnId == null) return
        try {
            val identity = if (submission != null) (owner as? NativePromptReceiptSource)?.currentNativePromptIdentity() else null
            val settled = stopWithSubmissionReceipt(
                stop = { owner.cancel(turnId) },
                submission = submission,
                owner = owner,
                identity = identity,
                turnId = turnId,
                timeoutMs = 5_000L,
                clearCheckpoint = { confirmed ->
                    (owner as? NativePromptReceiptSource)?.clearNotAdmitted(confirmed, checkNotNull(turnId)) == true
                },
            )
            if (settled && source === owner &&
                (identity?.sideId != null || identity?.sessionId?.let(::canonicalSessionId) == canonicalSessionId(_state.value.session.id)) &&
                durableTurnId == turnId) {
                durableTurnId = null
                reportBackgroundTurnState()
            }
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
        newSessionDirectory: String? = null,
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
        setNavigationOwner(target.id, _sourceScope.value)
        visualizationDraftRevision++
        _state.update {
            it.copy(
                session = target,
                promptSuggestion = null, promptSuggestionUuid = null,
                messages = emptyList(),
                visualizationChip = null,
                visualizationFollowup = null,
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
                contextUsage = null,
                agentRunsByMessageId = emptyMap(),
                error = null,
                planTasks = emptyList(),
                planExpanded = false,
                expandedToolCalls = if (newSession) emptySet() else disclosureBySession[canonicalSessionId(target.id)].orEmpty(),
                workflowRuns = workflowRunsBySession[canonicalSessionId(target.id)].orEmpty(),
            )
        }
        sessionTransitionJob = viewModelScope.launch {
            try {
                explicitCancellation?.await()?.getOrThrow()
                explicitCancellation = null
                _state.update { it.copy(cancellationInFlight = false) }
                cancelEngineTurn(detachedTurn)
                if (token != sessionToken) return@launch
                when {
                    newSession && newSessionDirectory != null -> source.newSession(newSessionDirectory)
                    newSession -> source.newSession()
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

    /** The engine's ModelChanged event confirms the model shown in the chip. */
    fun selectModel(model: ModelOption) {
        if (sessionOwnerRejected || model.id.isBlank()) return
        viewModelScope.launch {
            try {
                source.setModel(model.id)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Throwable) {
                reportHostError(error.message ?: error.toString())
            }
        }
    }

    /** The engine's controls snapshot confirms the effort shown in the picker. */
    fun selectReasoning(id: String) {
        if (sessionOwnerRejected || id == _state.value.reasoning?.selection) return
        viewModelScope.launch {
            try {
                source.setReasoningSelection(id)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Throwable) {
                reportHostError(error.message ?: error.toString())
            }
        }
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
        if (_state.value.sessionTransitioning) return
        newChat()
    }

    /** A widget drafted a follow-up: the screen moves it into the composer. */
    fun offerVisualizationFollowup(followup: VisualizationFollowup) {
        _state.update { it.copy(visualizationFollowup = followup) }
    }

    /** The composer took the offered follow-up; its chip now rides the next send. */
    fun acceptVisualizationFollowup(followup: VisualizationFollowup) {
        visualizationDraftRevision++
        _state.update {
            if (it.visualizationFollowup != followup) it
            else it.copy(visualizationFollowup = null, visualizationChip = followup.chip)
        }
    }

    fun clearVisualizationChip() {
        visualizationDraftRevision++
        _state.update { it.copy(visualizationChip = null) }
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
    /** Synchronous composer admission keeps rejected side commands editable. */
    fun admitComposerSend(text: String, hasImages: Boolean): Boolean {
        val current = _state.value
        if (sessionOwnerRejected || !current.sessionReady || current.sessionTransitioning) return false
        if (!source.turnAdmissionReady) {
            reportHostError(strings.resolve(R.string.chat_turn_preparing_retry, "会话正在准备，请稍后重试。"))
            return false
        }
        val trimmed = text.trim()
        if (trimmed.isEmpty()) return false
        if (trimmed.split(Regex("\\s+"), limit = 2).first().equals("/compact", ignoreCase = true) && current.streaming) {
            showCompactRejection()
            return false
        }
        if (source is ExternalTurnConversationSource) return true
        if (nativeTurnAdmission.blocksFollowUp(current.streaming)) {
            reportHostError(strings.resolve(R.string.chat_turn_preparing_retry, "会话正在准备，请稍后重试。"))
            return false
        }
        if (refuseWhileDurableTurnParked() || explicitCancellation?.isActive == true ||
            current.compaction?.status == CompactionProgressStatus.Running ||
            (isManualCompactCommand(trimmed) && hasImages)
        ) return false
        val rejection = sideChatSendRejection(text, hasImages, source is SideChatSource)
            ?: if (sideChatInput(text) != null && source !is EngineConversationSource) SideChatSendRejection.Nested else return true
        reportHostError(when (rejection) {
            SideChatSendRejection.Attachment -> strings.resolve(R.string.side_chat_attachment_rejected, "创建旁聊时请先移除图片附件。")
            SideChatSendRejection.Nested -> strings.resolve(R.string.side_chat_nested_unavailable, "旁聊中无法创建新的旁聊")
        })
        return false
    }

    fun send(
        text: String,
        images: List<ImageRefDto> = emptyList(),
        origin: ConversationTurnOrigin = ConversationTurnOrigin.Ordinary,
        onNotAdmitted: (() -> Unit)? = null,
    ) {
        val trimmed = text.trim()
        if (trimmed.isEmpty()) { onNotAdmitted?.invoke(); return }
        if (trimmed.split(Regex("\\s+"), limit = 2).first().equals("/compact", ignoreCase = true) && _state.value.streaming) {
            showCompactRejection()
            onNotAdmitted?.invoke()
            return
        }
        (source as? ExternalTurnConversationSource)?.let { external ->
            // No side chat, no local turn: the desktop validates the prompt and
            // is the only place a turn starts.
            sendExternal(trimmed, text, images, external)
            return
        }
        val isBashInput = text.startsWith("!")
        val submittedText = if (isBashInput) text.substring(1) else trimmed
        val promptMode = if (isBashInput) PromptModeDto.BASH else null
        if (!admitComposerSend(trimmed, images.isNotEmpty())) { onNotAdmitted?.invoke(); return }
        if (!_state.value.sessionReady || _state.value.sessionTransitioning) { onNotAdmitted?.invoke(); return }
        sideChatInput(trimmed)?.let { input ->
            val parent = source as? EngineConversationSource
            if (parent == null) {
                reportHostError(strings.resolve(R.string.side_chat_nested_unavailable, "旁聊中无法创建新的旁聊"))
                onNotAdmitted?.invoke()
                return
            }
            sideChats.create(input = input, draftOnFailure = text, newSource = parent::newSideChatSource) {
                reportHostError(sideChats.error.value ?: "Unable to create side chat")
                if (onNotAdmitted != null) onNotAdmitted() else onDraftChanged(text)
            }
            return
        }
        if (refuseWhileDurableTurnParked()) { onNotAdmitted?.invoke(); return }
        if (!_state.value.sessionReady || _state.value.sessionTransitioning) { onNotAdmitted?.invoke(); return }
        if (explicitCancellation?.isActive == true) { onNotAdmitted?.invoke(); return }
        if (_state.value.compaction?.status == CompactionProgressStatus.Running) { onNotAdmitted?.invoke(); return }

        if (isManualCompactCommand(trimmed)) {
            if (images.isNotEmpty() || _state.value.streaming) { onNotAdmitted?.invoke(); return }
            val compactSource = source
            val compactMessage = Message(role = Role.User, sentAtMs = System.currentTimeMillis(), text = trimmed)
            savedState?.set(KEY_DRAFT, "")
            _state.update {
                it.copy(
                    isNew = false,
                    statusLine = null,
                    error = null,
                    messages = it.messages + compactMessage,
                    compaction = CompactionProgressUi(
                        status = CompactionProgressStatus.Running,
                    ),
                )
            }
            viewModelScope.launch {
                runCatching { compactSource.submitClientCommand(ClientCommand.ForceCompact) }
                    .onFailure { failure ->
                        if (failure is CancellationException) throw failure
                        val refused = if (compactSource is SideChatSource) isKnownSideChatRejection(failure)
                            else failure is com.lingxi.code.bindings.client.ClientException.Rejected ||
                                isCompactTurnRejection(failure.message.orEmpty())
                        if (refused) {
                            _state.update { state -> state.copy(messages = state.messages.filterNot { it.id == compactMessage.id }) }
                            onNotAdmitted?.invoke()
                        }
                        if (isCompactTurnRejection(failure.message.orEmpty())) {
                            showCompactRejection(removeOptimistic = false)
                            return@onFailure
                        }
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

        // The widget this prompt follows up on rides with it exactly once.
        val visualizationChip = _state.value.visualizationChip

        if (_state.value.streaming) {
            val queuedTurnId = DurableConversationTurnIds.next()
            val queuedUserMessage = Message(role = Role.User, sentAtMs = System.currentTimeMillis(), text = trimmed, images = images, visualizationContext = visualizationChip)
            optimisticRowsByToken["client-turn:$queuedTurnId"] = queuedUserMessage.id
            val queuedSource = source
            val queuedAdmission = NativeSubmissionGate {
                optimisticRowsByToken.remove("client-turn:$queuedTurnId")
                _state.update { it.copy(messages = it.messages.filterNot { row -> row.id == queuedUserMessage.id }) }
                onNotAdmitted?.invoke()
            }
            pendingFollowUps[queuedTurnId] = queuedAdmission
            savedState?.set(KEY_DRAFT, "")
            visualizationDraftRevision++
            _state.update {
                it.copy(
                    isNew = false,
                    error = null,
                    visualizationChip = null,
                    messages = it.messages + queuedUserMessage,
                )
            }
            viewModelScope.launch {
                try {
                    val command = ClientCommand.SendPrompt(
                        text = submittedText.toClientTextDto(), promptMode = promptMode, images = images,
                        turnId = queuedTurnId.toULong(), visualizationContext = visualizationChip?.reference?.toDto(),
                    )
                    if (queuedSource is SideChatSource) queuedSource.submitQueuedPrompt(command, queuedAdmission::enter)
                    else {
                        if (!queuedAdmission.enter()) return@launch
                        queuedSource.submitClientCommand(command)
                    }
                } catch (failure: Throwable) {
                    if (failure is CancellationException) throw failure
                    if (queuedSource is SideChatSource && isKnownSideChatRejection(failure)) queuedAdmission.rejectKnown()
                    _state.update {
                        it.copy(
                            error = ChatError(
                                failure.message ?: failure::class.simpleName.orEmpty(),
                                ChatErrorKind.GENERIC,
                            ),
                        )
                    }
                } finally {
                    pendingFollowUps.remove(queuedTurnId)
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
        if (source is EngineConversationSource || source is RecoveringConversationSource) {
            nativeTurnAdmission.start(submittedTurnId)
        } else {
            nativeTurnAdmission.clear()
        }
        val userMessage = Message(role = Role.User, sentAtMs = System.currentTimeMillis(), text = trimmed, images = images, visualizationContext = visualizationChip)
        optimisticRowsByToken["client-turn:$submittedTurnId"] = userMessage.id
        val admission = NativeSubmissionGate {
            optimisticRowsByToken.remove("client-turn:$submittedTurnId")
            _state.update { state -> state.copy(messages = state.messages.filterNot { it.id == userMessage.id }) }
            onNotAdmitted?.invoke()
        }
        pendingSubmission = admission
        val turnSource = source
        val slashCommandSource = turnSource as? SlashCommandConversationSource
        val isSlash = !isBashInput && isSlashCommandLine(text) && slashCommandSource != null
        val echoSlash = isSlash && slashCommandEchoesRequest(text)
        pendingSlash = if (isSlash) PendingSlash(text, userMessage, submittedTurnId, echoSlash) else null
        val nativePrompt = if (!isSlash && turnSource is NativePromptReceiptSource) {
            NativePromptSubmission(turnSource, submittedTurnId, admission)
        } else null
        pendingNativePrompt = nativePrompt
        durableTurnId = submittedTurnId
        recoveredTurnToken = null
        recoveryReplayTurnId = null
        terminalReplaySettledTurnId = null
        durableTurnUiSequence = 0L
        recoveredProjectionFrontier.reset(0L)
        pendingReplayRaw.clear()
        currentTurnOrigin = origin
        // The draft was just sent — clear the persisted copy.
        savedState?.set(KEY_DRAFT, "")
        visualizationDraftRevision++
        _state.update {
            it.copy(
                isNew = false,
                statusLine = null,
                compaction = null,
                error = null, // a fresh turn clears the prior turn's error banner
                streaming = true, // gate the composer immediately, before the first event
                liveTurnWaitingForUser = false,
                messages = if (!isSlash || echoSlash) it.messages + userMessage else it.messages,
                commandResult = null,
                visualizationChip = null,
                streamingMessage = null,
                agentRun = AgentRunState(turnId = token),
            )
        }
        captureTurnProjection(submittedTurnId, 0L)
        reportBackgroundTurnState()
        // Start while this user action still has a visible Activity. Android 12+
        // generally rejects foreground-service starts attempted only after the
        // process has already crossed into the background.
        setBackgroundTurnActive(true)

        turnJob = viewModelScope.launch {
            try {
                val awaitPath: (suspend () -> String)? = when (turnSource) {
                    is DurableTranscriptSource -> { { turnSource.awaitTurnTranscriptPath() } }
                    else -> null
                }
                if (awaitPath != null) {
                    prepareTurnRecovery(
                        sessionId = _state.value.session.id,
                        turnId = submittedTurnId,
                        awaitTranscriptPath = awaitPath,
                        isCurrent = { source === turnSource && turnToken == token && durableTurnId == submittedTurnId },
                        snapshot = ::currentBackgroundSnapshot,
                        persist = backgroundExecution::prepareSubmission,
                    )
                }
            } catch (cancelled: kotlinx.coroutines.CancellationException) {
                admission.reject()
                throw cancelled
            } catch (failure: Exception) {
                admission.reject()
                reduce(ReplyEvent.Error(failure.message ?: failure::class.simpleName.orEmpty()), token)
                return@launch
            }
            val rejected = { admission.reject(); Unit }
            val knownRejected = { admission.rejectKnown(); Unit }
            val replyStream = when {
                isSlash && turnSource is EngineConversationSource ->
                    turnSource.submitSlashForComposer(text.toClientTextDto(), images, submittedTurnId, rejected, admission::enter)
                isSlash && turnSource is RecoveringConversationSource ->
                    turnSource.submitSlashForComposer(text.toClientTextDto(), images, submittedTurnId, rejected, admission::enter)
                isSlash -> {
                    if (!admission.enter()) throw CancellationException("Turn cancelled before submission")
                    slashCommandSource!!.submitSlashCommand(text.toClientTextDto(), images, submittedTurnId)
                }
                turnSource is NativePromptReceiptSource ->
                    turnSource.submitForComposer(
                        submittedText, images, submittedTurnId, visualizationChip?.reference, promptMode,
                        if (turnSource is SideChatSource) knownRejected else rejected, admission::enter,
                        bindNativeIdentity = { identity ->
                            source === turnSource && turnToken == token && durableTurnId == submittedTurnId &&
                                (identity.sideId != null || canonicalSessionId(_state.value.session.id) == canonicalSessionId(identity.sessionId)) &&
                                nativePrompt?.bindIdentity(turnSource, identity, submittedTurnId) == true
                        },
                        onNativeResult = { nativePrompt?.record(it) },
                    )
                turnSource is SideChatSource ->
                    turnSource.submitForComposer(submittedText, images, submittedTurnId, visualizationChip?.reference, promptMode, knownRejected, admission::enter)
                else -> {
                    if (!admission.enter()) throw CancellationException("Turn cancelled before submission")
                    turnSource.submit(submittedText, images, submittedTurnId, visualizationChip?.reference, promptMode)
                }
            }
            replyStream.collect { event ->
                reduce(event, token)
                if (
                    event is ReplyEvent.DurableTurnReplayAcknowledged &&
                    token == turnToken &&
                    durableTurnId == submittedTurnId && event.turnId == submittedTurnId
                ) {
                    durableTurnUiSequence = maxOf(durableTurnUiSequence, event.sequence)
                    captureTurnProjection(submittedTurnId, durableTurnUiSequence)
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
        if (sessionOwnerRejected) return
        if (source is ExternalTurnConversationSource) {
            // The turn ends when the desktop reports it; nothing is settled here.
            viewModelScope.launch {
                runCatching { source.cancel() }.onFailure { reportHostError(it.message ?: it.toString()) }
            }
            return
        }
        if (inactiveWaitingTurnId != null) {
            discardRecoveredTurn()
            return
        }
        if (!_state.value.streaming && !_state.value.liveTurnWaitingForUser) return
        pendingSubmission?.reject()
        pendingFollowUps.values.toList().forEach { it.reject() }
        val stoppedOwner = source
        val stoppedTurnId = durableTurnId
        val stoppedSubmission = pendingNativePrompt
        val cancelledToken = turnToken
        val cancelledText = _state.value.streamingMessage?.text.orEmpty()
        // Supersede the turn: a late event arriving after the engine's Cancel
        // round-trip must not re-open streaming on the now-idle transcript.
        val job = abandonLocalTurn()
        _state.update {
            val settled = it.settleTurn(
                run = it.agentRun?.finish(AgentRunOutcome.Cancelled),
                settling = it.streamingMessage?.withoutPendingVisualizations(),
            )
            it.copy(
                streaming = false,
                cancellationInFlight = true,
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
            runCatching { cancelEngineTurn(job, stoppedTurnId, stoppedOwner, stoppedSubmission) }
        }
        explicitCancellation = cancellation
        viewModelScope.launch {
            val result = cancellation.await()
            if (explicitCancellation === cancellation) {
                explicitCancellation = null
                _state.update { it.copy(cancellationInFlight = false) }
            }
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

    internal fun hasActiveComputerUseTurn(): Boolean {
        val current = _state.value
        return computerUseBelongsToTurn(current.computerUseRequestKey, turnToken,
            current.streaming || current.liveTurnWaitingForUser || inactiveWaitingTurnId != null)
    }

    fun updatePromptSuggestionContext(sessionId: String, focus: String, inputEmpty: Boolean, pendingPermission: Boolean) {
        viewModelScope.launch {
            runCatching { source.submitClientCommand(ClientCommand.UpdatePromptSuggestionContext(
                sessionId, focus, inputEmpty, pendingPermission, false,
            )) }
        }
    }

    fun promptSuggestionFeedback(sessionId: String, uuid: String, action: String) {
        viewModelScope.launch {
            runCatching { source.submitClientCommand(ClientCommand.PromptSuggestionFeedback(sessionId, uuid, action)) }
        }
    }

    private fun showCompactRejection(removeOptimistic: Boolean = true) {
        _state.update { state ->
            val optimistic = state.compaction?.let {
                it.status == CompactionProgressStatus.Running && it.phase == "queued"
            } == true
            state.copy(
                compactRejectionId = java.util.UUID.randomUUID().toString(),
                compaction = if (optimistic) null else state.compaction,
                messages = if (removeOptimistic && optimistic && state.messages.lastOrNull()?.let {
                    it.role == Role.User && isManualCompactCommand(it.text)
                } == true) state.messages.dropLast(1) else state.messages,
            )
        }
    }

    fun dismissCompactRejection() { _state.update { it.copy(compactRejectionId = null) } }

    fun dismissCommandResult() { _state.update { it.copy(commandResult = null) } }

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
        val sessionId = canonicalSessionId(_state.value.session.id)
        if (sessionId.isNotBlank() && sessionId != "new") {
            disclosureBySession.remove(sessionId)
            disclosureBySession[sessionId] = _state.value.expandedToolCalls
            savedState?.set(KEY_DISCLOSURES, encodeDisclosures(disclosureBySession))
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
        if (event is ReplyEvent.End && settledSlashToken == token) return // mapper's trailing End cannot recreate a utility run
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
                        streamingMessage = live.appendingStreamText(event.text),
                        agentRun = run,
                    )
                } else {
                    // First delta of the turn: open a new assistant message.
                    s.copy(
                        streaming = true,
                        streamingMessage = Message(role = Role.Ai, text = event.text, transcriptUuid = pendingAssistantTranscriptUuids.firstOrNull(), transcriptAliases = pendingAssistantTranscriptUuids.toSet(), streamed = true),
                        agentRun = run,
                    )
                }
            }

            is ReplyEvent.Visualization -> _state.update { s ->
                val live = s.streamingMessage ?: Message(role = Role.Ai, text = "")
                s.copy(
                    streaming = true,
                    streamingMessage = live.applyingVisualizationBlock(event.status, event.reference),
                    agentRun = (s.agentRun ?: AgentRunState(turnId = token)).markGenerating(),
                )
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
                        syntheticShellCommand = replacement.command,
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

            is ReplyEvent.CommandOutput -> {
                if (event.turnId != null && event.turnId != durableTurnId?.toULong()) return
                val pending = pendingSlash
                val inMessages = pending?.echoed == true
                settledSlashToken = token
                pendingSlash = null
                nativeTurnAdmission.clear()
                turnJob = null
                emitTurnCompletion(
                    token = token,
                    outcome = if (event.isError) {
                        ConversationTurnOutcome.Failed
                    } else {
                        ConversationTurnOutcome.Completed
                    },
                    finalAssistantText = event.display,
                )
                _state.update { state ->
                    state.copy(
                        streaming = false,
                        messages = if (inMessages && event.display.isNotBlank()) state.messages + Message(role = Role.Ai, text = event.display) else state.messages,
                        commandResult = if (!inMessages && event.display.isNotBlank()) ConversationCommandOutput(pending?.raw.orEmpty(), event.display, event.isError) else null,
                        streamingMessage = null,
                        statusLine = null,
                        error = null, // Error output is already visible in its result surface.
                        // Local commands have no model/tool trace to retain.
                        agentRun = null,
                    )
                }
            }

            is ReplyEvent.Usage -> _state.update {
                it.copy(
                    contextUsage = event.usage,
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
                    val uuids = assistantTranscriptUuidsByIdentity[identity].orEmpty().ifEmpty { pendingAssistantTranscriptUuids.toList() }
                    val completed = (state.streamingMessage?.let { live -> message.copy(id = live.id, streamed = true) } ?: message)
                        .copy(transcriptUuid = message.transcriptUuid ?: uuids.firstOrNull(), transcriptAliases = message.transcriptAliases + uuids)
                    pendingAssistantTranscriptUuids.clear()
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
                if (pendingSlash?.echoed == false) {
                    reduce(ReplyEvent.CommandOutput(display = event.message, isError = true, turnId = durableTurnId?.toULong()), token)
                    return
                }
                pendingSlash = null
                nativeTurnAdmission.clear()
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
                        settling = it.streamingMessage?.withoutPendingVisualizations(),
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
                nativeTurnAdmission.clear()
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
                        ?.let { live -> event.message.copy(id = live.id, streamed = true) }
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
                nativeTurnAdmission.clear()
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
                        settling = it.streamingMessage?.withoutPendingVisualizations(),
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

    override fun onCleared() {
        pendingAuthoringRetry = null
        sideChats.release()
        val backgroundSnapshot = currentBackgroundSnapshot().takeIf { _state.value.requiresBackgroundExecution }
        if (backgroundSnapshot != null) {
            val current = _state.value
            val captured = backgroundSnapshot.turnId?.let { turnId ->
                acknowledgedTurnProjection?.takeIf {
                    it.turnId == turnId && canonicalSessionId(it.sessionId) == canonicalSessionId(backgroundSnapshot.sessionId)
                }
            }
            val projection = if (captured != null) {
                val savedIds = captured.state.messages.mapTo(mutableSetOf()) { it.id }
                val laterUserRows = current.messages.filter { it.role == Role.User && it.id !in savedIds }
                captured.copy(state = captured.state.copy(
                    messages = captured.state.messages + laterUserRows,
                    expandedToolCalls = current.expandedToolCalls,
                    activeBackgroundTaskIds = current.activeBackgroundTaskIds,
                    backgroundTasks = current.backgroundTasks,
                    workflowRuns = current.workflowRuns,
                    pendingQuestions = current.pendingQuestions,
                ))
            } else retainedProjection(current, backgroundSnapshot.turnId, 0L)
            source.retainUiTranscript(projection)
        }
        val retained = backgroundSnapshot?.let { backgroundExecution.retainAfterUiDestroyed(source, it) } == true
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

    /**
     * Hands new notification preferences to the platform notifier. It holds
     * armed timers (upstream's 60s idle delay and 6s permission delay), so a
     * preference it is never told about would keep applying until the next
     * process start.
     */
    fun setNotificationPreferences(config: NotifConfig) {
        backgroundExecution.setNotificationPreferences(config)
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
        const val KEY_DRAFT = "chat.draft" // String — unsent composer text
        const val KEY_DRAFT_SCOPE = "chat.draft.scope"
        const val KEY_DRAFT_SESSION_ID = "chat.draft.session.id"
        const val KEY_SESSION_ID = "chat.session.id" // String
        const val KEY_SESSION_TITLE = "chat.session.title" // String
        const val KEY_SESSION_SCOPE = "chat.session.scope" // String — workspace identity
        const val KEY_IS_NEW = "chat.isNew" // Boolean — empty-state hero vs list
        const val KEY_DISCLOSURES = "chat.disclosures" // bounded JSON of session -> stable item ids

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
