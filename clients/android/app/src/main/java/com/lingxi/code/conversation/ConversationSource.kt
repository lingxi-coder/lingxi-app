package com.lingxi.code.conversation

import com.lingxi.code.R
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.ImageRefDto
import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.bindings.PermissionRequest
import com.lingxi.code.bindings.PermissionResponseDto
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.Message
import com.lingxi.code.voice.buildVoiceEngine
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.flow.flow

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
