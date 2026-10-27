package com.lingxi.code.conversation

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.lingxi.code.R
import com.lingxi.code.bindings.client.AskUserQuestionAnswersDto
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ClientException
import com.lingxi.code.bindings.client.ClientTextDto
import com.lingxi.code.bindings.client.AttachmentDto
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.PromptModeDto
import com.lingxi.code.bindings.client.QueuedTurnStatusDto
import com.lingxi.code.bindings.client.ListingKindDto
import com.lingxi.code.bindings.client.ModSessionEventPayload
import com.lingxi.code.bindings.android.MobileLinuxEventFfi
import com.lingxi.code.bindings.android.MobileLinuxEventKindFfi
import com.lingxi.code.bindings.android.MobileLinuxTaskSnapshotFfi
import com.lingxi.code.bindings.android.MobileLinuxTaskStateFfi
import com.lingxi.code.bindings.client.PermissionResponseDto
import com.lingxi.code.bindings.client.TaskStatusDto
import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import com.lingxi.code.bindings.client.TurnRecoverySnapshotDto
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
import kotlinx.coroutines.isActive
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
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
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull

private const val ANDROID_COMPUTER_USE_TOOL = "android_use"

internal class DurableTurnIdAllocator(initial: Long, private val reserved: () -> Set<Long>,
    private val persist: (Long) -> Boolean = { true }) {
    private val counter = AtomicLong(initial)
    @Synchronized fun advanceAtLeast(value: Long) {
        if (value > counter.get()) counter.set(value)
    }
    @Synchronized fun next(): Long? {
        val existing = reserved()
        while (counter.get() < Long.MAX_VALUE) {
            val candidate = counter.get() + 1
            if (candidate !in existing && !persist(candidate)) return null
            counter.set(candidate)
            if (candidate !in existing) return candidate
        }
        return null
    }
}

internal object DurableConversationTurnIds {
    private var recoveryPreferences: android.content.SharedPreferences? = null
    private var idPreferences: android.content.SharedPreferences? = null
    private val allocator = DurableTurnIdAllocator(
        SecureRandom().nextLong().and(Long.MAX_VALUE).coerceAtLeast(1L),
        reserved = {
            recoveryPreferences?.all?.asSequence()?.mapNotNull { (key, value) ->
                when {
                    (key == "turn_id" || key.endsWith("_turn_id")) && value is Long -> value
                    value is String && (key.contains("_queued_") || key.contains("_parked_") || key.contains("_terminal_")) ->
                        key.substringAfterLast('_').toLongOrNull()
                    else -> null
                }
            }?.toSet().orEmpty()
        },
        persist = { candidate -> idPreferences?.edit()?.putLong("allocated_turn_counter", candidate)?.commit() ?: true },
    )

    @Synchronized fun initialize(context: android.content.Context) {
        recoveryPreferences = context.applicationContext.getSharedPreferences("durable_conversation_turn", android.content.Context.MODE_PRIVATE)
        val ids = context.applicationContext.getSharedPreferences("durable_conversation_turn_ids", android.content.Context.MODE_PRIVATE)
        idPreferences = ids
        allocator.advanceAtLeast(ids.getLong("allocated_turn_counter", 0L))
    }
    @Synchronized fun next(): Long? = allocator.next()
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
    internal val sideChats = SideChatManager(viewModelScope)

    internal val supportsSideChats: Boolean get() = source is EngineConversationSource

    internal fun openSideChats() {
        if (sideChats.chats.value.isNotEmpty()) sideChats.visible.value = true
        else createSideChat()
    }

    internal fun createSideChat() {
        if (sideChats.isCreating.value) return
        val parent = source as? EngineConversationSource ?: return
        sideChats.create(input = "", draftOnFailure = null, newSource = parent::newSideChatSource) {
            reportHostError(sideChats.error.value ?: "Unable to create side chat")
        }
    }

    /** Provider reconnect token is independent from workspace-source swaps. */
    private var reconnectGeneration: Int = sourceGeneration
    private var requestedSourceGeneration: Int? = null

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
     * Scheduled, or a Project workspace. The generalization of
     * [sourceProjectId] (which stays for the project-only flows): the drawer
     * and the scope-restore effects branch on this.
     */
    private val _sourceScope = MutableStateFlow<ConversationScope>(ConversationScope.Global)
    val sourceScope: StateFlow<ConversationScope> = _sourceScope.asStateFlow()

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

    /** Mirror the live composer draft into saved state (called as the user types). */
    fun onDraftChanged(draft: String) {
        savedState?.set(KEY_DRAFT, draft)
    }

    /** Collector for the active reply stream. */
    private var turnJob: Job? = null
    private data class ActiveSubmission(
        val source: ConversationSource,
        val turnId: Long,
        val admission: TurnSubmissionAdmission,
        var sessionId: String?,
    )
    private var activeSubmission: ActiveSubmission? = null
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
    private data class DurableAttachInterest(
        val sessionId: String,
        val turnId: Long,
        val expectsResume: Boolean,
        val requestId: String?,
        var attaching: Boolean = true,
        var sawSnapshot: Boolean = false,
        var terminalSnapshot: TurnRecoverySnapshotDto? = null,
    )
    private var durableAttachInterest: DurableAttachInterest? = null
    private data class CancellingProjection(
        val sessionId: String, val turnId: Long, val generation: Int, var terminalObserved: Boolean = false,
        val finished: CompletableDeferred<Boolean> = CompletableDeferred(),
    )
    private var cancellingProjection: CancellingProjection? = null
    private data class PendingAdmissionRecovery(val source: ConversationSource, val sessionId: String,
        val turnId: Long, val generation: Int, val requiresResumeActivation: Boolean = false,
        val trustNotFound: Boolean = true)
    private var pendingAdmissionRecovery: PendingAdmissionRecovery? = null
    private var admissionRecoveryJob: Job? = null
    private data class TerminalRecoveryReview(val source: ConversationSource, val sessionId: String,
        val generation: Int, val turns: List<Long>)
    private var terminalRecoveryReview: TerminalRecoveryReview? = null
    private data class QueuedStatusRequest(val source: ConversationSource, val sessionId: String,
        val turnId: Long, val generation: Int, val visibleUuids: Set<String>,
        val completed: CompletableDeferred<Unit> = CompletableDeferred())
    private val queuedStatusRequests = mutableMapOf<String, QueuedStatusRequest>()
    private var queuedStatusJob: Job? = null
    private data class TransitionCancellation(val sessionId: String, val turnId: Long,
        val consumed: CompletableDeferred<Boolean> = CompletableDeferred())
    private var transitionCancellation: TransitionCancellation? = null
    private data class QueuedTurnInterest(val sessionId: String, val generation: Int)
    private val queuedTurnInterests = mutableMapOf<Long, QueuedTurnInterest>()
    // The source shares leased watermarks across consumers. This reducer keeps
    // only its current main-turn version after identity fencing.
    private val recoveryRevisions = ActiveRecoveryRevisionGuard()
    /** Token used when a durable turn is attached without a local submit collector. */
    private var recoveredTurnToken: Long? = null
    /** Non-null only while AttachTurn is replaying retained history. */
    private var recoveryReplayTurnId: Long? = null
    /** UI-owner-local durable replay cursor. */
    private var durableTurnUiSequence: Long = 0L
    private var durableReplayDegraded = false
    private val pendingDurableEvents = mutableMapOf<ULong, ConversationDelivery.TurnEvent>()
    /** A recovered live event has rendered and awaits its replay envelope. */
    private var pendingRecoveredReplayAckTurnId: Long? = null
    /** Session whose SessionResumed transcript is currently authoritative. */
    private var restoredTranscriptSessionId: String? = null
    private var restoredTranscriptHasContent = false
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
        optimisticRowsByToken.clear()
        assistantTranscriptUuidsByIdentity.clear()
        pendingAssistantTranscriptUuids.clear()
        sourceBindingJob?.cancel()
        _state.update { it.copy(slashCommands = emptyList()) }
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
                boundSource.activeSessionState.collect { activated ->
                    if (sourceGeneration == generation) {
                        activated?.let {
                            applyActivatedSession(it)
                            if (!boundSource.recoveryStorageAvailable()) {
                                markRecoveryStorageFailed()
                                return@collect
                            }
                            val session = canonicalSessionId(it.sessionId)
                            val receipts = boundSource.terminalRecoveryReceipts(session)
                            val cancelled = boundSource.cancelledQueuedReceipts(session).toSet()
                            val pendingQueue = boundSource.pendingQueuedRecoveryTurns(session)
                            if (sourceGeneration != generation || source !== boundSource || !isVisibleSession(session)) return@collect
                            pendingQueue.forEach { turn -> queuedTurnInterests[turn] = QueuedTurnInterest(session, generation) }
                            if (pendingQueue.isNotEmpty()) _state.update { state -> state.copy(queuedRecoveryWarning = true) }
                            terminalRecoveryReview = if (receipts.isEmpty()) null else TerminalRecoveryReview(boundSource, session, generation, receipts)
                            val uncertain = receipts.any { it !in cancelled }
                            _state.update { state -> state.copy(terminalRecoveryReviewAvailable = receipts.isNotEmpty(),
                                terminalRecoveryUncertain = uncertain, queuedCancellationNotice = receipts.any { it in cancelled }) }
                            if (it.kind == SessionActivationKind.Resumed && pendingQueue.isNotEmpty()) {
                                queryQueuedRecoveries(boundSource, generation, session, pendingQueue, it.fullyVisibleUserUuids)
                            }
                            val pending = pendingAdmissionRecovery
                            if (pending != null && pending.source === boundSource && pending.generation == generation &&
                                canonicalSessionId(it.sessionId) == pending.sessionId
                            ) {
                                if (pending.requiresResumeActivation && it.kind != SessionActivationKind.Resumed) return@collect
                                if (pending.requiresResumeActivation) {
                                    pendingAdmissionRecovery = pending.copy(requiresResumeActivation = false)
                                }
                                durableTurnId = pending.turnId
                                inactiveWaitingTurnId = pending.turnId
                                _state.update { state -> state.copy(durableRecoveryBlocked = true, recoveryRetryAvailable = true) }
                                retryDurableRecovery()
                                return@collect
                            }
                            runCatching {
                                boundSource.attachDurableTurnForUi(afterSequence = durableTurnUiSequence) { sessionId, turnId, expectsResume, requestId ->
                                    if (sourceGeneration == generation) noteDurableAttach(sessionId, turnId, expectsResume, requestId)
                                }
                            }.onFailure { error ->
                                if (sourceGeneration == generation && (error as? DurableAttachFailure)?.orderedDelivery != true) {
                                    preserveDurableRecoveryAfterAttachFailure(error)
                                }
                            }
                        }
                    }
                }
            }
            launch {
                boundSource.pendingPermission.collect { prompt ->
                    if (sourceGeneration != generation) return@collect
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
                        if (sourceGeneration == generation) ingestExternal(item)
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
            launch(start = CoroutineStart.UNDISPATCHED) {
                // The generic out-of-band event stream: interactive questions
                // and background-task transitions ride here, NOT the per-turn
                // reply stream — a question can outlive its turn's stream and
                // a task can finish with no turn in flight at all.
                boundSource.conversationDeliveries.collect { delivery ->
                    if (sourceGeneration != generation) return@collect
                    when (delivery) {
                        is ConversationDelivery.Event -> {
                            // Ownerless connection notices cannot be attached to
                            // whichever turn happens to be visible now.
                            if ((boundSource !is EngineConversationSource && boundSource !is RecoveringConversationSource) ||
                                delivery.event !is ClientEvent.SystemNotice
                            ) {
                                reduceClientEvent(delivery.event)
                            }
                        }
                        is ConversationDelivery.RecoveryState -> if (delivery.isActive()) {
                            reduceClientEvent(ClientEvent.TurnRecoveryState(delivery.snapshot))
                        }
                        is ConversationDelivery.TurnEvent -> reduceDurableTurnDelivery(delivery)
                        is ConversationDelivery.TurnEventDecodeFailed -> {
                            if (delivery.isActive() && ownsDurableDelivery(delivery.sessionId, delivery.turnId) &&
                                (delivery.attachRequestId == null || durableAttachInterest?.requestId == delivery.attachRequestId)
                            ) markDurableReplayIncomplete()
                        }
                        is ConversationDelivery.QueuedTurnIncomplete -> {
                            if (matchesQueuedTurn(delivery.sessionId, delivery.turnId)) {
                                queuedTurnInterests.remove(delivery.turnId.toLong())
                                showDurableRecoveryWarning()
                            }
                        }
                        is ConversationDelivery.QueuedTurnRegistered -> {
                            if (delivery.turnId <= Long.MAX_VALUE.toULong() && isVisibleSession(delivery.sessionId)) {
                                queuedTurnInterests[delivery.turnId.toLong()] = QueuedTurnInterest(
                                    canonicalSessionId(delivery.sessionId), sourceGeneration,
                                )
                            }
                        }
                        is ConversationDelivery.QueuedTurnSettled -> {
                            if (matchesQueuedTurn(delivery.sessionId, delivery.turnId)) {
                                queuedTurnInterests.remove(delivery.turnId.toLong())
                                val session = canonicalSessionId(delivery.sessionId)
                                if (queuedTurnInterests.values.none { it.sessionId == session && it.generation == generation }) {
                                    _state.update { state -> state.copy(queuedRecoveryWarning = false) }
                                }
                                if (delivery.reviewRequired) {
                                    launch {
                                        val receipts = boundSource.terminalRecoveryReceipts(session)
                                        val cancelled = boundSource.cancelledQueuedReceipts(session).toSet()
                                        if (sourceGeneration == generation && source === boundSource && isVisibleSession(session) && receipts.isNotEmpty()) {
                                            terminalRecoveryReview = TerminalRecoveryReview(boundSource, session, generation, receipts)
                                            val uncertain = receipts.any { it !in cancelled }
                                            _state.update { state -> state.copy(terminalRecoveryReviewAvailable = true,
                                                terminalRecoveryUncertain = uncertain, queuedCancellationNotice = receipts.any { it in cancelled }) }
                                        }
                                    }
                                }
                            }
                        }
                        is ConversationDelivery.CancellationDrained -> {
                            finishCancelledProjection(delivery)
                            transitionCancellation?.takeIf { it.turnId == delivery.turnId && it.sessionId == canonicalSessionId(delivery.sessionId) }
                                ?.consumed?.complete(delivery.terminalObserved)
                        }
                        is ConversationDelivery.CancellationAborted -> {
                            abortCancelledProjection(delivery)
                            transitionCancellation?.takeIf { it.turnId == delivery.turnId && it.sessionId == canonicalSessionId(delivery.sessionId) }
                                ?.consumed?.complete(false)
                        }
                        is ConversationDelivery.AttachStarted -> {
                            val interest = durableAttachInterest
                            if (delivery.isActive() && interest?.requestId == delivery.requestId &&
                                interest.sessionId == canonicalSessionId(delivery.snapshot.sessionId) &&
                                interest.turnId == delivery.snapshot.turnId.toLong()
                            ) {
                                if (delivery.admissionPending) {
                                    if (pendingAdmissionRecovery?.let { it.source === boundSource && it.sessionId == interest.sessionId &&
                                            it.turnId == interest.turnId } != true) {
                                        pendingAdmissionRecovery = PendingAdmissionRecovery(boundSource, interest.sessionId,
                                            interest.turnId, generation, trustNotFound = false)
                                    }
                                    _state.update { state -> state.copy(recoveryRetryAvailable = false,
                                        error = ChatError(strings.resolve(R.string.chat_recovery_unknown,
                                            "暂时无法确认操作状态，请重试。"), ChatErrorKind.GENERIC)) }
                                }
                                reduceClientEvent(ClientEvent.TurnRecoveryState(delivery.snapshot), delivery.requestId)
                            }
                        }
                        is ConversationDelivery.AdmissionPending -> {
                            val interest = durableAttachInterest
                            if (delivery.isActive() && interest?.requestId == delivery.requestId &&
                                interest.sessionId == canonicalSessionId(delivery.sessionId) && interest.turnId == delivery.turnId
                            ) {
                                if (pendingAdmissionRecovery?.let { it.source === boundSource && it.sessionId == interest.sessionId &&
                                        it.turnId == interest.turnId } != true) {
                                    pendingAdmissionRecovery = PendingAdmissionRecovery(boundSource, interest.sessionId,
                                        interest.turnId, generation, trustNotFound = false)
                                }
                                durableTurnId = interest.turnId
                                inactiveWaitingTurnId = interest.turnId
                                _state.update { state -> state.copy(durableRecoveryBlocked = true, recoveryRetryAvailable = false,
                                    error = ChatError(strings.resolve(R.string.chat_recovery_unknown,
                                        "暂时无法确认操作状态，请重试。"), ChatErrorKind.GENERIC)) }
                            }
                        }
                        is ConversationDelivery.AttachReplayEnded -> {
                            try {
                                val accepted = finishDurableAttachReplay(delivery)
                                delivery.acknowledged?.complete(accepted)
                            }
                            catch (error: Throwable) { delivery.acknowledged?.completeExceptionally(error); throw error }
                        }
                        is ConversationDelivery.AttachFailed -> {
                            if (canonicalSessionId(delivery.sessionId) == canonicalSessionId(_state.value.session.id) &&
                                durableAttachInterest?.let { it.turnId == delivery.turnId && it.requestId == delivery.requestId } == true
                            ) preserveDurableRecoveryAfterAttachFailure(delivery.error)
                        }
                        is ConversationDelivery.ReconciliationNotFound -> {
                            val interest = durableAttachInterest
                            val matching = interest != null && interest.requestId == delivery.requestId &&
                                interest.sessionId == canonicalSessionId(delivery.sessionId) && interest.turnId == delivery.turnId
                            val accepted = when {
                                !delivery.isActive() -> false
                                !matching -> null
                                else -> {
                                    clearDurableRecoveryAfterTerminal(delivery.turnId, hadMatchingDurableIdentity = true)
                                    _state.update { it.copy(streaming = false, cancellationInFlight = false,
                                        error = ChatError(strings.resolve(R.string.chat_recovery_not_found,
                                            "无法找到该运行记录，请检查对话后继续。"), ChatErrorKind.GENERIC)) }
                                    true
                                }
                            }
                            delivery.acknowledged?.complete(accepted)
                        }
                        ConversationDelivery.RecoveryStorageFailed -> markRecoveryStorageFailed()
                        is ConversationDelivery.RecoveryInterestEnded -> Unit // consumed by the tracked flow
                    }
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
    internal fun reduceClientEvent(event: ClientEvent, correlatedAttachRequest: String? = null) {
        if (event is ClientEvent.QueuedTurnStatus) {
            handleQueuedStatus(event)
            return
        }
        if (event is ClientEvent.TurnRecoveryState) {
            val snapshot = event.snapshot
            if (!isVisibleSession(snapshot.sessionId)) return
            val ownedTurn = durableTurnId ?: inactiveWaitingTurnId ?: liveWaitingTurnId
            val interest = durableAttachInterest
            if (interest != null && interest.requestId != null && interest.attaching && !interest.sawSnapshot &&
                interest.sessionId == canonicalSessionId(snapshot.sessionId) && interest.turnId == snapshot.turnId.toLong() &&
                correlatedAttachRequest != interest.requestId
            ) return
            val interested = interest != null && interest.sessionId == canonicalSessionId(snapshot.sessionId) &&
                interest.turnId == snapshot.turnId.toLong()
            val queuedStart = ownedTurn == null && snapshot.state == TurnRecoveryStateDto.RUNNING &&
                matchesQueuedTurn(snapshot.sessionId, snapshot.turnId)
            if (ownedTurn != null && ownedTurn != snapshot.turnId.toLong()) return
            if (ownedTurn == null && !interested && !queuedStart) return
            if (!recoveryRevisions.accept(snapshot)) return
            bindVerifiedInitialSession(snapshot.sessionId)
            if (queuedStart) adoptQueuedTurn(snapshot.sessionId, snapshot.turnId)
            if (cancellingProjection?.let {
                    it.generation == sourceGeneration && it.sessionId == canonicalSessionId(snapshot.sessionId) &&
                        it.turnId == snapshot.turnId.toLong()
                } == true && snapshot.state in setOf(
                    TurnRecoveryStateDto.COMPLETED, TurnRecoveryStateDto.FAILED, TurnRecoveryStateDto.CANCELLED,
                )
            ) {
                cancellingProjection?.terminalObserved = true
                return
            }
        }
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
                    messages = it.messages + Message(role = Role.Ai, text = output),
                    error = if (event.isError) ChatError(output, classifyError(output)) else it.error,
                )
            }
            return
        }
        planTasksFrom(event)?.let { tasks ->
            _state.update { it.copy(planTasks = tasks) }
            return
        }
        when (event) {
            is ClientEvent.UserTranscriptRowIdentity -> {
                if (!event.rowToken.startsWith("client-turn:")) return
                event.rowToken.removePrefix("client-turn:").split(',').forEach { value ->
                    value.toLongOrNull()?.let(queuedTurnInterests::remove)
                }
                val rowIds = event.rowToken.removePrefix("client-turn:").split(',')
                    .mapNotNull { id -> id.takeIf { it.isNotEmpty() && it.all(Char::isDigit) }?.let { optimisticRowsByToken.remove("client-turn:$it") } }.toSet()
                if (rowIds.isEmpty()) return
                _state.update { state -> state.copy(messages = state.messages.map { message ->
                    if (message.id in rowIds) message.copy(transcriptUuid = event.uuid) else message
                }) }
            }
            is ClientEvent.ModSessionEvent -> {
                if (_state.value.sessionTransitioning ||
                    !isVisibleSession(event.sessionId)
                ) return
                bindVerifiedInitialSession(event.sessionId)
                when (val payload = event.payload) {
                    is ModSessionEventPayload.Notice -> if (payload.isError) {
                        val text = payload.message.displayText()
                        _state.update { it.copy(error = ChatError(text, classifyError(text))) }
                    } else _state.update { it.copy(statusLine = payload.message.displayText()) }
                    is ModSessionEventPayload.Log -> _state.update { it.copy(statusLine = "${payload.plugin}: ${payload.text.displayText()}") }
                    is ModSessionEventPayload.Toast -> _state.update { it.copy(statusLine = "${payload.plugin}: ${payload.text.displayText()}") }
                    is ModSessionEventPayload.Notify -> _state.update { it.copy(statusLine = "${payload.plugin}: ${payload.text.displayText()}") }
                    is ModSessionEventPayload.Status -> _state.update { it.copy(statusLine = payload.text?.displayText()) }
                }
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
                    )
                })
            }
            is ClientEvent.TurnStarted -> {
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
                is ClientEvent.TaskRow -> _state.update {
                    val incoming = event.task.toBackgroundTaskUi()
                    val exitedUnknown = incoming.status == TaskStatusDto.EXITED_UNKNOWN ||
                        incoming.taskId in it.exitedUnknownTaskIds
                    val task = if (exitedUnknown) incoming.copy(status = TaskStatusDto.EXITED_UNKNOWN) else incoming
                    it.copy(
                        backgroundTasks = it.backgroundTasks + (task.taskId to task),
                        exitedUnknownTaskIds = if (exitedUnknown) it.exitedUnknownTaskIds + task.taskId else it.exitedUnknownTaskIds,
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
                val current = _state.value
                val belongsToVisibleSession = event.originSessionId
                    ?.let(::canonicalSessionId)
                    ?.let { origin -> origin == canonicalSessionId(current.session.id) }
                    ?: true
                val knownUnknownInOrigin = event.originSessionId
                    ?.let(::canonicalSessionId)
                    ?.let { workflowStatusesBySession[it]?.get(event.taskId) == TaskStatusDto.EXITED_UNKNOWN }
                    ?: false
                // An observed unknown exit cannot transition to another state.
                // Only compare against evidence for the event's own origin.
                if (knownUnknownInOrigin || (belongsToVisibleSession && event.taskId in current.exitedUnknownTaskIds)) return
                // `TaskStatusDto` is a UniFFI-generated Kotlin ENUM, not a
                // sealed class — `is` would not compile.
                val terminal = event.status == TaskStatusDto.COMPLETED || event.status == TaskStatusDto.FAILED
                if (terminal && belongsToVisibleSession) {
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
                        exitedUnknownTaskIds = if (event.status == TaskStatusDto.EXITED_UNKNOWN) {
                            it.exitedUnknownTaskIds + event.taskId
                        } else {
                            it.exitedUnknownTaskIds
                        },
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
                val interest = durableAttachInterest?.takeIf {
                    it.sessionId == canonicalSessionId(snapshot.sessionId) && it.turnId == recoveredId
                }
                val firstAttachSnapshot = interest != null && !interest.sawSnapshot
                if (firstAttachSnapshot) interest?.sawSnapshot = true
                if (firstAttachSnapshot && durableTurnUiSequence == 0L && snapshot.projectionUncertain) {
                    showDurableRecoveryWarning(
                        strings.resolve(R.string.chat_recovery_projection_uncertain, "部分运行内容可能未恢复，请检查后继续。"),
                    )
                }
                if (firstAttachSnapshot && durableTurnUiSequence == 0L && snapshot.firstSequence > 1uL) {
                    markDurableReplayIncomplete()
                }
                if (firstAttachSnapshot && snapshot.reason in setOf("replay_history_truncated", "replay_cursor_ahead")) {
                    markDurableReplayIncomplete()
                }
                val terminal = snapshot.state in setOf(
                    TurnRecoveryStateDto.COMPLETED,
                    TurnRecoveryStateDto.FAILED,
                    TurnRecoveryStateDto.CANCELLED,
                )
                // Equal terminal snapshots received before the Attach boundary
                // are repeats, not the later Resume confirmation.
                if (interest?.attaching == true && terminal && interest.terminalSnapshot?.let {
                        it.revision == snapshot.revision && it.state == snapshot.state
                    } == true
                ) return
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
                    (durableTurnId != recoveredId || inactiveWaitingTurnId == recoveredId) &&
                    firstAttachSnapshot
                if (startsRecoveredAttach) {
                    turnToken++
                    recoveredTurnToken = turnToken
                    recoveryReplayTurnId = recoveredId
                    if (durableTurnId != recoveredId) durableTurnUiSequence = 0L
                    pendingRecoveredReplayAckTurnId = null
                    currentTurnOrigin = ConversationTurnOrigin.Ordinary
                } else if (
                    recoveredTurnToken != null &&
                    recoveryReplayTurnId == recoveredId &&
                    interest?.attaching != true
                ) {
                    // AttachTurn has returned and ResumeTurn has now emitted its
                    // own state. Subsequent sequenced envelopes are live copies;
                    // the following raw event is what the reducer should render.
                    recoveryReplayTurnId = null
                }
                durableTurnId = recoveredId

                if (interest != null && !interest.attaching && interest.expectsResume) {
                    durableAttachInterest = null
                }
                val hasAuthoritativeRestoredTranscript =
                    restoredTranscriptHasContent && restoredTranscriptSessionId == canonicalSessionId(snapshot.sessionId)
                val isInactiveRecoveredCheckpoint = recoveredTurnToken != null &&
                    turnJob == null &&
                    !_state.value.streaming
                // On the first attach snapshot, terminal history is replayed
                // immediately afterwards. Defer settling until ResumeTurn emits
                // the authoritative terminal state a second time.
                if (startsRecoveredAttach && terminal) {
                    interest?.terminalSnapshot = snapshot
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
                } else if (
                    canonicalSessionId(event.sessionId) == canonicalSessionId(_state.value.session.id) &&
                    pendingRecoveredReplayAckTurnId == event.turnId.toLong()
                ) {
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
                    reply !is ReplyEvent.CommandOutput &&
                    durableTurnId != null
                ) {
                    pendingRecoveredReplayAckTurnId = durableTurnId
                }
                if (reply is ReplyEvent.End || reply is ReplyEvent.Error || reply is ReplyEvent.CommandOutput) {
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

    /** Rows of the followed session's user prompts, by the turn the desktop assigned. */
    private val externalPromptRows = mutableMapOf<Long, String>()

    /** Provisional rows of prompts this device sent, by outgoing prompt id. */
    private val externalOutgoingRows = mutableMapOf<String, String>()
    private val externalFailedPrompts = mutableSetOf<String>()
    private val externalRetryPrompts = mutableMapOf<String, ExternalOutgoingPrompt>()
    private val _externalSendFailures = MutableSharedFlow<ExternalSendFailure>(extraBufferCapacity = 4)

    /** Failed drafts to restore in the composer, retaining attachments for an idempotent retry. */
    internal val externalSendFailures = _externalSendFailures.asSharedFlow()

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
    internal fun ingestExternal(item: ExternalFeedItem) {
        when (item) {
            is ExternalFeedItem.Base -> {
                externalPromptRows.clear()
                externalOutgoingRows.clear()
                applyActivatedSession(item.session)
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
    }

    /** A turn of the followed session started; idle means nothing else owns the reducer. */
    private fun beginExternalTurn() {
        if (turnJob != null || recoveredTurnToken != null) return
        turnToken++
        recoveredTurnToken = turnToken
        recoveryReplayTurnId = null
        durableTurnUiSequence = 0L
        pendingRecoveredReplayAckTurnId = null
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
                _externalSendFailures.tryEmit(ExternalSendFailure(prompt.text, prompt.images))
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
    private fun sendExternal(text: String, images: List<ImageRefDto>, external: ExternalTurnConversationSource) {
        if (!_state.value.sessionReady) return
        savedState?.set(KEY_DRAFT, "")
        _state.update { it.copy(isNew = false, error = null) }
        val retry = externalRetryPrompts.values.firstOrNull { it.text == text && it.images == images }
        viewModelScope.launch {
            try {
                if (retry != null) external.retryExternalPrompt(retry.id)
                else external.sendExternalPrompt(text, images)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Throwable) {
                _externalSendFailures.tryEmit(ExternalSendFailure(text, images))
                reportHostError(error.message ?: error::class.simpleName.orEmpty())
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
        val uncertainAdmission = pendingAdmissionRecovery?.turnId == failure.turnId
        durableAttachInterest = null
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
                recoveryRetryAvailable = uncertainAdmission,
                statusLine = if (uncertainAdmission) null else strings.resolve(
                    R.string.chat_background_paused_text,
                    // The fallback must be the resource's OWN zh-Hans copy —
                    // `DefaultConversationStrings` returns it verbatim, so a
                    // drifted fallback makes every JVM test assert copy the
                    // device never renders.
                    "后台时间已结束，打开对话即可安全恢复",
                ),
                error = if (uncertainAdmission) ChatError(strings.resolve(R.string.chat_recovery_unknown,
                    "暂时无法确认操作状态，请重试。"), ChatErrorKind.GENERIC) else ChatError(
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

    private fun ownsDurableDelivery(sessionId: String, turnId: ULong): Boolean {
        if (turnId > Long.MAX_VALUE.toULong()) return false
        val session = canonicalSessionId(sessionId)
        val visible = canonicalSessionId(_state.value.session.id)
        val confirmed = source.activeSessionState.value?.sessionId?.let(::canonicalSessionId)
        if (session != visible && !(visible == "new" && session == confirmed)) return false
        val owned = durableTurnId ?: inactiveWaitingTurnId ?: liveWaitingTurnId
        val interest = durableAttachInterest
        if (owned != null) return owned == turnId.toLong()
        return interest != null && interest.sessionId == session && interest.turnId == turnId.toLong()
    }

    private fun isVisibleSession(sessionId: String): Boolean {
        val session = canonicalSessionId(sessionId)
        val visible = canonicalSessionId(_state.value.session.id)
        if (session == visible) return true
        return visible == "new" && !_state.value.sessionTransitioning &&
            source.activeSessionState.value?.sessionId?.let(::canonicalSessionId) == session
    }

    private fun bindVerifiedInitialSession(sessionId: String) {
        if (_state.value.session.id != "new" || !isVisibleSession(sessionId)) return
        _state.update { it.copy(session = it.session.copy(id = sessionId), sessionReady = true) }
    }

    private fun markDurableReplayIncomplete() {
        durableReplayDegraded = true
        pendingDurableEvents.clear()
        showDurableRecoveryWarning()
    }

    private fun showDurableRecoveryWarning() {
        val message = strings.resolve(R.string.chat_recovery_replay_gap, "对话恢复不完整，请重新打开对话。")
        showDurableRecoveryWarning(message)
    }

    private fun showDurableRecoveryWarning(message: String) {
        _state.update { state ->
            if (state.error == null) state.copy(error = ChatError(message, ChatErrorKind.GENERIC))
            else state.copy(statusLine = message)
        }
    }

    private fun queryQueuedRecoveries(owner: ConversationSource, generation: Int, session: String,
        turns: List<Long>, visibleUuids: Set<String>) {
        queuedStatusJob?.cancel()
        queuedStatusRequests.clear()
        queuedStatusJob = viewModelScope.launch {
            for (batch in turns.distinct().chunked(32)) {
                if (source !== owner || sourceGeneration != generation || !isVisibleSession(session)) return@launch
                coroutineScope {
                    val requests = batch.map { turn ->
                        val id = java.util.UUID.randomUUID().toString()
                        val request = QueuedStatusRequest(owner, session, turn, generation, visibleUuids)
                        queuedStatusRequests[id] = request
                        id to request
                    }
                    val submissions = requests.map { (id, request) -> async {
                        runCatching { owner.queryQueuedRecovery(session, request.turnId, id) }.onFailure {
                            if (queuedStatusRequests[id] === request) queuedStatusRequests.remove(id)
                            request.completed.complete(Unit)
                        }
                    } }
                    withTimeoutOrNull(10_000) { requests.map { it.second.completed }.awaitAll() }
                    submissions.forEach { it.cancel() }
                    requests.forEach { (id, request) ->
                        if (queuedStatusRequests[id] === request) queuedStatusRequests.remove(id)
                    }
                }
            }
        }
    }

    private fun handleQueuedStatus(event: ClientEvent.QueuedTurnStatus) {
        val request = queuedStatusRequests[event.requestId] ?: return
        if (source !== request.source || sourceGeneration != request.generation || !isVisibleSession(request.sessionId) ||
            canonicalSessionId(event.sessionId) != request.sessionId || event.turnId != request.turnId.toULong()
        ) return
        queuedStatusRequests.remove(event.requestId)
        request.completed.complete(Unit)
        val resolution: QueuedRecoveryResolution
        val turns = when (val status = event.status) {
            is QueuedTurnStatusDto.Delivered -> {
                resolution = QueuedRecoveryResolution.Delivered
                confirmedQueuedMembers(event.turnId, status, request.visibleUuids, _state.value.messages).filter {
                    queuedTurnInterests[it]?.let { interest ->
                        interest.generation == request.generation && interest.sessionId == request.sessionId
                    } == true
                }
            }
            is QueuedTurnStatusDto.Cancelled -> { resolution = QueuedRecoveryResolution.CancelledBeforeStart; listOf(request.turnId) }
            is QueuedTurnStatusDto.Terminal -> {
                if (status.state !in setOf(TurnRecoveryStateDto.COMPLETED, TurnRecoveryStateDto.FAILED, TurnRecoveryStateDto.CANCELLED)) return
                resolution = QueuedRecoveryResolution.Terminal
                listOf(request.turnId)
            }
            else -> return
        }
        if (turns.isEmpty()) return
        viewModelScope.launch {
            if (source === request.source && sourceGeneration == request.generation && isVisibleSession(request.sessionId)) {
                if (!request.source.resolveQueuedRecovery(request.sessionId, turns, resolution)) {
                    showDurableRecoveryWarning(strings.resolve(R.string.chat_recovery_unknown, "暂时无法确认操作状态，请重试。"))
                }
            }
        }
    }

    internal fun reduceDurableTurnDelivery(delivery: ConversationDelivery.TurnEvent) {
        if (!delivery.isActive()) return
        val attach = durableAttachInterest
        if (delivery.excludedAttachRequestId != null && attach?.requestId == delivery.excludedAttachRequestId) return
        if (attach?.attaching == true && attach.requestId != null &&
            attach.sessionId == canonicalSessionId(delivery.sessionId) && attach.turnId == delivery.turnId.toLong() &&
            delivery.attachRequestId != attach.requestId
        ) return
        if (delivery.attachRequestId != null && attach?.requestId != delivery.attachRequestId &&
            !(ownsDurableDelivery(delivery.sessionId, delivery.turnId) && attach?.attaching != true &&
                (turnJob != null || recoveredTurnToken != null) && _state.value.streaming)
        ) return
        val cancellation = cancellingProjection
        if (cancellation != null && cancellation.generation == sourceGeneration &&
            cancellation.sessionId == canonicalSessionId(delivery.sessionId) && cancellation.turnId == delivery.turnId.toLong()
        ) {
            if (delivery.sequence == null) {
                if (cancellation.terminalObserved && delivery.event is ClientEvent.ToolUseResult) {
                    showDurableRecoveryWarning(strings.resolve(R.string.chat_recovery_projection_uncertain,
                        "部分运行内容可能未恢复，请检查后继续。"))
                } else if (!cancellation.terminalObserved) markDurableReplayIncomplete()
            }
            if (delivery.event is ClientEvent.ToolUseResult) {
                val reply = clientEventToReply(delivery.event, strings)
                if (reply is ReplyEvent.ToolActivity || reply is ReplyEvent.ShellTool) {
                    reduce(reply, turnToken)
                    _state.update { it.copy(streaming = false, statusLine = strings.resolve(R.string.chat_stopping, "正在停止…")) }
                }
            }
            return
        }
        if (durableTurnId == null && matchesQueuedTurn(delivery.sessionId, delivery.turnId) &&
            delivery.event is ClientEvent.TurnStarted &&
            (delivery.event.turnId == null || delivery.event.turnId == delivery.turnId) &&
            (delivery.sequence == null || delivery.sequence == 1uL)
        ) adoptQueuedTurn(delivery.sessionId, delivery.turnId)
        if (!ownsDurableDelivery(delivery.sessionId, delivery.turnId)) return
        // A failed attempt has no projected live owner. Its retry must begin
        // at the last actually consumed cursor, even while another UI keeps
        // receiving this source's live deliveries.
        if (inactiveWaitingTurnId == delivery.turnId.toLong() && recoveredTurnToken == null &&
            durableAttachInterest?.attaching != true
        ) return
        bindVerifiedInitialSession(delivery.sessionId)
        val attaching = durableAttachInterest?.let {
            it.attaching && it.turnId == delivery.turnId.toLong() && it.sessionId == canonicalSessionId(delivery.sessionId)
        } == true
        val sequence = delivery.sequence
        if (sequence == null) {
            markDurableReplayIncomplete()
            projectDurableTurnEvent(delivery, retained = false)
            return
        }
        if (durableReplayDegraded) {
            if (!attaching) projectDurableTurnEvent(delivery, retained = false)
            return
        }
        if (sequence > Long.MAX_VALUE.toULong()) {
            markDurableReplayIncomplete()
            return
        }
        val projected = durableTurnUiSequence.toULong()
        if (sequence <= projected || pendingDurableEvents.containsKey(sequence)) return
        if (sequence != projected + 1uL) {
            if (pendingDurableEvents.size >= 512) markDurableReplayIncomplete()
            else pendingDurableEvents[sequence] = delivery
            return
        }
        var nextSequence = sequence
        var next = delivery
        while (true) {
            if (!projectDurableTurnEvent(next, retained = attaching)) return
            durableTurnUiSequence = nextSequence.toLong()
            if (nextSequence == Long.MAX_VALUE.toULong()) return
            val buffered = pendingDurableEvents.remove(nextSequence + 1uL) ?: return
            nextSequence += 1uL
            next = buffered
        }
    }

    private fun projectDurableTurnEvent(delivery: ConversationDelivery.TurnEvent, retained: Boolean): Boolean {
        val event = delivery.event
        if (retained && event is ClientEvent.TurnStarted) return true
        if (retained && (event is ClientEvent.TurnEnded || event is ClientEvent.Error)) {
            markDurableReplayIncomplete()
            return false
        }
        if (retained && restoredTranscriptHasContent &&
            restoredTranscriptSessionId == canonicalSessionId(delivery.sessionId)
        ) {
            markDurableReplayIncomplete()
            return false
        }
        val reply = clientEventToReply(event, strings)
        val sessionOnly = when (event) {
            is ClientEvent.UiLog -> {
                _state.update { it.copy(statusLine = "${event.plugin}: ${event.text.displayText()}") }
                true
            }
            is ClientEvent.UiToast -> {
                _state.update { it.copy(statusLine = "${event.plugin}: ${event.text.displayText()}") }
                true
            }
            is ClientEvent.UiStatus -> {
                _state.update { it.copy(statusLine = event.text?.displayText()) }
                true
            }
            is ClientEvent.Attachment -> when (val attachment = event.attachment) {
                is AttachmentDto.NestedMemory -> {
                    _state.update { it.copy(statusLine = attachment.displayPath) }
                    true
                }
                else -> false
            }
            is ClientEvent.CoordinatorWorker -> {
                val worker = event.worker
                _state.update { state ->
                    val entry = SessionAgentUi(worker.agentId, worker.name, worker.agentType, null, worker.status, null)
                    state.copy(sessionAgents = state.sessionAgents.filterNot { it.agentId == worker.agentId } + entry)
                }
                true
            }
            else -> false
        }
        val auxiliary = event is ClientEvent.AskUserQuestion || event is ClientEvent.AskUserQuestionResolved ||
            event is ClientEvent.PermissionRequestResolved || event is ClientEvent.PlanUpdated || sessionOnly
        if (reply == null && !auxiliary) {
            markDurableReplayIncomplete()
            return false
        }
        val liveSubmission = recoveredTurnToken == null && durableTurnId == delivery.turnId.toLong() &&
            (turnJob != null || _state.value.streaming)
        if (liveSubmission && event is ClientEvent.TextDelta && !_state.value.streaming) {
            markDurableReplayIncomplete()
            return false
        }
        val token = turnToken
        reduceClientEvent(event)
        if (liveSubmission && reply != null) reduce(reply, token)
        return true
    }

    internal fun noteDurableAttach(sessionId: String, turnId: Long, expectsResume: Boolean, requestId: String? = null) {
        val session = canonicalSessionId(sessionId)
        if (session != canonicalSessionId(_state.value.session.id)) return
        durableAttachInterest = DurableAttachInterest(session, turnId, expectsResume, requestId?.takeIf(String::isNotBlank))
    }

    private fun matchesQueuedTurn(sessionId: String, turnId: ULong): Boolean {
        val interest = queuedTurnInterests[turnId.toLong()] ?: return false
        val session = canonicalSessionId(sessionId)
        return interest.generation == sourceGeneration && interest.sessionId == session && isVisibleSession(session)
    }

    private fun adoptQueuedTurn(sessionId: String, turnId: ULong) {
        if (durableTurnId != null || !matchesQueuedTurn(sessionId, turnId)) return
        queuedTurnInterests.remove(turnId.toLong())
        turnToken++
        durableTurnId = turnId.toLong()
        recoveredTurnToken = turnToken
        recoveryReplayTurnId = null
        durableTurnUiSequence = 0L
        durableReplayDegraded = false
        pendingDurableEvents.clear()
        currentTurnOrigin = ConversationTurnOrigin.Ordinary
        _state.update {
            it.copy(streaming = true, durableRecoveryBlocked = false, liveTurnWaitingForUser = false,
                statusLine = null, agentRun = AgentRunState(turnId = turnToken))
        }
        reportBackgroundTurnState()
    }

    private fun finishCancelledProjection(delivery: ConversationDelivery.CancellationDrained) {
        val cancellation = cancellingProjection ?: return
        if (cancellation.generation != sourceGeneration || cancellation.turnId != delivery.turnId ||
            cancellation.sessionId != canonicalSessionId(delivery.sessionId)
        ) return
        cancellingProjection = null
        if (!delivery.terminalObserved) {
            cancellation.finished.complete(false)
            preserveUncertainAdmission(source, delivery.sessionId, delivery.turnId, cancellation.generation)
            return
        }
        clearDurableRecoveryAfterTerminal(delivery.turnId, hadMatchingDurableIdentity = true)
        _state.update { state ->
            val settled = state.settleTurn(state.agentRun?.finish(AgentRunOutcome.Cancelled),
                state.streamingMessage?.withoutPendingVisualizations())
            state.copy(streaming = false, cancellationInFlight = false, statusLine = null,
                messages = settled.messages, streamingMessage = null, agentRun = settled.run,
                agentRunsByMessageId = settled.agentRunsByMessageId)
        }
        reportBackgroundTurnState()
        cancellation.finished.complete(true)
    }

    private fun preserveUncertainAdmission(owner: ConversationSource, sessionId: String, turnId: Long, generation: Int) {
        if (source !== owner || sourceGeneration != generation || !isVisibleSession(sessionId)) return
        if (_state.value.recoveryStorageFailed) return
        pendingAdmissionRecovery = PendingAdmissionRecovery(owner, canonicalSessionId(sessionId), turnId, generation)
        turnJob = null
        durableTurnId = turnId
        inactiveWaitingTurnId = turnId
        recoveredTurnToken = null
        recoveryReplayTurnId = null
        _state.update { it.copy(streaming = false, cancellationInFlight = false, durableRecoveryBlocked = true,
            recoveryRetryAvailable = true, statusLine = null,
            error = ChatError(strings.resolve(R.string.chat_recovery_unknown,
                "暂时无法确认操作状态，请重试。"), ChatErrorKind.GENERIC)) }
        retryDurableRecovery()
    }

    private fun markRecoveryStorageFailed() {
        _state.update { it.copy(recoveryStorageFailed = true, durableRecoveryBlocked = true,
            recoveryRetryAvailable = false, statusLine = null,
            error = ChatError(strings.resolve(R.string.chat_recovery_storage_failed,
                "无法保存对话恢复状态，请重新启动应用后检查对话。"), ChatErrorKind.GENERIC)) }
    }

    /** Retry a bounded, non-resuming query for the original owner. */
    fun retryDurableRecovery() {
        val pending = pendingAdmissionRecovery ?: return
        if (admissionRecoveryJob?.isActive == true || source !== pending.source ||
            sourceGeneration != pending.generation || !isVisibleSession(pending.sessionId)) return
        _state.update { it.copy(recoveryRetryAvailable = false,
            statusLine = strings.resolve(R.string.chat_status_resuming_session, "正在恢复会话…")) }
        admissionRecoveryJob = viewModelScope.launch {
            var began = false
            try {
                withTimeout(25_000) {
                    pending.source.reconcileDurableTurnForUi(pending.sessionId, pending.turnId, durableTurnUiSequence, pending.trustNotFound) {
                            session, turn, expectsResume, request ->
                        began = true
                        if (source === pending.source && sourceGeneration == pending.generation) {
                            noteDurableAttach(session, turn, expectsResume, request)
                        }
                    }
                }
                check(began) { "Recovery query was not started" }
                if (pendingAdmissionRecovery === pending) {
                    pendingAdmissionRecovery = null
                    _state.update { it.copy(recoveryRetryAvailable = false, statusLine = null) }
                }
            } catch (cancelled: CancellationException) {
                if (!kotlinx.coroutines.currentCoroutineContext().isActive) throw cancelled
                if (pendingAdmissionRecovery === pending) _state.update { it.copy(recoveryRetryAvailable = true, statusLine = null) }
            } catch (_: Throwable) {
                if (pendingAdmissionRecovery === pending) _state.update { it.copy(recoveryRetryAvailable = true, statusLine = null,
                    durableRecoveryBlocked = true,
                    error = ChatError(strings.resolve(R.string.chat_recovery_unknown,
                        "暂时无法确认操作状态，请重试。"), ChatErrorKind.GENERIC)) }
            }
        }
    }

    fun acknowledgeTerminalRecovery() {
        val review = terminalRecoveryReview ?: return
        if (source !== review.source || sourceGeneration != review.generation || !isVisibleSession(review.sessionId)) return
        viewModelScope.launch {
            val saved = runCatching { review.source.acknowledgeTerminalRecovery(review.sessionId, review.turns) }.getOrDefault(false)
            if (source !== review.source || sourceGeneration != review.generation || !isVisibleSession(review.sessionId)) return@launch
            if (!saved) {
                reportHostError(strings.resolve(R.string.chat_recovery_unknown, "暂时无法确认操作状态，请重试。"))
                return@launch
            }
            val remaining = review.source.terminalRecoveryReceipts(review.sessionId)
            val cancelled = review.source.cancelledQueuedReceipts(review.sessionId).toSet()
            if (source !== review.source || sourceGeneration != review.generation || !isVisibleSession(review.sessionId)) return@launch
            terminalRecoveryReview = if (remaining.isEmpty()) null else review.copy(turns = remaining)
            _state.update { it.copy(terminalRecoveryReviewAvailable = remaining.isNotEmpty(),
                terminalRecoveryUncertain = remaining.any { turn -> turn !in cancelled },
                queuedCancellationNotice = remaining.any { turn -> turn in cancelled }) }
        }
    }

    private fun abortCancelledProjection(delivery: ConversationDelivery.CancellationAborted) {
        val cancellation = cancellingProjection ?: return
        if (cancellation.generation != sourceGeneration || cancellation.turnId != delivery.turnId ||
            cancellation.sessionId != canonicalSessionId(delivery.sessionId)
        ) return
        if (delivery.terminalObserved) {
            cancellation.terminalObserved = true
            return // The source will publish the final boundary after TurnEnded.
        }
        cancellingProjection = null
        recoveredTurnToken = turnToken
        _state.update { it.copy(streaming = true, cancellationInFlight = false, statusLine = null) }
        cancellation.finished.complete(false)
    }

    internal fun finishDurableAttachReplay(delivery: ConversationDelivery.AttachReplayEnded): Boolean? {
        if (!delivery.isActive()) return false
        val interest = durableAttachInterest ?: return null
        if (interest.sessionId != canonicalSessionId(delivery.sessionId) ||
            interest.turnId != delivery.turnId || interest.expectsResume != delivery.expectsResume ||
            interest.requestId != delivery.requestId || !interest.attaching
        ) return null
        if (pendingDurableEvents.isNotEmpty()) markDurableReplayIncomplete()
        interest.attaching = false
        if (!durableReplayDegraded && !interest.expectsResume &&
            pendingAdmissionRecovery?.let { it.sessionId == interest.sessionId && it.turnId == interest.turnId } == true
        ) {
            pendingAdmissionRecovery = null
            _state.update { it.copy(recoveryRetryAvailable = false) }
        }
        if (recoveryReplayTurnId == interest.turnId) recoveryReplayTurnId = null
        if (!interest.expectsResume) {
            durableAttachInterest = null
            interest.terminalSnapshot?.let { reduceClientEvent(ClientEvent.TurnRecoveryState(it)) }
        }
        return !durableReplayDegraded
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
        if (pendingAdmissionRecovery?.turnId == recoveredId) pendingAdmissionRecovery = null
        if (durableAttachInterest?.turnId == recoveredId) durableAttachInterest = null
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
                recoveryRetryAvailable = false,
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
        val previous = workflowStatusesBySession[origin]?.get(taskId)
        val effectiveStatus = if (previous == TaskStatusDto.EXITED_UNKNOWN) TaskStatusDto.EXITED_UNKNOWN else status
        workflowStatusesBySession[origin] =
            workflowStatusesBySession[origin].orEmpty() + (taskId to effectiveStatus)
        val sessionRuns = workflowRunsBySession[origin] ?: return
        val run = sessionRuns[taskId] ?: return
        val nextRuns = sessionRuns + (taskId to run.copy(status = effectiveStatus))
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
        val discardSource = source
        val discardSession = _state.value.session.id
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
            runCatching { discardSource.discardDurableTurnForSession(discardSession, turnId) }
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
     * The host can accept a discard without emitting a terminal snapshot.
     * Bound the wait while keeping ownership until an authoritative query or
     * terminal callback confirms the result, then allow the user to retry.
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
            durableDiscardInFlightTurnId = null
            _state.update {
                it.copy(
                    statusLine = null,
                    error = ChatError(
                        strings.resolve(
                            R.string.chat_error_discard_unconfirmed,
                            "暂时无法确认后台对话已停止，请重试。",
                        ),
                        ChatErrorKind.GENERIC,
                    ),
                )
            }
            val owner = source
            val session = _state.value.session.id
            if (owner.supportsSubmissionAdmission && session != "new" && session.isNotBlank()) {
                preserveUncertainAdmission(owner, session, turnId, sourceGeneration)
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
        createSource: () -> ConversationSource,
    ) {
        if (generation <= reconnectGeneration || generation <= (requestedSourceGeneration ?: Int.MIN_VALUE)) return
        if (sideChats.hasLiveChats) {
            reportHostError(strings.resolve(R.string.side_chat_end_before_reconnect, "请先结束旁聊，再重新连接引擎。"))
            sideChats.whenVacant = { ensureSourceInBackground(generation, dispatcher, createSource) }
            return
        }
        requestedSourceGeneration = generation
        val expectedSource = source
        viewModelScope.launch {
            workspaceSwitchMutex.withLock {
                var prepared: ConversationSource? = null
                try {
                    // UniFFI construction is synchronous and cannot be interrupted.
                    // Retain the returned handle even after cancellation so it can
                    // be closed rather than lost at withContext's dispatch boundary.
                    withContext(dispatcher + NonCancellable) { prepared = createSource() }
                    coroutineContext.ensureActive()
                    if (requestedSourceGeneration != generation || generation <= reconnectGeneration || source !== expectedSource) return@launch
                    if (sideChats.hasLiveChats) {
                        sideChats.whenVacant = { ensureSourceInBackground(generation, dispatcher, createSource) }
                        return@launch
                    }
                    val replacement = checkNotNull(prepared)
                    ensureSource(generation) { replacement }
                    prepared = null
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (failure: Exception) {
                    if (requestedSourceGeneration == generation && source === expectedSource) ensureSource(generation) {
                        UnavailableConversationSource(failure.message ?: "Engine initialization failed", strings)
                    }
                } finally {
                    val abandoned = prepared
                    if (abandoned != null && abandoned !== source) withContext(dispatcher + NonCancellable) { abandoned.close() }
                    if (requestedSourceGeneration == generation) requestedSourceGeneration = null
                }
            }
        }
    }

    internal fun ensureSource(
        generation: Int,
        createSource: () -> ConversationSource,
    ) {
        if (generation <= reconnectGeneration) return
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
        val pendingBeforeReconnect = pendingAdmissionRecovery
        pendingAdmissionRecovery = null
        admissionRecoveryJob?.cancel()
        admissionRecoveryJob = null
        reconnectGeneration = generation
        sourceGeneration++
        queuedTurnInterests.clear()
        cancellingProjection?.finished?.complete(false)
        cancellingProjection = null

        abandonLocalTurn()?.cancel()
        explicitCancellation?.cancel()
        explicitCancellation = null
        _state.update { it.copy(cancellationInFlight = false) }
        sessionTransitionJob?.cancel()
        sessionTransitionJob = null
        sourceBindingJob?.cancel()

        sideChats.release()
        source = replacement
        if (pendingBeforeReconnect != null) {
            pendingAdmissionRecovery = pendingBeforeReconnect.copy(source = replacement, generation = sourceGeneration,
                requiresResumeActivation = true, trustNotFound = false)
            durableTurnId = null
            inactiveWaitingTurnId = null
        }
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
                exitedUnknownTaskIds = emptySet(),
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
        if (pendingAdmissionRecovery != null) {
            _state.update { it.copy(recoveryRetryAvailable = admissionRecoveryJob?.isActive != true) }
            return@withLock false
        }
        if (sideChats.hasLiveChats) {
            reportHostError(strings.resolve(R.string.side_chat_end_before_workspace, "请先结束旁聊，再切换工作区。"))
            return@withLock false
        }
        if (!allowInactiveWaitingRecovery && refuseWhileDurableTurnParked()) return@withLock false
        val previous = source
        val previousGeneration = sourceGeneration
        suspend fun replacementIsBlocked(): Boolean {
            // A failed preference commit can precede delivery of its UI marker.
            // Recheck the owning store as well as local state after every await.
            val healthy = try {
                previous.recoveryStorageAvailable()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Throwable) {
                return true
            }
            if (!healthy && source === previous && sourceGeneration == previousGeneration) markRecoveryStorageFailed()
            return !healthy || pendingAdmissionRecovery != null || _state.value.recoveryStorageFailed ||
                liveWaitingTurnId != null || (_state.value.durableRecoveryBlocked &&
                    (!allowInactiveWaitingRecovery || inactiveWaitingTurnId == null)) ||
                (!allowInactiveWaitingRecovery && inactiveWaitingTurnId != null) ||
                sideChats.hasLiveChats || source !== previous || sourceGeneration != previousGeneration ||
                _state.value.blocksWorkspaceReplacement(
                hasPendingPermission = _pendingPermission.value != null,
                replacePendingTransition = replacePendingTransition,
            )
        }
        fun reportBlockedReplacement() {
            if (pendingAdmissionRecovery != null) {
                _state.update { it.copy(recoveryRetryAvailable = admissionRecoveryJob?.isActive != true) }
                return
            }
            if (_state.value.recoveryStorageFailed) return
            if (!allowInactiveWaitingRecovery && refuseWhileDurableTurnParked()) return
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
        queuedTurnInterests.clear()
        cancellingProjection?.finished?.complete(false)
        cancellingProjection = null
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
                exitedUnknownTaskIds = emptySet(),
                backgroundTasks = emptyMap(),
                sessionAgents = emptyList(),
                workflowResumeState = WorkflowResumeUiState.Idle,
                workflowRuns = emptyMap(),
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
    internal fun applyActivatedSession(restored: ActivatedSession) {
        val current = _state.value
        if (current.sessionTransitioning) {
            val matchesPendingResume =
                pendingResumeId != null &&
                    restored.kind == SessionActivationKind.Resumed &&
                    restored.sessionId == pendingResumeId
            val matchesPendingNew =
                pendingNewSession && restored.kind == SessionActivationKind.Started
            if (!matchesPendingResume && !matchesPendingNew) return
        }
        // A startup listing only binds an unowned conversation. In particular,
        // it cannot confirm a pending NewSession/Resume or replace an owner.
        if (restored.kind == SessionActivationKind.Bootstrap && current.session.id != "new") return
        if ((restored.kind == SessionActivationKind.Started || restored.kind == SessionActivationKind.Bootstrap)
            && !current.sessionTransitioning
        ) {
            // The first prompt can already be streaming when engine bootstrap
            // announces its owner. Bind that identity without abandoning the
            // collector or clearing the optimistic prompt and widget slots.
            if (current.session.id == "new") {
                _state.update {
                    val catalogTitle = _sessions.value.rows.firstOrNull { row ->
                        canonicalSessionId(row.uuid) == canonicalSessionId(restored.sessionId)
                    }?.title
                    it.copy(session = it.session.copy(id = restored.sessionId, title = catalogTitle ?: it.session.title), sessionReady = true)
                }
                reportBackgroundTurnState()
                return
            }
            // Repeated/bootstrap announcements cannot replace a transcript
            // already owned by a confirmed session. Explicit NewSession still
            // follows the transition path above.
            if (canonicalSessionId(current.session.id) == canonicalSessionId(restored.sessionId) ||
                current.messages.isNotEmpty() || current.streamingMessage != null
            ) return
        }
        abandonLocalTurn()
        queuedTurnInterests.clear()
        cancellingProjection?.finished?.complete(false)
        cancellingProjection = null
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
        restoredTranscriptHasContent = restored.kind == SessionActivationKind.Resumed && restored.transcript.isNotEmpty()
        val title = _sessions.value.rows.firstOrNull { canonicalSessionId(it.uuid) == canonicalSessionId(restored.sessionId) }?.title
            ?: _state.value.session.takeIf { canonicalSessionId(it.id) == canonicalSessionId(restored.sessionId) }?.title
            ?: _state.value.session.title
        _state.update {
            it.copy(
                session = SessionRef(id = restored.sessionId, title = title),
                promptSuggestion = null, promptSuggestionUuid = null,
                messages = restored.transcript, // clear-then-restore (oldest-first)
                visualizationChip = null,
                visualizationFollowup = null,
                streamingMessage = null,
                isNew = restored.kind == SessionActivationKind.Started && restored.transcript.isEmpty(),
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
                exitedUnknownTaskIds = emptySet(),
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
                    workspace != null && canonicalSessionId(workspace.sessionId) == canonicalSessionId(current.session.id)) {
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
        if (!onAnnouncedSession && pendingAdmissionRecovery != null) {
            _state.update { it.copy(recoveryRetryAvailable = admissionRecoveryJob?.isActive != true) }
            return false
        }
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
        durableAttachInterest = null
        durableReplayDegraded = false
        pendingDurableEvents.clear()
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
    private suspend fun cancelEngineTurn(
        job: Job?, turnId: Long? = null, sessionId: String? = null,
        targetSource: ConversationSource = source,
    ) {
        if (job == null && turnId == null) return
        try {
            if (sessionId != null) targetSource.cancelForSession(sessionId, turnId) else targetSource.cancel(turnId)
        } finally {
            job?.cancelAndJoin()
        }
    }

    private data class CancellationSubmission(val sent: Boolean, val uncertainAdmission: Boolean)

    /** The absence of a terminal callback is not proof of successful cancellation. */
    private suspend fun cancelSubmittedTurn(
        targetSource: ConversationSource, job: Job?, turnId: Long?, sessionId: String?,
        submission: ActiveSubmission?,
    ): CancellationSubmission = withContext(NonCancellable) {
        val completion = submission?.admission?.cancelAndAwait()
        if (submission != null && (completion == null || completion.outcome == TurnSubmissionAdmission.Outcome.NotAdmitted)) {
            job?.cancelAndJoin()
            targetSource.abandonUnsubmittedTurn(submission.sessionId ?: sessionId, submission.turnId)
            CancellationSubmission(sent = false, uncertainAdmission = false)
        } else {
            cancelEngineTurn(job, turnId, completion?.sessionId ?: sessionId, targetSource)
            CancellationSubmission(sent = true, uncertainAdmission = completion?.outcome == TurnSubmissionAdmission.Outcome.Unknown)
        }
    }

    private suspend fun resolveSubmissionOwner(
        targetSource: ConversationSource, capturedSession: String?, sourceEpoch: Int, sessionEpoch: Long,
    ): String {
        if (!capturedSession.isNullOrBlank()) return capturedSession
        // A newly constructed source has not yet published its workspace
        // session. Resolve that initial owner only while the initiating action
        // still belongs to the same source and session transition.
        val initial = targetSource.submissionSession()?.takeIf(String::isNotBlank)
            ?: throw IllegalStateException("Cannot start a turn without a session owner")
        check(source === targetSource && sourceGeneration == sourceEpoch && sessionToken == sessionEpoch &&
            (_state.value.session.id == "new" || canonicalSessionId(_state.value.session.id) == canonicalSessionId(initial))
        ) { "Session changed before the turn was submitted" }
        return initial
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
        val pendingCancellation = cancellingProjection
        if (_state.value.cancellationInFlight && pendingCancellation != null) {
            val waitingToken = ++sessionToken
            viewModelScope.launch {
                if (pendingCancellation.finished.await() && waitingToken == sessionToken &&
                    pendingCancellation.generation == sourceGeneration
                ) beginSessionTransition(target, newSession, newSessionDirectory, resumeEmpty, status, allowInactiveWaitingRecovery)
            }
            return
        }
        if (!allowInactiveWaitingRecovery && refuseWhileDurableTurnParked()) return
        // Do not enqueue a second ambiguous Resume/New while the first command
        // is accepted but its SessionResumed/SessionStarted event is still in
        // flight. Those events do not carry a client operation id.
        if (_state.value.sessionTransitioning && sessionTransitionJob != null) return
        sessionToken++
        val token = sessionToken
        val previousState = _state.value
        terminalRecoveryReview = null
        queuedStatusRequests.clear()
        queuedStatusJob?.cancel()
        queuedStatusJob = null
        pendingResumeId = target.id.takeUnless { newSession }
        pendingNewSession = newSession
        val previousSession = _state.value.session.id.takeUnless { it == "new" }
            ?: source.activeSessionState.value?.sessionId
        val previousTurn = durableTurnId
        val previousSource = source
        val previousSubmission = activeSubmission?.takeIf { it.source === previousSource && it.turnId == previousTurn }
        val transitionWaiter = if (previousSession != null && previousTurn != null) {
            TransitionCancellation(canonicalSessionId(previousSession), previousTurn).also { transitionCancellation = it }
        } else null
        activeSubmission = null
        val detachedTurn = abandonLocalTurn()
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
                terminalRecoveryReviewAvailable = false,
                terminalRecoveryUncertain = false,
                queuedCancellationNotice = false,
                queuedRecoveryWarning = false,
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
                val cancellation = cancelSubmittedTurn(previousSource, detachedTurn, previousTurn, previousSession, previousSubmission)
                if (cancellation.uncertainAdmission) {
                    val confirmed = if (cancellation.sent && previousSource.orderedCancellationBoundaries && transitionWaiter != null) {
                        withTimeoutOrNull(10_000) { transitionWaiter.consumed.await() } == true
                    } else false
                    if (!confirmed && token == sessionToken && source === previousSource && previousTurn != null && previousSession != null) {
                        pendingResumeId = null
                        pendingNewSession = false
                        if (transitionCancellation === transitionWaiter) transitionCancellation = null
                        _state.value = previousState.copy(sessionTransitioning = false, sessionReady = true, streaming = false)
                        preserveUncertainAdmission(previousSource, previousSession, previousTurn, sourceGeneration)
                        return@launch
                    }
                }
                if (transitionCancellation === transitionWaiter) transitionCancellation = null
                when {
                    newSession && newSessionDirectory != null -> source.newSession(newSessionDirectory)
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

    /** The engine's ModelChanged event confirms the model shown in the chip. */
    fun selectModel(model: ModelOption) {
        if (model.id.isBlank()) return
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
        if (id == _state.value.reasoning?.selection) return
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
        newChat()
    }

    /** A widget drafted a follow-up: the screen moves it into the composer. */
    fun offerVisualizationFollowup(followup: VisualizationFollowup) {
        _state.update { it.copy(visualizationFollowup = followup) }
    }

    /** The composer took the offered follow-up; its chip now rides the next send. */
    fun acceptVisualizationFollowup(followup: VisualizationFollowup) {
        _state.update {
            if (it.visualizationFollowup != followup) it
            else it.copy(visualizationFollowup = null, visualizationChip = followup.chip)
        }
    }

    fun clearVisualizationChip() {
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
        if (source is ExternalTurnConversationSource) return true
        val rejection = sideChatSendRejection(text, hasImages, source is SideChatSource) ?: return true
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
    ) {
        val trimmed = text.trim()
        if (trimmed.isEmpty()) return
        if (trimmed.split(Regex("\\s+"), limit = 2).first().equals("/compact", ignoreCase = true) && _state.value.streaming) {
            showCompactRejection()
            return
        }
        (source as? ExternalTurnConversationSource)?.let { external ->
            // No side chat, no local turn: the desktop validates the prompt and
            // is the only place a turn starts.
            sendExternal(trimmed, images, external)
            return
        }
        val isBashInput = text.startsWith("!")
        val submittedText = if (isBashInput) text.substring(1) else trimmed
        val promptMode = if (isBashInput) PromptModeDto.BASH else null
        if (!admitComposerSend(trimmed, images.isNotEmpty())) return
        sideChatInput(trimmed)?.let { input ->
            val parent = source as? EngineConversationSource
            if (parent == null) {
                reportHostError(strings.resolve(R.string.side_chat_nested_unavailable, "旁聊中无法创建新的旁聊"))
                return
            }
            sideChats.create(input = input, draftOnFailure = text, newSource = parent::newSideChatSource) {
                onDraftChanged(text)
                reportHostError(sideChats.error.value ?: "Unable to create side chat")
            }
            return
        }
        if (refuseWhileDurableTurnParked()) return
        if (!_state.value.sessionReady || _state.value.sessionTransitioning) return
        if (_state.value.recoveryStorageFailed) return
        if (_state.value.cancellationInFlight) return
        if (explicitCancellation?.isActive == true) return
        if (_state.value.compaction?.status == CompactionProgressStatus.Running) return

        if (isManualCompactCommand(trimmed)) {
            if (images.isNotEmpty() || _state.value.streaming) return
            savedState?.set(KEY_DRAFT, "")
            _state.update {
                it.copy(
                    isNew = false,
                    statusLine = null,
                    error = null,
                    messages = it.messages + Message(role = Role.User, sentAtMs = System.currentTimeMillis(), text = trimmed),
                    compaction = CompactionProgressUi(
                        status = CompactionProgressStatus.Running,
                    ),
                )
            }
            viewModelScope.launch {
                runCatching { source.submitClientCommand(ClientCommand.ForceCompact) }
                    .onFailure { failure ->
                        if (isCompactTurnRejection(failure.message.orEmpty())) {
                            showCompactRejection()
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
        val actionSource = source
        val actionSourceGeneration = sourceGeneration
        val actionSessionToken = sessionToken
        val actionSession = (_state.value.session.id.takeUnless { it == "new" }
            ?: actionSource.activeSessionState.value?.sessionId)?.takeIf(String::isNotBlank)

        if (_state.value.streaming) {
            val queuedTurnId = DurableConversationTurnIds.next() ?: run {
                reportHostError(strings.resolve(R.string.chat_engine_unavailable, "移动端引擎当前不可用"))
                return
            }
            val queuedSource = actionSource
            val queuedGeneration = actionSourceGeneration
            var queuedSession = actionSession?.let(::canonicalSessionId).orEmpty()
            if (queuedSession.isNotBlank()) queuedTurnInterests[queuedTurnId] = QueuedTurnInterest(queuedSession, queuedGeneration)
            val queuedUserMessage = Message(role = Role.User, sentAtMs = System.currentTimeMillis(), text = trimmed, images = images, visualizationContext = visualizationChip)
            optimisticRowsByToken["client-turn:$queuedTurnId"] = queuedUserMessage.id
            savedState?.set(KEY_DRAFT, "")
            _state.update {
                it.copy(
                    isNew = false,
                    error = null,
                    visualizationChip = null,
                    messages = it.messages + queuedUserMessage,
                )
            }
            viewModelScope.launch {
                runCatching {
                    if (queuedSource.supportsSubmissionAdmission) {
                        queuedSession = canonicalSessionId(resolveSubmissionOwner(
                            queuedSource, actionSession, queuedGeneration, actionSessionToken,
                        ))
                        if (source === queuedSource && sourceGeneration == queuedGeneration && sessionToken == actionSessionToken) {
                            queuedTurnInterests[queuedTurnId] = QueuedTurnInterest(queuedSession, queuedGeneration)
                        }
                    }
                    queuedSource.submitClientCommand(
                        ClientCommand.SendPrompt(
                            sessionId = queuedSession.takeIf(String::isNotBlank),
                            text = submittedText.toClientTextDto(),
                            promptMode = promptMode,
                            images = images,
                            turnId = queuedTurnId.toULong(),
                            visualizationContext = visualizationChip?.reference?.toDto(),
                        ),
                    )
                }.onFailure { failure ->
                    if (failure is ClientException.SubmissionNotAdmitted) queuedTurnInterests.remove(queuedTurnId)
                    if (sourceGeneration != queuedGeneration || source !== queuedSource ||
                        (queuedSession.isNotBlank() && canonicalSessionId(_state.value.session.id) != queuedSession)
                    ) return@onFailure
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
        val submittedTurnId = DurableConversationTurnIds.next() ?: run {
            reportHostError(strings.resolve(R.string.chat_engine_unavailable, "移动端引擎当前不可用"))
            return
        }
        val userMessage = Message(role = Role.User, sentAtMs = System.currentTimeMillis(), text = trimmed, images = images, visualizationContext = visualizationChip)
        optimisticRowsByToken["client-turn:$submittedTurnId"] = userMessage.id
        durableTurnId = submittedTurnId
        recoveredTurnToken = null
        recoveryReplayTurnId = null
        durableTurnUiSequence = 0L
        durableReplayDegraded = false
        pendingDurableEvents.clear()
        pendingRecoveredReplayAckTurnId = null
        currentTurnOrigin = origin
        // The draft was just sent — clear the persisted copy.
        savedState?.set(KEY_DRAFT, "")
        _state.update {
            it.copy(
                isNew = false,
                statusLine = null,
                compaction = null,
                error = null, // a fresh turn clears the prior turn's error banner
                streaming = true, // gate the composer immediately, before the first event
                liveTurnWaitingForUser = false,
                messages = it.messages + userMessage,
                visualizationChip = null,
                streamingMessage = null,
                agentRun = AgentRunState(turnId = token),
            )
        }
        reportBackgroundTurnState()
        // Start while this user action still has a visible Activity. Android 12+
        // generally rejects foreground-service starts attempted only after the
        // process has already crossed into the background.
        setBackgroundTurnActive(true)

        val turnSource = actionSource
        val slashCommandSource = turnSource as? SlashCommandConversationSource
        val submission = if (turnSource.supportsSubmissionAdmission) {
            ActiveSubmission(turnSource, submittedTurnId, TurnSubmissionAdmission(), actionSession)
        } else null
        activeSubmission = submission
        turnJob = viewModelScope.launch {
            val replyStream = try {
                val owner = submission?.let {
                    resolveSubmissionOwner(turnSource, it.sessionId, actionSourceGeneration, actionSessionToken).also { resolved ->
                        it.sessionId = resolved
                        if (source === turnSource && sourceGeneration == actionSourceGeneration &&
                            sessionToken == actionSessionToken && _state.value.session.id == "new"
                        ) _state.update { state -> state.copy(session = state.session.copy(id = resolved), sessionReady = true) }
                    }
                }
                if (!isBashInput && isSlashCommandLine(text) && slashCommandSource != null) {
                    if (submission != null && owner != null) {
                        slashCommandSource.submitSlashCommand(text.toClientTextDto(), images, submittedTurnId, owner, submission.admission)
                    } else slashCommandSource.submitSlashCommand(text.toClientTextDto(), images, submittedTurnId)
                } else {
                    if (submission != null && owner != null) {
                        turnSource.submit(submittedText, images, submittedTurnId, visualizationChip?.reference, promptMode, owner, submission.admission)
                    } else turnSource.submit(submittedText, images, submittedTurnId, visualizationChip?.reference, promptMode)
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                reduce(ReplyEvent.Error(failure.message ?: failure.toString()), token)
                reduce(ReplyEvent.End, token)
                return@launch
            }
            var uncertainSubmission = false
            replyStream.collect { event ->
                if (uncertainSubmission) return@collect
                if (event is ReplyEvent.DurableTurnStreamEnded) return@collect
                if (event is ReplyEvent.Error && token == turnToken && submission != null) {
                    when (submission.admission.outcome) {
                        TurnSubmissionAdmission.Outcome.Unknown -> {
                            uncertainSubmission = true
                            submission.sessionId?.let { preserveUncertainAdmission(turnSource, it, submittedTurnId, actionSourceGeneration) }
                            return@collect
                        }
                        TurnSubmissionAdmission.Outcome.NotAdmitted -> {
                            if (durableTurnId == submittedTurnId) durableTurnId = null
                        }
                        else -> Unit
                    }
                }
                reduce(event, token)
                if (
                    event is ReplyEvent.DurableTurnReplayAcknowledged &&
                    token == turnToken &&
                    source === turnSource &&
                    durableTurnId == submittedTurnId &&
                    event.turnId == submittedTurnId &&
                    (canonicalSessionId(event.sessionId) == canonicalSessionId(_state.value.session.id) ||
                        (_state.value.session.id == "new" && turnSource.activeSessionState.value?.sessionId?.let {
                            canonicalSessionId(event.sessionId) == canonicalSessionId(it)
                        } == true))
                ) {
                    durableTurnUiSequence = maxOf(durableTurnUiSequence, event.sequence)
                }
            }
        }
        turnJob?.invokeOnCompletion { if (activeSubmission === submission) activeSubmission = null }
    }

    /**
     * Cancel the in-flight turn (or discard a recovered parked checkpoint) from
     * the composer's Stop affordance. A recovered WaitingForUser checkpoint is
     * not active execution, so it remains blocked until its correlated terminal
     * TurnRecoveryState arrives from the host.
     */
    fun cancel() {
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
        val cancelledToken = turnToken
        val cancelledText = _state.value.streamingMessage?.text.orEmpty()
        val cancelledTurn = durableTurnId
        val cancelledSource = source
        val submission = activeSubmission?.takeIf { it.source === cancelledSource && it.turnId == cancelledTurn }
        val cancelledSession = canonicalSessionId(
            submission?.sessionId ?: if (_state.value.session.id == "new") cancelledSource.activeSessionState.value?.sessionId ?: "new"
            else _state.value.session.id,
        )
        var orderedCancellation = cancelledSource.orderedCancellationBoundaries
        var uncertainAdmission = false
        cancellingProjection = cancelledTurn?.let { CancellingProjection(cancelledSession, it, sourceGeneration) }
        // Supersede the turn: a late event arriving after the engine's Cancel
        // round-trip must not re-open streaming on the now-idle transcript.
        val job = abandonLocalTurn()
        _state.update {
            it.copy(
                streaming = false,
                cancellationInFlight = true,
                liveTurnWaitingForUser = false,
                statusLine = strings.resolve(R.string.chat_stopping, "正在停止…"),
            )
        }
        emitTurnCompletion(cancelledToken, ConversationTurnOutcome.Cancelled, cancelledText)
        val cancellation = viewModelScope.async {
            runCatching {
                try {
                    val result = cancelSubmittedTurn(cancelledSource, job, cancelledTurn, cancelledSession, submission)
                    orderedCancellation = result.sent && cancelledSource.orderedCancellationBoundaries
                    uncertainAdmission = result.uncertainAdmission
                } catch (error: Throwable) {
                    orderedCancellation = cancelledSource.orderedCancellationBoundaries
                    uncertainAdmission = submission?.admission?.outcome == TurnSubmissionAdmission.Outcome.Unknown
                    throw error
                }
            }
        }
        explicitCancellation = cancellation
        viewModelScope.launch {
            val result = cancellation.await()
            if (explicitCancellation === cancellation) {
                explicitCancellation = null
                if (!orderedCancellation) _state.update { it.copy(cancellationInFlight = false) }
            }
            result.fold(
                onSuccess = {
                    if (!orderedCancellation && cancelledTurn != null) {
                        finishCancelledProjection(ConversationDelivery.CancellationDrained(cancelledSession, cancelledTurn, !uncertainAdmission))
                    }
                    if (!_state.value.sessionTransitioning) {
                        if (!orderedCancellation) _state.update { it.copy(statusLine = null) }
                    }
                },
                onFailure = { t ->
                    if (uncertainAdmission && cancelledTurn != null) {
                        cancellingProjection?.finished?.complete(false)
                        cancellingProjection = null
                        preserveUncertainAdmission(cancelledSource, cancelledSession, cancelledTurn, sourceGeneration)
                        return@fold
                    }
                    if (!orderedCancellation && cancelledTurn != null) {
                        abortCancelledProjection(ConversationDelivery.CancellationAborted(cancelledSession, cancelledTurn, false))
                    }
                    _state.update {
                        it.copy(
                            cancellationInFlight = if (orderedCancellation && cancellingProjection != null) true else false,
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

    private fun showCompactRejection() {
        _state.update { state ->
            val optimistic = state.compaction?.let {
                it.status == CompactionProgressStatus.Running && it.phase == "queued"
            } == true
            state.copy(
                compactRejectionId = java.util.UUID.randomUUID().toString(),
                compaction = if (optimistic) null else state.compaction,
                messages = if (optimistic && state.messages.lastOrNull()?.let {
                    it.role == Role.User && isManualCompactCommand(it.text)
                } == true) state.messages.dropLast(1) else state.messages,
            )
        }
    }

    fun dismissCompactRejection() { _state.update { it.copy(compactRejectionId = null) } }

    /** Dismiss the persistent error banner (its × affordance). */
    fun dismissError() {
        _state.update { if (it.recoveryStorageFailed) it else it.copy(error = null) }
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
                        messages = state.messages + Message(role = Role.Ai, text = event.display),
                        streamingMessage = null,
                        statusLine = null,
                        error = if (event.isError) {
                            ChatError(event.display, classifyError(event.display))
                        } else {
                            null
                        },
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
            is ReplyEvent.DurableTurnStreamEnded -> Unit

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
        sideChats.release()
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
        val targetSource = source
        val targetSession = _state.value.session.id.takeUnless { it == "new" }
            ?: targetSource.activeSessionState.value?.sessionId ?: return
        viewModelScope.launch {
            runCatching { targetSource.cancelForSession(targetSession, turnId) }
        }
    }

    internal companion object {
        // SavedStateHandle keys for lightweight process-death navigation state.
        const val LEGACY_KEY_TRANSCRIPT = "chat.transcript" // removed on migration; never decoded
        const val KEY_DRAFT = "chat.draft" // String — unsent composer text
        const val KEY_SESSION_ID = "chat.session.id" // String
        const val KEY_SESSION_TITLE = "chat.session.title" // String
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
