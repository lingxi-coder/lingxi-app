package com.lingxi.code.conversation

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.content.Context
import android.content.SharedPreferences
import android.os.Build
import com.lingxi.code.R
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.ListingKindDto
import com.lingxi.code.bindings.McpServerDto
import com.lingxi.code.bindings.McpStatusDto
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.MCPServer
import com.lingxi.code.bindings.ErrorKindDto
import com.lingxi.code.bindings.ImageRefDto
import com.lingxi.code.bindings.MessageBlockDto
import com.lingxi.code.bindings.MessageDto
import com.lingxi.code.bindings.MessageImageDto
import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.bindings.PermissionRequest
import com.lingxi.code.bindings.PermissionResponseDto
import com.lingxi.code.bindings.TurnRecoveryStateDto
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.DefaultSessionCatalogStrings
import com.lingxi.code.model.SessionCatalog
import com.lingxi.code.model.SessionCatalogStrings
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.model.sessionCatalogStrings
import com.lingxi.code.secure.SecureKeyStore
import com.lingxi.code.settings.ProviderSettingsRepository
import com.lingxi.code.project.ProjectWorkspace
import com.lingxi.code.secure.resolveEngineCredentials
import com.lingxi.code.settings.LinuxRuntimeMode
import com.lingxi.code.voice.buildVoiceEngine
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.flow.transformWhile
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONObject

internal data class DurableConversationTurnRecord(
    val scope: String,
    val sessionId: String,
    val turnId: Long,
)

/** Minimum non-secret launch context needed for service redelivery recovery. */
data class ConversationRecoverySpec(
    val projectId: String?,
    val hostPath: String?,
    val linuxRuntimeMode: LinuxRuntimeMode,
) {
    val scopeKey: String get() = hostPath ?: "__global__"

    fun projectWorkspace(): ProjectWorkspace? = hostPath?.let { path ->
        ProjectWorkspace(projectId = projectId ?: "recovered", hostPath = path)
    }
}

internal class DurableConversationTurnClientStore(
    private val preferences: SharedPreferences,
    private val scope: String,
    latestExit: ApplicationExitInfo? = null,
) {
    init {
        val handledAt = preferences.getLong("handled_exit_timestamp", 0L)
        if (latestExit != null && latestExit.timestamp > handledAt) {
            if (latestExit.reason == ApplicationExitInfo.REASON_USER_REQUESTED) {
                clearRecord()
            }
            preferences.edit().putLong("handled_exit_timestamp", latestExit.timestamp).apply()
        }
    }

    constructor(
        context: Context,
        scope: String,
    ) : this(
        preferences = context.applicationContext.getSharedPreferences(
            "durable_conversation_turn",
            Context.MODE_PRIVATE,
        ),
        scope = scope,
        latestExit = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            context.getSystemService(ActivityManager::class.java)
                .getHistoricalProcessExitReasons(context.packageName, 0, 1)
                .firstOrNull()
        } else {
            null
        },
    )

    fun begin(sessionId: String, turnId: Long) {
        if (sessionId.isBlank()) return
        preferences.edit()
            .putString("scope", scope)
            .putString("session_id", sessionId)
            .putLong("turn_id", turnId)
            .commit()
    }

    fun load(): DurableConversationTurnRecord? {
        if (preferences.getString("scope", null) != scope) return null
        val sessionId = preferences.getString("session_id", null)?.takeIf(String::isNotBlank)
            ?: return null
        if (!preferences.contains("turn_id")) return null
        return DurableConversationTurnRecord(
            scope = scope,
            sessionId = sessionId,
            turnId = preferences.getLong("turn_id", 0L),
        )
    }

    fun clear(turnId: Long? = null) {
        val current = load() ?: return
        if (turnId == null || current.turnId == turnId) {
            clearRecord()
        }
    }

    /**
     * Clear only the exact session/turn checkpoint that produced a terminal
     * event.  Turn ids are client supplied, so the session comparison is
     * required as well: a late terminal event from a previous session must not
     * erase the checkpoint belonging to the current session.
     */
    fun clear(sessionId: String, turnId: Long) {
        val current = load() ?: return
        if (
            current.turnId == turnId &&
            canonicalSessionId(current.sessionId) == canonicalSessionId(sessionId)
        ) {
            clearRecord()
        }
    }

    private fun clearRecord() {
        preferences.edit()
            .remove("scope")
            .remove("session_id")
            .remove("turn_id")
            .remove("last_sequence")
            .commit()
    }
}

internal class DurableTurnReplayGate {
    private var pendingColdResumeAttachTurnId: Long? = null
    private var suppressingRetainedReplayTurnId: Long? = null
    private var awaitingTerminalConfirmationTurnId: Long? = null

    @Synchronized
    fun noteAttachRequested(
        turnId: Long,
        activation: ActivatedSession?,
        afterSequence: Long,
    ) {
        pendingColdResumeAttachTurnId =
            turnId.takeIf {
                afterSequence <= 0L &&
                    activation?.kind == SessionActivationKind.Resumed
            }
        if (pendingColdResumeAttachTurnId != turnId) {
            clearTurn(turnId)
        }
    }

    @Synchronized
    fun cancelAttach(turnId: Long) {
        if (pendingColdResumeAttachTurnId == turnId) pendingColdResumeAttachTurnId = null
    }

    @Synchronized
    fun shouldForward(event: ClientEvent): Boolean = when (event) {
        is ClientEvent.TurnRecoveryState -> {
            val turnId = event.snapshot.turnId.toLong()
            val terminal = event.snapshot.state in terminalStates
            when {
                pendingColdResumeAttachTurnId == turnId -> {
                    pendingColdResumeAttachTurnId = null
                    if (terminal) {
                        suppressingRetainedReplayTurnId = turnId
                        awaitingTerminalConfirmationTurnId = turnId
                    } else {
                        clearTurn(turnId)
                    }
                }
                awaitingTerminalConfirmationTurnId == turnId && terminal -> clearTurn(turnId)
            }
            true
        }
        is ClientEvent.TurnEventReplay ->
            suppressingRetainedReplayTurnId != event.turnId.toLong()
        is ClientEvent.TurnEnded, is ClientEvent.Error -> {
            clearAll()
            true
        }
        else -> true
    }

    private fun clearTurn(turnId: Long) {
        if (pendingColdResumeAttachTurnId == turnId) pendingColdResumeAttachTurnId = null
        if (suppressingRetainedReplayTurnId == turnId) suppressingRetainedReplayTurnId = null
        if (awaitingTerminalConfirmationTurnId == turnId) awaitingTerminalConfirmationTurnId = null
    }

    private fun clearAll() {
        pendingColdResumeAttachTurnId = null
        suppressingRetainedReplayTurnId = null
        awaitingTerminalConfirmationTurnId = null
    }
    private companion object {
        val terminalStates = setOf(
            TurnRecoveryStateDto.COMPLETED,
            TurnRecoveryStateDto.FAILED,
            TurnRecoveryStateDto.CANCELLED,
        )
    }
}

/**
 * Single-flight ownership for durable-turn attachment on one engine source.
 *
 * The headless service and a newly-created UI can observe the same
 * SessionResumed activation.  They must not both submit the same AttachTurn /
 * ResumeTurn pair, but a UI takeover is allowed to request a new replay from
 * its own cursor.  The key therefore includes the canonical session, turn,
 * and cursor, while ownership is tracked separately from the key.
 */
internal enum class DurableAttachOwner {
    Headless,
    Ui,
}

internal data class DurableAttachRequest(
    val sessionId: String,
    val turnId: Long,
    val afterSequence: Long,
)

/** A durable attach/resume failed after the checkpoint was already identified. */
internal class DurableAttachFailure(
    val turnId: Long,
    val phase: String,
    cause: Throwable,
) : IllegalStateException("Unable to $phase durable turn $turnId", cause)

/** Resume is needed for fresh UI/cold-headless sources, not retained takeovers. */
internal fun shouldResumeDurableTurn(
    forceAttach: Boolean,
    resumeOnUiAttach: Boolean,
): Boolean = !forceAttach || resumeOnUiAttach

internal class DurableAttachCoordinator {
    private var claimedByUi = false
    private var lastOwner: DurableAttachOwner? = null
    private var lastRequest: DurableAttachRequest? = null

    /** Claim the source for UI attachment; subsequent headless requests skip. */
    @Synchronized
    fun claimForUi() {
        claimedByUi = true
    }

    /** Release the UI claim when the Activity is gone and service monitoring resumes. */
    @Synchronized
    fun releaseToHeadless() {
        claimedByUi = false
    }

    /**
     * Reserve one command pair. A UI takeover is intentionally a new owner,
     * even when it starts at cursor zero, because the service's SharedFlow
     * collector may have consumed that earlier replay.
     */
    @Synchronized
    fun reserve(request: DurableAttachRequest, owner: DurableAttachOwner): Boolean {
        if (owner == DurableAttachOwner.Headless && claimedByUi) return false
        if (lastOwner == owner && lastRequest == request) return false
        lastOwner = owner
        lastRequest = request
        return true
    }

    @Synchronized
    fun rollback(request: DurableAttachRequest, owner: DurableAttachOwner) {
        if (lastOwner == owner && lastRequest == request) {
            lastOwner = null
            lastRequest = null
        }
    }

    internal fun ownerForTesting(): DurableAttachOwner? = synchronized(this) { lastOwner }
}

internal inline fun submitTurnCancellation(
    permissionIngress: PermissionIngress,
    submit: () -> Unit,
) {
    val permissionSnapshot = permissionIngress.beginCancellation()
    try {
        submit()
    } catch (error: Throwable) {
        permissionIngress.restoreAfterFailedCancellation(permissionSnapshot)
        throw error
    }
}

/**
 * Resolves a localized string for conversation-package code that runs OUTSIDE a
 * `@Composable` body — [ChatViewModel], [ConversationSource] and the pure
 * top-level mappers below ([clientEventToReply], [userFacingEngineError],
 * [mapReplyStream], [messageDtoText]) — and therefore cannot call
 * `stringResource()`.
 *
 * [fallback] is always the exact zh-Hans base-locale copy for [id], passed in at
 * the call site right next to the resource id. [DefaultConversationStrings]
 * (the parameter default everywhere this is threaded through) returns [fallback]
 * verbatim — formatted with [args] when present — which is why the many JVM unit
 * tests that construct [ChatViewModel] / call these mappers directly, with no
 * Android `Context` at all, keep asserting the same literal Chinese copy without
 * being touched. The production implementation ([conversationStrings]) ignores
 * [fallback] and resolves the REAL localized text through
 * [Context.getString], so the app actually renders in the user's selected
 * language, not just in tests.
 */
fun interface ConversationStrings {
    fun resolve(id: Int, fallback: String, vararg args: Any): String
}

/** Test/no-Context fallback: the literal zh-Hans copy, `String.format`-ed. */
val DefaultConversationStrings = ConversationStrings { _, fallback, args ->
    if (args.isEmpty()) fallback else String.format(java.util.Locale.getDefault(), fallback, *args)
}

/** Production resolver: real localized text via the app's (locale-wrapped) [Context]. */
fun conversationStrings(context: Context): ConversationStrings =
    ConversationStrings { id, _, args -> context.getString(id, *args) }

/**
 * Process-wide FIFO for permission callbacks. Workflow children can request
 * permission while the main turn is idle, so main-turn lifecycle events must
 * not suppress or clear their prompts. The engine's correlated
 * `PermissionRequestResolved` event removes the exact request that completed.
 */
internal class PermissionIngress(
    private val permissions: MutableStateFlow<PermissionPromptState?>,
    private val strings: ConversationStrings = DefaultConversationStrings,
) {
    /** Multiple workflow children can park independently; preserve callback order. */
    private val queued = linkedMapOf<ULong, PermissionPromptState>()

    internal class CancellationSnapshot internal constructor()

    @Synchronized
    fun beginTurn() = Unit

    @Synchronized
    fun confirmTurnStarted() = Unit

    @Synchronized
    fun beginCancellation(): CancellationSnapshot = CancellationSnapshot()

    @Synchronized
    fun restoreAfterFailedCancellation(@Suppress("UNUSED_PARAMETER") snapshot: CancellationSnapshot) = Unit

    @Synchronized
    fun endTurn() = Unit

    @Synchronized
    fun publish(request: PermissionRequest) {
        queued[request.requestId] = permissionRequestToPrompt(request, strings)
        publishHead()
    }

    @Synchronized
    fun resolve(requestId: ULong) {
        queued.remove(requestId)
        publishHead()
    }

    private fun publishHead() {
        permissions.value = queued.values.firstOrNull()
    }
}

/**
 * The seam between the [ChatViewModel] and whatever produces turns. The
 * ViewModel talks ONLY to a [ConversationSource] — it never reaches into mock
 * data, a network, or the engine handle directly. Two implementations back it:
 *
 *  - [UnavailableConversationSource] — explicit empty/error state when the
 *    engine cannot be built.
 *  - [EngineConversationSource] — the real in-process engine over UniFFI: owns
 *    a [MobileEngineHandle] built via [buildVoiceEngine], registers an
 *    `AndroidEventListener` whose `onEvent` pushes each inbound [ClientEvent]
 *    into an internal flow, and drives turns via
 *    `handle.submit(ClientCommand.SendPrompt(...))`.
 *
 * This mirrors the iOS `ConversationSource` seam (clients/ios → ConversationSource.swift):
 * one protocol (`client-protocol`), one transport (UniFFI), one renderer, with a
 * fail-closed unavailable state when the engine can't build.
 */
interface ConversationSource {

    /** Process-redelivery context for the engine this source owns. */
    val recoverySpec: ConversationRecoverySpec?
        get() = null

    /**
     * Out-of-band engine events consumed by feature stores such as Local Apps.
     *
     * Conversation rendering still observes its purpose-built state flows; this
     * generic stream exists so profile-global features can share the exact same
     * engine connection instead of constructing a second [MobileEngineHandle].
     */
    val clientEvents: Flow<ClientEvent>
        get() = emptyFlow()

    /** Structured workflow/subagent updates, pushed directly by the engine. */
    val workflowProgress: Flow<WorkflowProgressUpdate>
        get() = emptyFlow()

    /** Submit a non-conversation protocol command through this source. */
    suspend fun submitClientCommand(command: ClientCommand) {}

    /** Refresh task rows and the current session agent roster after a resume. */
    suspend fun refreshExecutionStatus() {}

    /** Attach the durable active turn when a new UI owner binds this source. */
    suspend fun attachDurableTurnForUi(afterSequence: Long = 0L) {}

    /** Apply the persisted permission-mode preference to the live engine. */
    suspend fun setPermissionMode(mode: String) {
        submitClientCommand(ClientCommand.SetPermissionMode(mode))
    }

    /** The conversation a freshly-opened session starts with. */
    fun initialMessages(): List<Message> = emptyList()

    /**
     * Submit a user turn and observe the assistant's reply as a stream of
     * [ReplyEvent]s. The mock emits a single [ReplyEvent.Thinking] then a
     * [ReplyEvent.Completed]; the engine emits incremental [ReplyEvent.Delta]s
     * (plus [ReplyEvent.Thinking] / [ReplyEvent.ToolActivity]) and terminates on
     * [ReplyEvent.End] or [ReplyEvent.Error].
     */
    fun submit(text: String): Flow<ReplyEvent> = submit(text, emptyList())

    /** Submit a prompt with the same ordered inline images used by the CLI. */
    fun submit(text: String, images: List<ImageRefDto>): Flow<ReplyEvent> = submit(text)

    /** Submit with the stable durable id used by background attach/resume. */
    fun submit(text: String, images: List<ImageRefDto>, turnId: Long): Flow<ReplyEvent> =
        submit(text, images)

    /**
     * Cancel the in-flight turn (the composer's Stop affordance). Fires the
     * engine's `Cancel` command so the streaming turn terminates promptly; the
     * resulting `TurnEnded` flows back through [submit]'s stream as a normal
     * [ReplyEvent.End]. A no-op for sources with no cancellable turn (the mock).
     */
    suspend fun cancel() {}

    /** Cancel exactly one durable turn, including from a notification route. */
    suspend fun cancel(turnId: Long?) = cancel()

    /**
     * Discard a recovered checkpoint by correlated turn id. Implementations
     * must not treat command acceptance as terminal; the host emits the
     * matching terminal [ClientEvent.TurnRecoveryState] asynchronously.
     */
    suspend fun discardDurableTurn(turnId: Long) {
        cancel(turnId)
    }

    /**
     * The head parked permission request awaiting the user's allow/deny, or
     * `null` when none is pending. The engine emits a [PermissionRequest]
     * OUTBOUND whenever a tool needs approval; the UI renders this and resolves
     * it via [approvePermission] / [denyPermission]. Sources with no engine (the
     * mock) never emit one, so this stays `null`.
     */
    val pendingPermission: StateFlow<PermissionPromptState?>
        get() = MutableStateFlow<PermissionPromptState?>(null).asStateFlow()

    /**
     * Approve the parked request `requestId` with `response` (once / always),
     * submitting `ClientCommand.ApprovePermission` so the engine's parked turn
     * resumes. A no-op for sources with no engine (the mock).
     */
    suspend fun approvePermission(requestId: ULong, response: PermissionResponseDto) {}

    /**
     * Deny the parked request `requestId`, submitting
     * `ClientCommand.DenyPermission` so the engine's parked turn unwinds. A no-op
     * for sources with no engine (the mock).
     */
    suspend fun denyPermission(requestId: ULong) {}

    /**
     * The engine's REAL model catalog + active id — the SEPARATE, out-of-band
     * model-state path (SHIP-BLOCKER #2). `ModelList` / `ModelChanged` are NOT
     * part of a text turn, so they flow here (a [StateFlow]) instead of through
     * [submit]'s per-turn [ReplyEvent] stream. Empty means the engine hasn't
     * reported its catalog yet.
     */
    val modelState: StateFlow<EngineModelState>
        get() = MutableStateFlow(EngineModelState()).asStateFlow()

    /**
     * Switch the engine's active model to the REAL wire `id`, submitting
     * `ClientCommand.SetModel`. The engine confirms with `ModelChanged`, which
     * updates [modelState]'s active id. A no-op for sources with no engine (the
     * mock keeps its local selection).
     */
    suspend fun setModel(id: String) {}

    /**
     * The engine's REAL resumable-session catalog — the SEPARATE, out-of-band
     * session-state path, the exact sibling of [modelState]. `SessionList` (the
     * reply to `ListSessions`) is NOT part of a text turn, so it flows here (a
     * [StateFlow]) instead of through [submit]'s per-turn [ReplyEvent] stream.
     * The UI observes [EngineSessionState.phase] to distinguish loading / empty /
     * error without a mock fallback.
     */
    val sessionState: StateFlow<EngineSessionState>
        get() = MutableStateFlow(EngineSessionState.loading()).asStateFlow()

    /**
     * Ask the engine to (re)report its resumable-session catalog, submitting
     * `ClientCommand.ListSessions`. The reply (`SessionList`) updates
     * [sessionState] out-of-band. Called when the drawer opens so the list is
     * fresh.
     */
    suspend fun refreshSessions() {}

    /**
     * Resume the engine session named by the REAL wire [uuid], submitting
     * `ClientCommand.ResumeSession`. The engine confirms with `SessionResumed`,
     * which carries the full restored transcript — surfaced OUT-OF-BAND through
     * [activeSessionState] so the ViewModel rehydrates the prior conversation.
     */
    suspend fun resumeSession(uuid: String) {}

    /**
     * Resume a confirmed zero-message session while preserving its UUID.
     *
     * This separate proof is required only for mobile upgrades whose Project
     * index predates the engine's empty-session JSONL anchor. Implementations
     * without that migration concern fall back to the normal resume path.
     */
    suspend fun resumeEmptySession(uuid: String, title: String) {
        resumeSession(uuid)
    }

    /**
     * The engine's REAL MCP server listing (out-of-band, sibling of [modelState]).
     * The listener folds `McpServers` into this StateFlow; the settings layer
     * mirrors it into the store when populated. Empty default ⇒ mock list.
     */
    val mcpServers: StateFlow<List<MCPServer>>
        get() = MutableStateFlow(emptyList<MCPServer>()).asStateFlow()

    /** Pull the real MCP listing (`RefreshListings(Mcp)` → `McpServers`). No-op on mock. */
    suspend fun refreshMcpServers() {}

    /**
     * The engine's most recent active-session transition — the SEPARATE,
     * out-of-band session-activation path, the sibling of [sessionState].
     * `SessionStarted` and `SessionResumed` are NOT part of a text turn, so the
     * active session id and any restored transcript flow here instead of through
     * [submit]'s per-turn [ReplyEvent] stream.
     */
    val activeSessionState: StateFlow<ActivatedSession?>
        get() = MutableStateFlow<ActivatedSession?>(null).asStateFlow()

    /**
     * Start a fresh engine session, submitting `ClientCommand.NewSession`. The
     * engine confirms with `SessionStarted`; the UI adopts the returned session id
     * from [activeSessionState].
     */
    suspend fun newSession() {}

    /** Release native handles and event pumps owned by this source. Idempotent. */
    fun close() {}
}

internal interface BackgroundRetainableConversationSource {
    fun engineSourceForBackgroundRetention(): EngineConversationSource?
}

/**
 * Non-blocking callback ingress with lossless, ordered delivery to a Flow.
 *
 * Native event callbacks must return promptly, while assistant deltas and turn
 * boundaries must never be discarded. An unlimited channel decouples the
 * callback thread from a single suspending SharedFlow pump: slow collectors add
 * bounded-by-turn memory pressure instead of blocking Rust or dropping tokens.
 */
internal class LosslessEventRelay<T>(
    scope: CoroutineScope,
) {
    private val queue = Channel<T>(capacity = Channel.UNLIMITED)
    private val shared = MutableSharedFlow<T>(extraBufferCapacity = 64)
    val events: SharedFlow<T> = shared.asSharedFlow()

    init {
        scope.launch {
            for (event in queue) shared.emit(event)
        }
    }

    /** Safe for a native callback thread; preserves FIFO order without suspension. */
    fun offer(event: T): Boolean = queue.trySend(event).isSuccess

    fun close() {
        queue.close()
    }
}

/**
 * PURE reducer for the out-of-band model events. Folds one inbound engine
 * [ClientEvent] into the prior [EngineModelState], or returns `prev` unchanged
 * for every event that isn't a model event. Mirrors [clientEventToReply] in
 * being a free function with NO engine / Android dependency so the
 * `ModelList` / `ModelChanged` handling is exhaustively unit-testable on the JVM
 * (no `buildAndroidEngine`).
 *
 *  - `ModelList`    → replace the catalog with the engine's real ids; adopt
 *                     `current` as the active id.
 *  - `ModelChanged` → keep the catalog, swap the active id to the new model.
 *  - anything else  → unchanged (`#[non_exhaustive]`, so an `else` is required).
 */
fun reduceModelEvent(prev: EngineModelState, event: ClientEvent): EngineModelState =
    when (event) {
        is ClientEvent.ModelList -> EngineModelState(
            available = event.models,
            active = event.current,
            details = event.details.associate { detail ->
                detail.reference to com.lingxi.code.model.CatalogModelDetails.fromDto(detail)
            },
        )
        is ClientEvent.ModelChanged -> prev.copy(active = event.model)
        else -> prev
    }

/**
 * PURE reducer for the out-of-band SESSION events — the exact sibling of
 * [reduceModelEvent]. Folds one inbound engine [ClientEvent] into the prior
 * [EngineSessionState], or returns `prev` unchanged for every event that isn't a
 * session-catalog event.
 *
 *  - `SessionList` → replace the catalog with the engine's real rows, mapping
 *                    each wire `SessionRowDto` to a UI [SessionRow] (title +
 *                    message count + humanized relative time) via
 *                    [SessionCatalog.rowFrom].
 *  - anything else → unchanged (`#[non_exhaustive]`, so an `else` is required).
 *
 * `SessionStarted` / `SessionResumed` / `SessionEnded` are lifecycle events the
 * ViewModel acts on (transcript reset / title swap), NOT catalog mutations, so
 * they are intentionally ignored here. In particular `SessionStarted` and
 * `SessionResumed` ride their own out-of-band path
 * ([sessionActivationFrom] → [ConversationSource.activeSessionState]) rather than
 * the catalog. [nowEpochSeconds] is injected so the relative-time bucketing is
 * deterministic in unit tests.
 */
fun reduceSessionEvent(
    prev: EngineSessionState,
    event: ClientEvent,
    nowEpochSeconds: Long = System.currentTimeMillis() / 1000L,
    strings: SessionCatalogStrings = DefaultSessionCatalogStrings,
): EngineSessionState =
    when (event) {
        is ClientEvent.SessionList -> EngineSessionState.ready(
            rows = event.sessions.map { dto ->
                SessionCatalog.rowFrom(
                    uuid = dto.uuid,
                    title = dto.title,
                    messageCount = dto.messageCount.toInt(),
                    modifiedRfc3339 = dto.modifiedRfc3339,
                    nowEpochSeconds = nowEpochSeconds,
                    strings = strings,
                )
            },
        )
        else -> prev
    }

/**
 * The result of a live session activation. `SessionStarted` carries the new
 * engine session id and an empty transcript; `SessionResumed` carries the
 * resumed id plus the restored transcript. PURE (no engine / Android types) so
 * rehydration is unit-testable on the plain JVM.
 */
enum class SessionActivationKind {
    Started,
    Resumed,
}

data class ActivatedSession(
    val sessionId: String,
    val transcript: List<Message>,
    val kind: SessionActivationKind,
)

/**
 * Lower one [McpServerDto] to the UI [MCPServer] model. The DTO is thinner than
 * the mock (no url / tool-count), so those default; status maps Connected→Connected,
 * Disconnected→Idle, Error→Error.
 */
fun McpServerDto.toMcpServer(): MCPServer {
    val s = when (status) {
        is McpStatusDto.Connected -> ConnStatus.Connected
        is McpStatusDto.Disconnected -> ConnStatus.Idle
        is McpStatusDto.Error -> ConnStatus.Error
    }
    return MCPServer(
        id = name, name = name, url = "", tools = 0,
        status = s, enabled = s == ConnStatus.Connected, transport = transport,
    )
}

/**
 * PURE recognizer for the out-of-band active-session transition — the sibling of
 * [reduceSessionEvent] for the catalog. Maps `SessionStarted` / `SessionResumed`
 * to the [ActivatedSession] the ViewModel rehydrates, or `null` for every other
 * event. The wire [MessageDto]s are lowered OLDEST-FIRST via
 * [messageDtoToMessage] — the same order the engine emits — so the scrollback
 * renders in conversation order. A free function with NO engine / Android
 * dependency so it is exhaustively unit-testable on the JVM.
 */
fun sessionActivationFrom(
    event: ClientEvent,
    strings: ConversationStrings = DefaultConversationStrings,
): ActivatedSession? = when (event) {
    is ClientEvent.SessionStarted -> ActivatedSession(
        sessionId = canonicalSessionId(event.sessionId),
        transcript = emptyList(),
        kind = SessionActivationKind.Started,
    )
    is ClientEvent.SessionResumed -> ActivatedSession(
        sessionId = canonicalSessionId(event.sessionId),
        transcript = transcriptFromDtos(event.messages, strings),
        kind = SessionActivationKind.Resumed,
    )
    else -> null
}

/**
 * PURE recognizer for `PlanUpdated` — the model-managed todo checklist.
 *
 * A FULL-LIST REPLACE, emitted on the TodoWrite CALL (not its result), so an
 * empty list is a legitimate payload meaning "clear the panel" — distinct from
 * the `null` this returns for every other event. It rides the OUT-OF-BAND
 * `clientEvents` stream (next to `AskUserQuestion` / `TaskStatusChanged`), not
 * the per-turn reply stream: the plan outlives the turn that rewrote it.
 */
fun planTasksFrom(event: ClientEvent): List<PlanTaskUi>? = when (event) {
    is ClientEvent.PlanUpdated -> event.tasks.map { it.toUi() }
    else -> null
}

/**
 * Lower one wire [MessageDto] to the UI [Message] model.
 *
 * The assistant bubble is no longer text-only: alongside the flattened prose in
 * [Message.text] the message now carries ORDERED [MessageContent] blocks, so a
 * tool call renders as its derived header + `⎿` result instead of collapsing to
 * the one-line `"调用工具 X…"` placeholder that discarded every diff and body.
 *
 * `ToolUse` and `ToolResult` are paired by id WITHIN this one message here; the
 * engine actually splits them across two messages (call in assistant N, result
 * in user N+1), which is why a whole transcript must go through
 * [transcriptFromDtos] instead of mapping each DTO independently.
 *
 * The wire `role` ("user" / "assistant" / "system") maps to [Role]: "user" →
 * [Role.User]; everything else → [Role.Ai]. PURE — no engine dependency.
 */
fun messageDtoToMessage(
    dto: MessageDto,
    strings: ConversationStrings = DefaultConversationStrings,
): Message {
    val build = MessageBuild(dto.role, messageImages(dto.images))
    val index = mutableMapOf<String, ToolBlockRef>()
    dto.blocks.forEach { block ->
        // One DTO in isolation has no preceding turn to hand an orphan result
        // to, and a user build can never render one (see [orphanHostFor]), so
        // an assistant DTO keeps its own orphans and a user DTO drops them.
        build.fold(block, strings, index) { build.takeIf { it.role == Role.Ai } }
    }
    return build.toMessage()
}

/**
 * Lower a WHOLE transcript, threading the tool-use index across messages.
 *
 * A `ToolUse` sits in assistant message N and its `ToolResult` in user message
 * N+1 — never in the same message (see `client-adapter`'s `ToolUseIndex`, which
 * keeps the same side-table for the same reason). Mapping each [MessageDto]
 * independently therefore renders every restored tool call as a header with no
 * result, and leaves a content-free user bubble holding the orphaned results.
 *
 * So: results are folded BACK into the message that made the call, and any
 * message left with neither prose nor a tool block is dropped rather than
 * rendered as an empty bubble. PURE — exercised on the JVM.
 */
fun transcriptFromDtos(
    dtos: List<MessageDto>,
    strings: ConversationStrings = DefaultConversationStrings,
): List<Message> {
    val builds = mutableListOf<MessageBuild>()
    val index = mutableMapOf<String, ToolBlockRef>()
    dtos.forEach { dto ->
        val build = MessageBuild(dto.role, messageImages(dto.images))
        builds.add(build)
        dto.blocks.forEach { block ->
            build.fold(block, strings, index) { orphanHostFor(builds, build) }
        }
    }
    return builds.filter { it.isRenderable() }.map { it.toMessage() }
}

/**
 * Which build an ORPHAN `ToolResult` — one whose `ToolUse` fell outside a torn
 * or compacted transcript window — is folded into.
 *
 * NEVER the user build it arrived in. [MessageBubble] renders a user turn from
 * [Message.text] alone and ignores its blocks, so a tool block parked there is
 * invisible AND makes [MessageBuild.isRenderable] answer `true`, producing the
 * empty bordered bubble that predicate exists to prevent. The old
 * `previous ?: this` fallback did exactly that whenever the orphan landed in
 * the FIRST message of the window (no `previous` at all) or right after another
 * user message.
 *
 * The host is the IMMEDIATELY preceding build when that is an assistant turn —
 * the one that made the call in every window torn only at the block level.
 * Otherwise the calling turn itself fell outside the window, and a build is
 * MINTED and spliced in right where it used to be, so the row renders in its
 * own position instead of being retro-fitted into an unrelated older bubble. A
 * minted build that never receives a block stays empty and is dropped by the
 * `isRenderable` filter, so this can never introduce a blank bubble of its own;
 * a second orphan in the same message finds the mint and reuses it.
 */
private fun orphanHostFor(builds: MutableList<MessageBuild>, current: MessageBuild): MessageBuild {
    if (current.role == Role.Ai) return current
    val at = builds.indexOf(current).coerceAtLeast(0)
    builds.getOrNull(at - 1)?.takeIf { it.role == Role.Ai }?.let { return it }
    val minted = MessageBuild(ASSISTANT_WIRE_ROLE)
    builds.add(at, minted)
    return minted
}

/**
 * The engine persists image bytes as a data URL in the resumed message DTO.
 * Keep the UI model's existing ImageRefDto shape so live and restored user
 * turns use the same renderer and resend path.
 */
private fun messageImages(images: List<MessageImageDto>): List<ImageRefDto> =
    images.mapNotNull { image ->
        val marker = ";base64,"
        val markerIndex = image.url.indexOf(marker)
        if (!image.url.startsWith("data:") || markerIndex <= "data:".length) return@mapNotNull null
        val mediaType = image.mediaType.ifBlank {
            image.url.substring("data:".length, markerIndex)
        }
        val base64 = image.url.substring(markerIndex + marker.length)
        if (mediaType.isBlank() || base64.isBlank()) null else ImageRefDto(mediaType, base64)
    }

/** The wire `role` a minted assistant build carries. */
private const val ASSISTANT_WIRE_ROLE = "assistant"

/** Where a recorded `ToolUse` block lives, so its later `ToolResult` can reach it. */
private class ToolBlockRef(val build: MessageBuild, val blockIndex: Int)

/** Mutable accumulator for one message's prose + ordered content blocks. */
private class MessageBuild(
    wireRole: String,
    val images: List<ImageRefDto> = emptyList(),
) {
    val role: Role = if (wireRole.equals("user", ignoreCase = true)) Role.User else Role.Ai
    val textParts = mutableListOf<String>()
    val blocks = mutableListOf<MessageContent>()

    /**
     * True when the message has anything to show — i.e. exactly what
     * [MessageBubble] would actually draw for this role. A user turn carrying
     * ONLY the previous assistant turn's tool results has neither prose nor a
     * tool block of its own (they were folded back), and must not render as an
     * empty bubble.
     *
     * The role check is not redundant with [orphanHostFor]: the user branch of
     * the bubble renders [Message.text] and nothing else, so "has a tool block"
     * can only mean "renderable" for an assistant turn. Answering `true` for a
     * text-less user build is what produced the empty bordered bubble.
     */
    fun isRenderable(): Boolean =
        textParts.any { it.isNotBlank() } ||
            (role == Role.User && images.isNotEmpty()) ||
            (role == Role.Ai && blocks.any { it is MessageContent.Tool })

    fun toMessage(): Message = Message(
        role = role,
        text = textParts.filter { it.isNotBlank() }.joinToString("\n\n"),
        images = images,
        blocks = blocks.toList(),
    )

    private fun addText(text: String) {
        textParts.add(text)
        if (text.isNotBlank()) blocks.add(MessageContent.Text(text))
    }

    /**
     * Fold one wire block in. The `when` is exhaustive over the generated
     * [MessageBlockDto] subclasses — a regen that adds a block kind is a compile
     * error here, mirroring the engine's exhaustive `ContentBlock` match.
     */
    fun fold(
        block: MessageBlockDto,
        strings: ConversationStrings,
        index: MutableMap<String, ToolBlockRef>,
        /**
         * Resolved LAZILY, and only for an orphan result, because resolving it
         * can MINT a build ([orphanHostFor]) — doing that eagerly per message
         * would splice an empty build ahead of every user turn.
         */
        orphanHost: () -> MessageBuild?,
    ) {
        when (block) {
            is MessageBlockDto.Text -> addText(block.text)
            is MessageBlockDto.Thinking -> addText(block.thinking)
            is MessageBlockDto.RedactedThinking ->
                addText(strings.resolve(R.string.chat_redacted_thinking, "[已折叠的思考]"))
            is MessageBlockDto.CompactBoundary ->
                addText(strings.resolve(R.string.chat_compacted_label, "对话已压缩"))
            is MessageBlockDto.ToolUse -> {
                index[block.id] = ToolBlockRef(this, blocks.size)
                blocks.add(
                    MessageContent.Tool(
                        ToolCallUi(
                            id = block.id,
                            tool = block.tool,
                            header = block.header?.toUi(),
                            status = AgentToolStatus.Running,
                            // Older engine (no header): the legacy scrape is the floor.
                            fallbackSummary = if (block.header == null) {
                                summarizeToolInput(block.inputJson)
                            } else {
                                null
                            },
                        ),
                    ),
                )
            }
            is MessageBlockDto.ToolResult -> {
                val status =
                    if (block.isError) AgentToolStatus.Failed else AgentToolStatus.Completed
                val display = block.display?.toUi()
                val ref = index.remove(block.id)
                if (ref != null) {
                    val existing = ref.build.blocks[ref.blockIndex] as MessageContent.Tool
                    ref.build.blocks[ref.blockIndex] = MessageContent.Tool(
                        existing.call.copy(display = display, status = status),
                    )
                } else {
                    // ORPHAN result — a torn or compacted transcript window. It
                    // belongs to an ASSISTANT turn, never to the user bubble it
                    // arrived in (which renders text only).
                    orphanHost()?.blocks?.add(
                        MessageContent.Tool(
                            ToolCallUi(
                                id = block.id,
                                tool = block.tool,
                                display = display,
                                status = status,
                            ),
                        ),
                    )
                }
            }
        }
    }
}

/**
 * LEGACY: flatten a message's ordered content [MessageBlockDto]s into ONE body
 * string, collapsing every tool call to a `"调用工具 X…"` placeholder.
 *
 * The bubble no longer renders this — [messageDtoToMessage] and
 * [transcriptFromDtos] now emit ordered [MessageContent] blocks so a tool call
 * keeps its derived header and `⎿` result instead of being erased into one line.
 * This remains as the plain-text projection of a message (and as the shape a
 * pre-structure client produced), so it must keep folding EVERY block kind:
 *  - Text             → the text verbatim.
 *  - Thinking         → the reasoning text (the bubble has no separate thinking
 *                       region for restored scrollback; it reads inline).
 *  - RedactedThinking → a placeholder marker (the payload is opaque).
 *  - CompactBoundary  → a visible boundary marker; its hidden summary is not
 *                       rendered as user-authored text.
 *  - ToolUse          → a compact "调用工具 <tool>" activity line.
 *  - ToolResult       → a compact "工具结果"/"工具失败" line.
 * Blocks are joined by blank lines and blanks are dropped so an empty trailing
 * block never leaves dangling whitespace. The `when` is exhaustive over the
 * generated [MessageBlockDto] subclasses — a regen that adds a new block kind is
 * a compile error here, mirroring the engine's exhaustive `ContentBlock` match.
 */
fun messageDtoText(
    blocks: List<MessageBlockDto>,
    strings: ConversationStrings = DefaultConversationStrings,
): String =
    blocks.mapNotNull { block ->
        val line: String = when (block) {
            is MessageBlockDto.Text -> block.text
            is MessageBlockDto.Thinking -> block.thinking
            is MessageBlockDto.RedactedThinking -> strings.resolve(R.string.chat_redacted_thinking, "[已折叠的思考]")
            is MessageBlockDto.CompactBoundary -> strings.resolve(R.string.chat_compacted_label, "对话已压缩")
            is MessageBlockDto.ToolUse ->
                strings.resolve(R.string.chat_tool_calling_label, "调用工具 %1\$s…", block.tool)
            is MessageBlockDto.ToolResult ->
                if (block.isError) {
                    strings.resolve(R.string.chat_tool_result_failed, "工具失败")
                } else {
                    strings.resolve(R.string.chat_tool_result_label, "工具结果")
                }
        }
        line.takeUnless { it.isBlank() }
    }.joinToString("\n\n")

/**
 * Streamed assistant-reply events — the UI-facing analog of engine
 * [ClientEvent]s. [clientEventToReply] maps the wire events onto these; the
 * [ChatViewModel] reduces them into [ChatState].
 */
sealed interface ReplyEvent {
    /** The model is thinking; starts the live run trace before text arrives. */
    data object Thinking : ReplyEvent

    /** An incremental model-reasoning delta, rendered in the live run trace. */
    data class ReasoningDelta(val text: String) : ReplyEvent

    /** An incremental assistant-text delta (the engine's streamed tokens). */
    data class Delta(val text: String) : ReplyEvent

    /** Correlated tool activity surfaced in both the status row and run trace. */
    data class ToolActivity(
        val label: String,
        val id: String? = null,
        val tool: String? = null,
        val status: AgentToolStatus? = null,
        /** LEGACY input scrape — the fallback when [header] is null (older engine). */
        val inputSummary: String? = null,
        val elapsedMs: Long? = null,
        /**
         * The engine's PRE-DERIVED call header, carried straight through from
         * `ToolUseStarted.header`. Absent on an older engine, and absent on the
         * result/heartbeat events (which carry no header) — the reducer keeps the
         * one the call already delivered.
         */
        val header: ToolHeaderUi? = null,
        /**
         * The engine's PRE-DERIVED `⎿` block from `ToolUseResult.display`. This
         * is the payload the non-shell result arm used to THROW AWAY entirely.
         */
        val display: ToolResultDisplayUi? = null,
    ) : ReplyEvent

    /** Correlated shell lifecycle update rendered as an expandable terminal card. */
    data class ShellTool(val update: ShellToolUpdate) : ReplyEvent

    /** A non-terminal engine notice. */
    data class Notice(val message: String, val isError: Boolean) : ReplyEvent

    /** Incremental token accounting for the latest API call. */
    data class Usage(val usage: AgentRunUsage) : ReplyEvent

    /** A provider retry/backoff that keeps the turn alive. */
    data class Retry(
        val message: String,
        val attempt: Int,
        val maxRetries: Int,
        val delayMs: Long,
    ) : ReplyEvent

    /** The engine's pre-formatted cumulative session cost. */
    data class Cost(val formatted: String) : ReplyEvent

    /** A completed context compaction. */
    data class Compaction(
        val messagesBefore: Int,
        val messagesAfter: Int,
        val bytesSaved: Long,
    ) : ReplyEvent

    /** Current coordinator/team worker activity. */
    data class Coordinator(val activeWorkers: Int, val team: String?) : ReplyEvent

    /** Internal-only: the matching raw event already reduced and can advance. */
    data class DurableTurnReplayAcknowledged(
        val turnId: Long,
        val sequence: Long,
    ) : ReplyEvent

    /** A terminal error to surface (engine `Error`, or a build/submit failure). */
    data class Error(val message: String) : ReplyEvent

    /** The final assistant message (MessageComplete / no-stream path). */
    data class Completed(val message: Message) : ReplyEvent

    /** Terminal marker: the turn ended cleanly. The stream completes after this. */
    data object End : ReplyEvent
}

private fun shouldAwaitDurableTurnReplayAck(event: ClientEvent, reply: ReplyEvent): Boolean =
    when (event) {
        is ClientEvent.TurnStarted,
        is ClientEvent.TextDelta,
        is ClientEvent.ThinkingDelta,
        is ClientEvent.SystemNotice,
        is ClientEvent.ToolUseStarted,
        is ClientEvent.ToolHeartbeat,
        is ClientEvent.ToolUseResult,
        is ClientEvent.UsageUpdate,
        is ClientEvent.ApiRetry,
        is ClientEvent.CostUpdate,
        is ClientEvent.CompactionCompleted,
        is ClientEvent.CoordinatorStatus,
        -> reply !is ReplyEvent.End &&
            reply !is ReplyEvent.Error &&
            reply !is ReplyEvent.Completed
        else -> false
    }

/**
 * PURE mapping from one inbound engine [ClientEvent] to a [ReplyEvent], or
 * `null` to ignore listing / configuration events that ride an out-of-band
 * state path. Live thinking, tools, retries, usage and cost remain on this path
 * so Android can render the same execution progress as the CLI/TUI.
 *
 * This is deliberately a free function with NO engine / Android dependencies so
 * it is exhaustively unit-testable on the JVM (where `buildAndroidEngine` is
 * unavailable). Keep it total over the variants we render and `null`-tolerant of
 * everything else — `ClientEvent` is `#[non_exhaustive]`, so an `else` is
 * required and must mean "ignore, don't break the stream".
 */
fun clientEventToReply(
    event: ClientEvent,
    strings: ConversationStrings = DefaultConversationStrings,
): ReplyEvent? = when (event) {
    is ClientEvent.TurnStarted -> ReplyEvent.Thinking
    is ClientEvent.TextDelta -> ReplyEvent.Delta(event.text)
    is ClientEvent.ThinkingDelta -> ReplyEvent.ReasoningDelta(event.thinking)
    is ClientEvent.SystemNotice -> ReplyEvent.Notice(event.message, event.isError)
    is ClientEvent.ToolUseStarted ->
        if (isShellTool(event.tool)) {
            ReplyEvent.ShellTool(shellStarted(event.id, event.inputJson))
        } else {
            ReplyEvent.ToolActivity(
                label = strings.resolve(R.string.chat_tool_calling_label, "调用工具 %1\$s…", event.tool),
                id = event.id,
                tool = event.tool,
                status = AgentToolStatus.Running,
                // The legacy scrape stays ONLY as the older-engine fallback; when
                // `header` is present the renderer ignores it entirely.
                inputSummary = summarizeToolInput(event.inputJson),
                header = event.header?.toUi(),
            )
        }
    is ClientEvent.ToolHeartbeat ->
        if (isShellTool(event.tool)) {
            ReplyEvent.ShellTool(
                ShellToolUpdate.Heartbeat(event.id, event.elapsedMs.toLong()),
            )
        } else {
            ReplyEvent.ToolActivity(
                label = strings.resolve(R.string.chat_tool_running_label, "工具 %1\$s 运行中…", event.tool),
                id = event.id,
                tool = event.tool,
                status = AgentToolStatus.Running,
                elapsedMs = event.elapsedMs.toLong(),
            )
        }
    is ClientEvent.ToolUseResult ->
        if (isShellTool(event.tool)) {
            ReplyEvent.ShellTool(shellFinished(event.id, event.resultJson, event.isError))
        } else {
            // This arm used to DISCARD the whole payload — a completed tool call
            // rendered as one dim status line and nothing else. The engine's
            // pre-derived `display` (headline, structured diff, clamped body,
            // collapse verdict) now rides through to the renderer intact.
            ReplyEvent.ToolActivity(
                label = if (event.isError) {
                    strings.resolve(R.string.chat_tool_failed_label, "工具 %1\$s 失败", event.tool)
                } else {
                    strings.resolve(R.string.chat_tool_completed_label, "工具 %1\$s 完成", event.tool)
                },
                id = event.id,
                tool = event.tool,
                status = if (event.isError) AgentToolStatus.Failed else AgentToolStatus.Completed,
                display = event.display?.toUi(),
            )
        }
    is ClientEvent.UsageUpdate -> ReplyEvent.Usage(
        AgentRunUsage(
            inputTokens = event.inputTokens.toLong(),
            outputTokens = event.outputTokens.toLong(),
            cacheReadTokens = event.cacheReadTokens.toLong(),
            cacheCreationTokens = event.cacheCreationTokens.toLong(),
        ),
    )
    is ClientEvent.ApiRetry -> ReplyEvent.Retry(
        message = event.message,
        attempt = event.attempt.toInt(),
        maxRetries = event.maxRetries.toInt(),
        delayMs = event.delayMs.toLong(),
    )
    is ClientEvent.CostUpdate -> ReplyEvent.Cost(event.formatted)
    is ClientEvent.CompactionCompleted -> ReplyEvent.Compaction(
        messagesBefore = event.messagesBefore.toInt(),
        messagesAfter = event.messagesAfter.toInt(),
        bytesSaved = event.bytesSaved.toLong(),
    )
    is ClientEvent.CoordinatorStatus ->
        ReplyEvent.Coordinator(event.activeWorkers.toInt(), event.team)
    is ClientEvent.MessageComplete -> event.message
        ?.let { ReplyEvent.Completed(messageDtoToMessage(it, strings)) }
        ?: ReplyEvent.End
    is ClientEvent.TurnEnded -> ReplyEvent.End
    is ClientEvent.Error -> ReplyEvent.Error(
        userFacingEngineError(event.kind, event.message, strings),
    )
    else -> null // model / session / permission / listings ride out-of-band flows
}

/**
 * Lower one retained durable-turn event into the same reducer input used by the
 * live stream. The durable envelope intentionally carries JSON (rather than a
 * recursive `ClientEvent`) so this decoder stays small and forward-compatible:
 * unknown event types are ignored, while narrative/tool/terminal events needed
 * to reconstruct the visible in-flight response are restored.
 */
internal fun retainedTurnEventToReply(
    eventJson: String,
    strings: ConversationStrings = DefaultConversationStrings,
): ReplyEvent? = runCatching {
    val event = JSONObject(eventJson)
    when (event.optString("type")) {
        "turn_started" -> ReplyEvent.Thinking
        "text_delta" -> ReplyEvent.Delta(event.optString("text"))
        "thinking_delta" -> ReplyEvent.ReasoningDelta(event.optString("thinking"))
        "system_notice" -> ReplyEvent.Notice(
            message = event.optString("message"),
            isError = event.optBoolean("is_error"),
        )
        "tool_use_started" -> clientEventToReply(
            ClientEvent.ToolUseStarted(
                id = event.optString("id"),
                tool = event.optString("tool"),
                inputJson = event.optString("input_json", "{}"),
                header = null,
            ),
            strings,
        )
        "tool_heartbeat" -> clientEventToReply(
            ClientEvent.ToolHeartbeat(
                id = event.optString("id"),
                tool = event.optString("tool"),
                elapsedMs = event.optLong("elapsed_ms").coerceAtLeast(0L).toULong(),
            ),
            strings,
        )
        "tool_use_result" -> clientEventToReply(
            ClientEvent.ToolUseResult(
                id = event.optString("id"),
                tool = event.optString("tool"),
                resultJson = event.optString("result_json", "{}"),
                isError = event.optBoolean("is_error"),
                display = null,
            ),
            strings,
        )
        "error" -> ReplyEvent.Error(event.optString("message"))
        "turn_ended" -> when (event.optString("outcome")) {
            "end_turn" -> ReplyEvent.End
            else -> null
        }
        else -> null
    }
}.getOrNull()

/**
 * Convert transport diagnostics into concise, actionable mobile copy.
 *
 * The Android HTTP backend opts into reqwest's nested cause chain, so DNS,
 * timeout, and TLS failures are identifiable here. Provider responses such as
 * 401/404 are not rewritten: their original message still reaches the existing
 * auth/model error handling.
 */
internal fun userFacingEngineError(
    kind: ErrorKindDto,
    message: String,
    strings: ConversationStrings = DefaultConversationStrings,
): String {
    if (kind != ErrorKindDto.TRANSPORT) return message
    val normalized = message.lowercase()
    return when {
        listOf(
            "dns",
            "unknown host",
            "no such host",
            "failed to lookup",
            "name or service not known",
            "nodename nor servname",
        ).any(normalized::contains) ->
            strings.resolve(R.string.chat_error_dns, "无法解析模型服务地址。请检查 VPN、私人 DNS 或当前网络后重试。")

        listOf("certificate", "tls", "ssl").any(normalized::contains) ->
            strings.resolve(R.string.chat_error_tls, "模型服务安全连接失败。请检查系统时间、VPN 或证书设置后重试。")

        listOf("timeout", "timed out").any(normalized::contains) ->
            strings.resolve(R.string.chat_error_connect_timeout, "连接模型服务超时。请检查当前网络或 VPN 后重试。")

        listOf(
            "connection failed",
            "connect error",
            "error sending request",
        ).any(normalized::contains) ->
            strings.resolve(R.string.chat_error_connect_failed, "无法连接模型服务。请检查当前网络或 VPN 后重试。")

        else -> message
    }
}

/**
 * PURE flow transform: turn an inbound [ClientEvent] stream into the UI-facing
 * [ReplyEvent] stream the [ChatViewModel] reduces. Prepends a leading
 * [ReplyEvent.Thinking] (so the run trace shows the instant a turn is armed,
 * before the first engine event), maps each event through [clientEventToReply]
 * (dropping ignored ones), and COMPLETES after the first terminal reply
 * ([ReplyEvent.End] / [ReplyEvent.Error] / [ReplyEvent.Completed]) — emitting a trailing
 * [ReplyEvent.End] after an `Error` so the ViewModel always sees a clean turn
 * boundary.
 *
 * Extracted from [EngineConversationSource.submit] as a free function with NO
 * engine / Android dependency so the streaming/ordering contract is exercised on
 * the plain JVM (see `EngineReplyStreamTest`) — including the subscribe-before-
 * submit guarantee, which a flow-level test can prove without a native engine.
 */
fun mapReplyStream(
    events: Flow<ClientEvent>,
    strings: ConversationStrings = DefaultConversationStrings,
): Flow<ReplyEvent> = flow {
    emit(ReplyEvent.Thinking)
    var awaitingDurableReplayAck = false
    emitAll(
        events.transformWhile { event ->
            if (event is ClientEvent.TurnEventReplay) {
                if (awaitingDurableReplayAck) {
                    emit(
                        ReplyEvent.DurableTurnReplayAcknowledged(
                            turnId = event.turnId.toLong(),
                            sequence = event.sequence.toLong(),
                        ),
                    )
                    awaitingDurableReplayAck = false
                }
                return@transformWhile true
            }
            val reply = clientEventToReply(event, strings) ?: return@transformWhile true
            emit(reply)
            awaitingDurableReplayAck = shouldAwaitDurableTurnReplayAck(event, reply)
            val terminal =
                reply is ReplyEvent.End || reply is ReplyEvent.Error || reply is ReplyEvent.Completed
            if (reply is ReplyEvent.Error) emit(ReplyEvent.End)
            !terminal // keep collecting until a terminal reply
        },
    )
}

/**
 * Explicit no-engine source. Production uses this when the engine cannot be
 * built instead of falling back to branded mock sessions/messages.
 */
class UnavailableConversationSource(
    explicitReason: String? = null,
    strings: ConversationStrings = DefaultConversationStrings,
) : ConversationSource {
    internal val reason: String = explicitReason
        ?: strings.resolve(R.string.chat_engine_unavailable, "移动端引擎当前不可用")
    override fun submit(text: String): Flow<ReplyEvent> = flow {
        emit(ReplyEvent.Error(reason))
        emit(ReplyEvent.End)
    }

    override val sessionState: StateFlow<EngineSessionState> =
        MutableStateFlow(EngineSessionState.error(reason)).asStateFlow()

    override suspend fun resumeSession(uuid: String): Nothing =
        throw IllegalStateException(reason)

    override suspend fun newSession(): Nothing =
        throw IllegalStateException(reason)
}

internal class RecoveringConversationSource(
    private val pendingSource: kotlinx.coroutines.CompletableDeferred<EngineConversationSource?>,
    private val strings: ConversationStrings,
    override val recoverySpec: ConversationRecoverySpec,
) : ConversationSource, BackgroundRetainableConversationSource {
    private val delegateScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val models = MutableStateFlow(EngineModelState())
    private val sessions = MutableStateFlow(EngineSessionState.loading())
    private val activeSession = MutableStateFlow<ActivatedSession?>(null)
    private val permissions = MutableStateFlow<PermissionPromptState?>(null)
    private val mcp = MutableStateFlow(emptyList<MCPServer>())
    @Volatile private var retainedSource: EngineConversationSource? = null
    @Volatile private var closed = false

    init {
        delegateScope.launch {
            val delegate = pendingSource.await() ?: run {
                delegateScope.cancel()
                return@launch
            }
            if (closed) {
                delegate.close()
                delegateScope.cancel()
                return@launch
            }
            retainedSource = delegate
            launch { delegate.modelState.collect { models.value = it } }
            launch { delegate.sessionState.collect { sessions.value = it } }
            launch { delegate.activeSessionState.collect { activeSession.value = it } }
            launch { delegate.pendingPermission.collect { permissions.value = it } }
            launch { delegate.mcpServers.collect { mcp.value = it } }
        }
    }

    override val clientEvents: Flow<ClientEvent> = flow {
        pendingSource.await()?.clientEvents?.let { emitAll(it) }
    }

    override val workflowProgress: Flow<WorkflowProgressUpdate> = flow {
        pendingSource.await()?.workflowProgress?.let { emitAll(it) }
    }

    override val pendingPermission: StateFlow<PermissionPromptState?> = permissions.asStateFlow()
    override val modelState: StateFlow<EngineModelState> = models.asStateFlow()
    override val sessionState: StateFlow<EngineSessionState> = sessions.asStateFlow()
    override val activeSessionState: StateFlow<ActivatedSession?> = activeSession.asStateFlow()
    override val mcpServers: StateFlow<List<MCPServer>> = mcp.asStateFlow()

    override suspend fun submitClientCommand(command: ClientCommand) {
        pendingSource.await()?.submitClientCommand(command)
    }

    override suspend fun refreshExecutionStatus() {
        pendingSource.await()?.refreshExecutionStatus()
    }

    override suspend fun attachDurableTurnForUi(afterSequence: Long) {
        pendingSource.await()?.attachDurableTurnForUi(afterSequence)
    }

    override fun initialMessages(): List<Message> = emptyList()

    override suspend fun refreshSessions() {
        pendingSource.await()?.refreshSessions()
    }

    override suspend fun resumeSession(uuid: String) {
        pendingSource.await()?.resumeSession(uuid)
    }

    override suspend fun resumeEmptySession(uuid: String, title: String) {
        pendingSource.await()?.resumeEmptySession(uuid, title)
    }

    override suspend fun refreshMcpServers() {
        pendingSource.await()?.refreshMcpServers()
    }

    override suspend fun newSession() {
        pendingSource.await()?.newSession()
    }

    override suspend fun setPermissionMode(mode: String) {
        pendingSource.await()?.setPermissionMode(mode)
    }

    override suspend fun setModel(id: String) {
        pendingSource.await()?.setModel(id)
    }

    override suspend fun approvePermission(requestId: ULong, response: PermissionResponseDto) {
        pendingSource.await()?.approvePermission(requestId, response)
    }

    override suspend fun denyPermission(requestId: ULong) {
        pendingSource.await()?.denyPermission(requestId)
    }

    override fun submit(text: String, images: List<ImageRefDto>, turnId: Long): Flow<ReplyEvent> = flow {
        val delegate = pendingSource.await()
        if (delegate == null) {
            emit(ReplyEvent.Error(strings.resolve(R.string.chat_engine_build_failed, "引擎创建失败")))
            emit(ReplyEvent.End)
            return@flow
        }
        emitAll(delegate.submit(text, images, turnId))
    }

    override suspend fun cancel(turnId: Long?) {
        pendingSource.await()?.cancel(turnId)
    }

    override suspend fun discardDurableTurn(turnId: Long) {
        pendingSource.await()?.discardDurableTurn(turnId)
    }

    override fun close() {
        closed = true
        retainedSource?.let {
            it.close()
            delegateScope.cancel()
        }
    }

    override fun engineSourceForBackgroundRetention(): EngineConversationSource? = retainedSource
}

/**
 * The real conversation source: an in-process engine reached over UniFFI.
 *
 * Owns the single [MobileEngineHandle] (built once via [buildVoiceEngine], which
 * wires every device-capability adapter through the FFI seam) and the single
 * registered `AndroidEventListener`. The listener pushes every inbound
 * [ClientEvent] into [events]; [submit] fires the `SendPrompt` command and
 * returns a [Flow] that maps the shared stream through [clientEventToReply],
 * completing on [ReplyEvent.End] / [ReplyEvent.Error].
 *
 * The handle is built eagerly in the constructor (the factory below catches a
 * build failure and falls back to the mock, mirroring iOS) so this type is only
 * ever instantiated when the engine is actually available. The listener is
 * passed INTO [buildVoiceEngine] so the source is the sole owner of the
 * handle+listener — no second build, no dropped events.
 *
 * NOTE: this type touches the UniFFI bindings + Android `Context`, so it is NOT
 * exercised by JVM unit tests. The pure [clientEventToReply] mapper carries the
 * mapping coverage; this wiring is covered by the on-device integration build.
 */
class EngineConversationSource private constructor(
    private val handle: MobileEngineHandle,
    private val events: SharedFlow<ClientEvent>,
    private val eventRelay: LosslessEventRelay<ClientEvent>,
    private val workflowEvents: SharedFlow<WorkflowProgressUpdate>,
    private val workflowRelay: LosslessEventRelay<WorkflowProgressUpdate>,
    private val eventScope: CoroutineScope,
    private val permissions: MutableStateFlow<PermissionPromptState?>,
    private val permissionIngress: PermissionIngress,
    private val models: MutableStateFlow<EngineModelState>,
    private val sessions: MutableStateFlow<EngineSessionState>,
    private val activeSession: MutableStateFlow<ActivatedSession?>,
    private val mcp: MutableStateFlow<List<MCPServer>>,
    private val strings: ConversationStrings,
    private val durableTurns: DurableConversationTurnClientStore,
    private val durableReplayGate: DurableTurnReplayGate,
    private val autoAttachDurableTurns: Boolean,
    @Volatile private var resumeOnUiAttach: Boolean,
    override val recoverySpec: ConversationRecoverySpec,
) : ConversationSource, BackgroundRetainableConversationSource {

    private val durableAttachMutex = Mutex()
    private val durableAttachCoordinator = DurableAttachCoordinator()

    init {
        if (autoAttachDurableTurns) {
            eventScope.launch {
                activeSession.collect { activated ->
                    val record = durableTurns.load() ?: return@collect
                    if (
                        activated != null &&
                        canonicalSessionId(activated.sessionId) == canonicalSessionId(record.sessionId)
                    ) {
                        attachAndResume(record, forceAttach = false)
                    }
                }
            }
        }
    }

    /** UI takes over attachment, while the service remains a passive observer. */
    internal fun claimDurableTurnForUi() {
        durableAttachCoordinator.claimForUi()
    }

    /**
     * Update whether a UI Attach must also Resume. This is changed by the
     * process coordinator when ownership moves between a retained headless
     * executor and a fresh UI source; the immutable construction path alone
     * cannot distinguish those lifecycles.
     */
    internal fun setUiAttachResumeRequired(required: Boolean) {
        resumeOnUiAttach = required
    }

    /** Allow service redelivery to resume ownership after the UI is destroyed. */
    internal fun releaseDurableTurnToHeadless() {
        durableAttachCoordinator.releaseToHeadless()
    }

    override val clientEvents: Flow<ClientEvent> = events
    override val workflowProgress: Flow<WorkflowProgressUpdate> = workflowEvents

    override suspend fun submitClientCommand(command: ClientCommand) {
        handle.submit(command)
    }

    override suspend fun refreshExecutionStatus() {
        handle.submit(ClientCommand.TaskList(null))
        handle.submit(ClientCommand.ListSessionAgents)
    }

    override suspend fun attachDurableTurnForUi(afterSequence: Long) {
        val record = durableTurns.load() ?: return
        if (
            activeSession.value?.let { canonicalSessionId(it.sessionId) } ==
                canonicalSessionId(record.sessionId)
        ) {
            attachAndResume(record, forceAttach = true, afterSequence = afterSequence)
        }
    }

    private suspend fun attachAndResume(
        record: DurableConversationTurnRecord,
        forceAttach: Boolean,
        afterSequence: Long = 0L,
    ) = durableAttachMutex.withLock {
        val latest = durableTurns.load()?.takeIf { it.turnId == record.turnId } ?: record
        // A second Activity can claim the shared source while AttachTurn is
        // suspended. Snapshot the first claim's disposition before yielding;
        // that claim must still consume its required Resume exactly once.
        val resumeForThisAttach = resumeOnUiAttach
        val owner = if (forceAttach) DurableAttachOwner.Ui else DurableAttachOwner.Headless
        val request = DurableAttachRequest(
            sessionId = canonicalSessionId(latest.sessionId),
            turnId = latest.turnId,
            afterSequence = afterSequence.coerceAtLeast(0L),
        )
        if (!durableAttachCoordinator.reserve(request, owner)) return@withLock
        durableReplayGate.noteAttachRequested(
            turnId = latest.turnId,
            activation = activeSession.value?.takeIf { canonicalSessionId(it.sessionId) == canonicalSessionId(latest.sessionId) },
            afterSequence = request.afterSequence,
        )
        try {
            handle.submit(
                ClientCommand.AttachTurn(
                    turnId = latest.turnId.toULong(),
                    afterSequence = request.afterSequence.toULong(),
                ),
            )
        } catch (error: Throwable) {
            durableReplayGate.cancelAttach(latest.turnId)
            durableAttachCoordinator.rollback(request, owner)
            ConversationHeadlessRecovery.markDurableAttachFailed(recoverySpec.scopeKey, this)
            throw DurableAttachFailure(latest.turnId, "attach", error)
        }
        // Cold-process recovery needs ResumeTurn. A UI takeover resumes only
        // when the process coordinator reports that no headless/UI executor is
        // already attached to this source; retained-live sources only Attach.
        if (shouldResumeDurableTurn(forceAttach, resumeForThisAttach)) {
            try {
                handle.submit(ClientCommand.ResumeTurn(latest.turnId.toULong()))
            } catch (error: Throwable) {
                durableReplayGate.cancelAttach(latest.turnId)
                durableAttachCoordinator.rollback(request, owner)
                ConversationHeadlessRecovery.markDurableAttachFailed(recoverySpec.scopeKey, this)
                throw DurableAttachFailure(latest.turnId, "resume", error)
            }
            if (forceAttach) {
                ConversationHeadlessRecovery.markDurableUiExecutorActive(
                    recoverySpec.scopeKey,
                    this,
                )
            }
        }
    }

    override suspend fun setPermissionMode(mode: String) {
        require(mode in com.lingxi.code.settings.PermissionModeOptions.values)
        handle.submit(ClientCommand.SetPermissionMode(mode))
    }

    /** A fresh engine session starts empty (the engine streams the transcript). */
    override fun initialMessages(): List<Message> = emptyList()

    /**
     * The engine's REAL model catalog + active id, driven OUT-OF-BAND by the
     * listener folding `ModelList` / `ModelChanged` through [reduceModelEvent]
     * (see [create]). The picker observes this; [setModel] confirms a pick.
     */
    override val modelState: StateFlow<EngineModelState> = models.asStateFlow()

    /**
     * The engine's REAL resumable-session catalog, driven OUT-OF-BAND by the
     * listener folding `SessionList` through [reduceSessionEvent] (see [create]).
     * The drawer observes this to render real history; [resumeSession] /
     * [newSession] act on a pick.
     */
    override val sessionState: StateFlow<EngineSessionState> = sessions.asStateFlow()

    /**
     * The engine's most recent active-session transition, driven OUT-OF-BAND by
     * the listener folding `SessionStarted` / `SessionResumed` through
     * [sessionActivationFrom] (see [create]). The ViewModel observes this to
     * adopt the real session id and, on resume, replace the transcript with the
     * rehydrated conversation. Sibling of [sessionState] (the catalog); both
     * ride the same listener, neither the per-turn stream.
     */
    override val activeSessionState: StateFlow<ActivatedSession?> = activeSession.asStateFlow()

    override val mcpServers: StateFlow<List<MCPServer>> = mcp.asStateFlow()

    override suspend fun refreshMcpServers() {
        try {
            handle.submit(ClientCommand.RefreshListings(which = listOf(ListingKindDto.MCP)))
        } catch (_: Throwable) {
            // A RefreshListings that can't be delivered leaves the MCP list as-is;
            // the settings page keeps whatever it last rendered (mock if empty).
        }
    }

    override suspend fun refreshSessions() {
        try {
            handle.submit(ClientCommand.ListSessions(limit = null))
        } catch (t: Throwable) {
            sessions.value = EngineSessionState.error(
                strings.resolve(
                    R.string.chat_error_session_list_failed,
                    "会话列表加载失败：%1\$s",
                    "${t.message ?: t::class.simpleName}",
                ),
            )
        }
    }

    override suspend fun resumeSession(uuid: String) {
        if (uuid.isBlank()) return
        val record = durableTurns.load()
        if (record != null && canonicalSessionId(uuid) != canonicalSessionId(record.sessionId)) {
            durableTurns.clear()
        }
        // Propagate command failures: the ViewModel must keep the composer gated
        // and surface an explicit session error instead of pretending the locally
        // selected transcript was resumed.
        handle.submit(
            ClientCommand.ResumeSession(
                sessionId = canonicalSessionId(uuid),
                cwd = null,
            ),
        )
    }

    override suspend fun resumeEmptySession(uuid: String, title: String) {
        if (uuid.isBlank()) return
        handle.resumeEmptySession(
            sessionId = canonicalSessionId(uuid),
            title = title.ifBlank { strings.resolve(R.string.chat_new_conversation, "新对话") },
        )
    }

    override suspend fun newSession() {
        durableTurns.clear()
        handle.submit(ClientCommand.NewSession(cwd = null, model = null))
    }

    override suspend fun setModel(id: String) {
        if (id.isBlank()) return
        try {
            handle.submit(ClientCommand.SetModel(model = id))
        } catch (_: Throwable) {
            // A SetModel that can't be delivered leaves the active id as-is; the
            // engine never emits ModelChanged, so the picker reverts to whatever
            // the engine last reported (no optimistic local mutation).
        }
    }

    /**
     * The head parked permission request, driven by the engine's outbound
     * `AndroidPermissionSink.onRequest` (registered in [create]). The UI observes
     * this and resolves it via [approvePermission] / [denyPermission].
     */
    override val pendingPermission: StateFlow<PermissionPromptState?> =
        permissions.asStateFlow()

    override suspend fun approvePermission(
        requestId: ULong,
        response: PermissionResponseDto,
    ) {
        submitPermissionResolution {
            handle.submit(ClientCommand.ApprovePermission(requestId = requestId, response = response))
        }
    }

    override suspend fun denyPermission(requestId: ULong) {
        submitPermissionResolution {
            handle.submit(ClientCommand.DenyPermission(requestId = requestId))
        }
    }

    /**
     * Submit the user's decision without speculatively mutating the queue. The
     * engine emits `PermissionRequestResolved` for approved, denied, cancelled,
     * and expired gates; that correlated event is the sole dequeue authority.
     * A delivery failure deliberately leaves the prompt available for retry.
     */
    private inline fun submitPermissionResolution(
        submit: () -> Unit,
    ) {
        try {
            submit()
        } catch (_: Throwable) {
            // Keep the request visible. The engine may still resolve it later,
            // otherwise the user can retry after reconnecting.
        }
    }

    override fun submit(text: String, images: List<ImageRefDto>): Flow<ReplyEvent> =
        submit(text, images, nextFallbackTurnId())

    override fun submit(text: String, images: List<ImageRefDto>, turnId: Long): Flow<ReplyEvent> =
        // Subscribe-before-submit: the returned reply stream maps the shared
        // engine flow through `mapReplyStream`, but the `SendPrompt` is fired
        // from `events.onSubscription { … }` — which runs ONLY AFTER this
        // collector is registered as a subscriber of the SharedFlow. That ordering
        // closes the race the prior `flow { submit(); emitAll(events…) }` had: a
        // `TextDelta` emitted by the engine's listener thread in the window between
        // `submit` returning and the collector subscribing is no longer dropped,
        // because the collector is already subscribed before the turn is spawned.
        mapReplyStream(
            events.onSubscription {
                try {
                    permissionIngress.beginTurn()
                    activeSession.value?.sessionId?.let { sessionId ->
                        durableTurns.begin(sessionId, turnId)
                    }
                    handle.submit(
                        ClientCommand.SendPrompt(
                            text = text,
                            promptMode = null,
                            images = images,
                            turnId = turnId.toULong(),
                        ),
                    )
                } catch (t: Throwable) {
                    permissionIngress.endTurn()
                    // Inject the build/submit failure into the same stream the
                    // collector is already reading, so the mapper terminates it.
                    emit(
                        ClientEvent.Error(
                            kind = ErrorKindDto.TRANSPORT,
                            message = strings.resolve(
                                R.string.chat_error_engine_submit_failed,
                                "引擎错误：%1\$s",
                                "${t.message ?: t::class.simpleName}",
                            ),
                        ),
                    )
                }
            },
            strings,
        )

    override suspend fun cancel() {
        cancel(null)
    }

    override suspend fun cancel(turnId: Long?) {
        // Narrow `Cancel(turnId = null)` cancels the current turn (bindings doc:
        // "None cancels the current one"). The engine emits `TurnEnded`, which
        // flows back through the active `submit` stream as `ReplyEvent.End`.
        // Do not clear permission UI speculatively here: a background workflow
        // child can own the prompt while the main turn is being cancelled. The
        // correlated PermissionRequestResolved event is the only authority that
        // removes a parked request.
        // The host intentionally returns Ok for a stale id or when no turn is
        // active.  A successful command submission therefore is not evidence
        // that this durable checkpoint was cancelled.  Keep the client record
        // until its correlated terminal TurnRecoveryState arrives.
        submitTurnCancellation(permissionIngress) {
            handle.submit(ClientCommand.Cancel(turnId = turnId?.toULong()))
        }
    }

    override suspend fun discardDurableTurn(turnId: Long) {
        cancel(turnId)
    }

    override fun close() {
        ConversationHeadlessRecovery.unregister(recoverySpec.scopeKey, this)
        eventRelay.close()
        workflowRelay.close()
        eventScope.cancel()
        runCatching { handle.destroy() }
    }

    override fun engineSourceForBackgroundRetention(): EngineConversationSource = this

    companion object {
        private val fallbackTurnIds = java.util.concurrent.atomic.AtomicLong(
            (System.currentTimeMillis() * 1_000L).coerceAtLeast(1L),
        )

        private fun nextFallbackTurnId(): Long = fallbackTurnIds.incrementAndGet()

        /**
         * Build the engine + register the event-bridging listener, or return an
         * explicit unavailable source when the engine is not usable.
         */
        fun create(
            context: Context,
            projectWorkspace: ProjectWorkspace? = null,
            linuxRuntimeMode: LinuxRuntimeMode = LinuxRuntimeMode.Legacy,
            reuseProcessSource: Boolean = true,
        ): ConversationSource {
            val recoverySpec = ConversationRecoverySpec(
                projectId = projectWorkspace?.projectId,
                hostPath = projectWorkspace?.hostPath,
                linuxRuntimeMode = linuxRuntimeMode,
            )
            if (reuseProcessSource) {
                when (
                    val claim = ConversationHeadlessRecovery.acquireForUi(
                        recoverySpec = recoverySpec,
                        strings = conversationStrings(context),
                    )
                ) {
                    is ConversationHeadlessRecovery.UiSourceClaim.Existing -> return claim.source
                    is ConversationHeadlessRecovery.UiSourceClaim.Pending -> return claim.source
                    ConversationHeadlessRecovery.UiSourceClaim.Build -> Unit
                }
            }
            // Callback ingress is non-blocking and lossless. A dedicated pump may
            // suspend behind a slow collector without ever stalling Rust's event
            // callback or dropping assistant text / terminal events.
            val eventScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            val eventRelay = LosslessEventRelay<ClientEvent>(eventScope)
            val workflowRelay = LosslessEventRelay<WorkflowProgressUpdate>(eventScope)
            // Resolves user-facing copy in the app's actual selected language
            // (via Context.getString, so it honors AppLanguageStore's locale
            // wrap) for every non-Composable emission site below.
            val strings = conversationStrings(context)
            val durableTurns = DurableConversationTurnClientStore(
                context = context,
                scope = projectWorkspace?.hostPath ?: "__global__",
            )
            val durableReplayGate = DurableTurnReplayGate()
            // The head parked permission request. The engine's outbound
            // `AndroidPermissionSink.onRequest` pushes each request here (mapped
            // to the UI render model); PermissionIngress retains all concurrent
            // workflow-child requests FIFO and advances this StateFlow when the
            // engine emits the correlated resolved event.
            val permissions = MutableStateFlow<PermissionPromptState?>(null)
            val permissionIngress = PermissionIngress(permissions, strings)
            // The engine's REAL model catalog + active id (SHIP-BLOCKER #2). The
            // listener below folds every inbound `ModelList` / `ModelChanged`
            // into this StateFlow via the pure `reduceModelEvent`, so the picker
            // is driven by real wire ids out-of-band from the per-turn stream.
            val models = MutableStateFlow(EngineModelState())
            // The engine's REAL resumable-session catalog (sibling of `models`).
            // The listener below folds every inbound `SessionList` into this
            // StateFlow via the pure `reduceSessionEvent`, so the drawer is driven
            // by real history out-of-band from the per-turn stream.
            val sessions = MutableStateFlow(EngineSessionState.loading())
            // The engine's most recent active-session transition (sibling of
            // `sessions`). The listener below folds both `SessionStarted` and
            // `SessionResumed` into this StateFlow so the ViewModel adopts the
            // real session id and any restored transcript.
            val activeSession = MutableStateFlow<ActivatedSession?>(null)
            // The engine's REAL MCP listing (sibling of `models`). The listener
            // folds every inbound `McpServers` into this StateFlow; empty until a
            // `RefreshListings(Mcp)` reply lands (settings shows the mock list).
            val mcp = MutableStateFlow(emptyList<MCPServer>())
            // Credentials: the encrypted-at-rest SecureKeyStore FIRST (the shipped
            // app's source of truth — SHIP-BLOCKER #1), falling back to the process
            // environment as a dev override. A shipped mobile app has no process
            // env, so the key normally comes from the secure store the Settings
            // screen writes; ANTHROPIC_API_KEY only ever overrides on a dev host.
            val store = SecureKeyStore.create(context)
            val creds = resolveEngineCredentials(
                storedKey = store?.apiKey() ?: "",
                storedBase = store?.apiBase() ?: "",
                env = System.getenv(),
            )
            val providerLaunch = ProviderSettingsRepository(context).engineLaunchConfig()
            val handle = buildVoiceEngine(
                context = context,
                apiBase = creds.apiBase,
                apiKey = creds.apiKey,
                // Empty `creds.model` → the engine starts on MobileConfig.default_model
                // (a real Anthropic wire id), never a branded `lx-*` mock id. A
                // dev-set LINGXI_MODEL still overrides; the secure store doesn't
                // persist a model, so a fresh install always uses the real default.
                model = creds.model.ifBlank { providerLaunch.defaultModel },
                providerProfilesJson = providerLaunch.providerProfilesJson,
                routingJson = providerLaunch.routingJson,
                visionDelegationEnabled = providerLaunch.visionDelegationEnabled,
                projectWorkspace = projectWorkspace,
                linuxRuntimeMode = linuxRuntimeMode,
                onEvent = { event ->
                    // The OUT-OF-BAND state paths: fold model catalog + session
                    // catalog + live-resume events into the StateFlows the picker /
                    // drawer / conversation observe, BEFORE forwarding to the
                    // per-turn stream. Every reducer is a no-op for unrelated
                    // events, so every event still reaches `events` unchanged
                    // (lifecycle events like SessionStarted ride the per-turn
                    // stream; the ViewModel acts on them there).
                    models.value = reduceModelEvent(models.value, event)
                    sessions.value = reduceSessionEvent(sessions.value, event, strings = sessionCatalogStrings(context))
                    sessionActivationFrom(event, strings)?.let { activeSession.value = it }
                    // Out-of-band MCP listing: fold `McpServers` into its StateFlow.
                    if (event is ClientEvent.McpServers) mcp.value = event.servers.map { it.toMcpServer() }
                    if (event is ClientEvent.TurnStarted) permissionIngress.confirmTurnStarted()
                    if (event is ClientEvent.PermissionRequestResolved) {
                        permissionIngress.resolve(event.requestId)
                    }
                    if (
                        event is ClientEvent.TurnRecoveryState &&
                            event.snapshot.state in setOf(
                                TurnRecoveryStateDto.COMPLETED,
                                TurnRecoveryStateDto.FAILED,
                                TurnRecoveryStateDto.CANCELLED,
                            )
                    ) {
                        durableTurns.clear(
                            sessionId = event.snapshot.sessionId,
                            turnId = event.snapshot.turnId.toLong(),
                        )
                    }
                    if (event is ClientEvent.TurnEnded || event is ClientEvent.Error) {
                        permissionIngress.endTurn()
                    }
                    if (durableReplayGate.shouldForward(event)) {
                        eventRelay.offer(event)
                    }
                },
                onWorkflowProgress = { originSessionId, taskId, runId, progress ->
                    workflowRelay.offer(
                        WorkflowProgressUpdate(
                            originSessionId = originSessionId,
                            taskId = taskId,
                            runId = runId,
                            progress = progress,
                        ),
                    )
                },
                onPermission = { request ->
                    permissionIngress.publish(request)
                },
            ) ?: run {
                if (reuseProcessSource) {
                    ConversationHeadlessRecovery.releaseUiReservation(recoverySpec.scopeKey)
                }
                eventRelay.close()
                workflowRelay.close()
                eventScope.cancel()
                return UnavailableConversationSource(
                    context.getString(R.string.chat_engine_build_failed),
                )
            }
            // Ask the engine for its REAL catalog now that the handle exists; the
            // reply (`ModelList`) flows back through the listener above into the
            // `models` StateFlow, populating the picker with real wire ids.
            // `handle.submit` is suspend, so fire it off the calling thread — a
            // failed ListModels leaves the catalog explicitly empty; it never
            // blocks building the source.
            CoroutineScope(Dispatchers.Default).launch {
                try {
                    handle.submit(ClientCommand.ListModels)
                } catch (_: Throwable) {
                    // benign: the picker keeps its current selection until a
                    // later catalog refresh succeeds.
                }
                // Same out-of-band priming for the session catalog: the reply
                // (`SessionList`) flows back through the listener into `sessions`,
                // populating the drawer with real history.
                try {
                    handle.submit(ClientCommand.ListSessions(limit = null))
                } catch (t: Throwable) {
                    sessions.value = EngineSessionState.error(
                        strings.resolve(
                            R.string.chat_error_session_list_failed,
                            "会话列表加载失败：%1\$s",
                            "${t.message ?: t::class.simpleName}",
                        ),
                    )
                }
                // Prime the unified execution card on cold start as well as
                // after lifecycle resume. Task rows and session-agent rows are
                // independent listing replies and may arrive in either order.
                try {
                    handle.submit(ClientCommand.TaskList(statusFilter = null))
                    handle.submit(ClientCommand.ListSessionAgents)
                } catch (_: Throwable) {
                    // Status is best-effort; a later foreground refresh retries.
                }
            }
            val source = EngineConversationSource(
                handle = handle,
                events = eventRelay.events,
                eventRelay = eventRelay,
                workflowEvents = workflowRelay.events,
                workflowRelay = workflowRelay,
                eventScope = eventScope,
                permissions = permissions,
                permissionIngress = permissionIngress,
                models = models,
                sessions = sessions,
                activeSession = activeSession,
                mcp = mcp,
                strings = strings,
                durableTurns = durableTurns,
                durableReplayGate = durableReplayGate,
                autoAttachDurableTurns = !reuseProcessSource,
                resumeOnUiAttach = reuseProcessSource,
                recoverySpec = recoverySpec,
            )
            ConversationHeadlessRecovery.register(recoverySpec.scopeKey, source)
            return source
        }
    }
}
