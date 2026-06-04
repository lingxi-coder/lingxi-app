package com.lingxi.code.conversation

import android.content.Context
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.ErrorKindDto
import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.bindings.PermissionRequest
import com.lingxi.code.bindings.PermissionResponseDto
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.Message
import com.lingxi.code.model.MockData
import com.lingxi.code.model.SessionCatalog
import com.lingxi.code.secure.SecureKeyStore
import com.lingxi.code.secure.resolveEngineCredentials
import com.lingxi.code.voice.buildVoiceEngine
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.flow.transformWhile

/**
 * The seam between the [ChatViewModel] and whatever produces turns. The
 * ViewModel talks ONLY to a [ConversationSource] — it never reaches into mock
 * data, a network, or the engine handle directly. Two implementations back it:
 *
 *  - [MockConversationSource]   — the prior canned behavior (no engine).
 *  - [EngineConversationSource] — the real in-process engine over UniFFI: owns
 *    a [MobileEngineHandle] built via [buildVoiceEngine], registers an
 *    `AndroidEventListener` whose `onEvent` pushes each inbound [ClientEvent]
 *    into an internal flow, and drives turns via
 *    `handle.submit(ClientCommand.SendPrompt(...))`.
 *
 * This mirrors the iOS `ConversationSource` seam (clients/ios → ConversationSource.swift):
 * one protocol (`client-protocol`), one transport (UniFFI), one renderer, with a
 * graceful fall back to the mock when the engine can't build.
 */
interface ConversationSource {

    /** The conversation a freshly-opened session starts with. */
    fun initialMessages(): List<Message>

    /**
     * Submit a user turn and observe the assistant's reply as a stream of
     * [ReplyEvent]s. The mock emits a single [ReplyEvent.Thinking] then a
     * [ReplyEvent.Completed]; the engine emits incremental [ReplyEvent.Delta]s
     * (plus [ReplyEvent.Thinking] / [ReplyEvent.ToolActivity]) and terminates on
     * [ReplyEvent.End] or [ReplyEvent.Error].
     */
    fun submit(text: String): Flow<ReplyEvent>

    /**
     * Cancel the in-flight turn (the composer's Stop affordance). Fires the
     * engine's `Cancel` command so the streaming turn terminates promptly; the
     * resulting `TurnEnded` flows back through [submit]'s stream as a normal
     * [ReplyEvent.End]. A no-op for sources with no cancellable turn (the mock).
     */
    suspend fun cancel() {}

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
     * [submit]'s per-turn [ReplyEvent] stream. Empty for the mock (the UI then
     * keeps showing [MockData.models]); the engine populates it after the
     * `ListModels` submitted at build time replies.
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
     * Empty for the mock (the drawer then keeps showing its [MockData] session
     * list); the engine populates it after the `ListSessions` submitted at build
     * time replies. The UI observes this to render real history; an empty state
     * means "mock mode / no catalog yet".
     */
    val sessionState: StateFlow<EngineSessionState>
        get() = MutableStateFlow(EngineSessionState()).asStateFlow()

    /**
     * Ask the engine to (re)report its resumable-session catalog, submitting
     * `ClientCommand.ListSessions`. The reply (`SessionList`) updates
     * [sessionState] out-of-band. Called when the drawer opens so the list is
     * fresh. A no-op for sources with no engine (the mock keeps [MockData]).
     */
    suspend fun refreshSessions() {}

    /**
     * Resume the engine session named by the REAL wire [uuid], submitting
     * `ClientCommand.ResumeSession`. The engine confirms with `SessionResumed`;
     * the transcript then streams from the resumed session. A no-op for sources
     * with no engine (the mock keeps its local selection).
     */
    suspend fun resumeSession(uuid: String) {}

    /**
     * Start a fresh engine session, submitting `ClientCommand.NewSession`. The
     * engine confirms with `SessionStarted`; the UI resets its transcript on
     * that event. A no-op for sources with no engine (the mock).
     */
    suspend fun newSession() {}
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
        is ClientEvent.ModelList -> EngineModelState(available = event.models, active = event.current)
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
 * they are intentionally ignored here. [nowEpochSeconds] is injected so the
 * relative-time bucketing is deterministic in unit tests.
 */
fun reduceSessionEvent(
    prev: EngineSessionState,
    event: ClientEvent,
    nowEpochSeconds: Long = System.currentTimeMillis() / 1000L,
): EngineSessionState =
    when (event) {
        is ClientEvent.SessionList -> EngineSessionState(
            rows = event.sessions.map { dto ->
                SessionCatalog.rowFrom(
                    uuid = dto.uuid,
                    title = dto.title,
                    messageCount = dto.messageCount.toInt(),
                    modifiedRfc3339 = dto.modifiedRfc3339,
                    nowEpochSeconds = nowEpochSeconds,
                )
            },
        )
        else -> prev
    }

/**
 * Streamed assistant-reply events — the UI-facing analog of engine
 * [ClientEvent]s. [clientEventToReply] maps the wire events onto these; the
 * [ChatViewModel] reduces them into [ChatState].
 */
sealed interface ReplyEvent {
    /** The model is "thinking" — render the pulsing dots row. */
    data object Thinking : ReplyEvent

    /** An incremental assistant-text delta (the engine's streamed tokens). */
    data class Delta(val text: String) : ReplyEvent

    /** Tool activity worth surfacing in the status row (start / result / failure). */
    data class ToolActivity(val label: String) : ReplyEvent

    /** A terminal error to surface (engine `Error`, or a build/submit failure). */
    data class Error(val message: String) : ReplyEvent

    /** The final assistant message (mock path — carries the whole reply at once). */
    data class Completed(val message: Message) : ReplyEvent

    /** Terminal marker: the turn ended cleanly. The stream completes after this. */
    data object End : ReplyEvent
}

/**
 * PURE mapping from one inbound engine [ClientEvent] to a [ReplyEvent], or
 * `null` to ignore (cost / listing / message-boundary events the conversation
 * surface doesn't render). Mirrors the iOS `EngineConversationSource.apply(_:)`
 * switch (clients/ios → ConversationSource.swift).
 *
 * This is deliberately a free function with NO engine / Android dependencies so
 * it is exhaustively unit-testable on the JVM (where `buildAndroidEngine` is
 * unavailable). Keep it total over the variants we render and `null`-tolerant of
 * everything else — `ClientEvent` is `#[non_exhaustive]`, so an `else` is
 * required and must mean "ignore, don't break the stream".
 */
fun clientEventToReply(event: ClientEvent): ReplyEvent? = when (event) {
    is ClientEvent.TurnStarted -> ReplyEvent.Thinking
    is ClientEvent.TextDelta -> ReplyEvent.Delta(event.text)
    is ClientEvent.ThinkingDelta -> ReplyEvent.Thinking
    is ClientEvent.ToolUseStarted -> ReplyEvent.ToolActivity("调用工具 ${event.tool}…")
    is ClientEvent.ToolUseResult ->
        if (event.isError) ReplyEvent.ToolActivity("工具 ${event.tool} 失败")
        else ReplyEvent.ToolActivity("工具 ${event.tool} 完成")
    is ClientEvent.TurnEnded -> ReplyEvent.End
    is ClientEvent.Error -> ReplyEvent.Error(event.message)
    else -> null // cost / usage / model / message-boundary / listings — ignored
}

/**
 * PURE flow transform: turn an inbound [ClientEvent] stream into the UI-facing
 * [ReplyEvent] stream the [ChatViewModel] reduces. Prepends a leading
 * [ReplyEvent.Thinking] (so the dots row shows the instant a turn is armed,
 * before the first engine event), maps each event through [clientEventToReply]
 * (dropping ignored ones), and COMPLETES after the first terminal reply
 * ([ReplyEvent.End] / [ReplyEvent.Error]) — emitting a trailing
 * [ReplyEvent.End] after an `Error` so the ViewModel always sees a clean turn
 * boundary.
 *
 * Extracted from [EngineConversationSource.submit] as a free function with NO
 * engine / Android dependency so the streaming/ordering contract is exercised on
 * the plain JVM (see `EngineReplyStreamTest`) — including the subscribe-before-
 * submit guarantee, which a flow-level test can prove without a native engine.
 */
fun mapReplyStream(events: Flow<ClientEvent>): Flow<ReplyEvent> = flow {
    emit(ReplyEvent.Thinking)
    emitAll(
        events.transformWhile { event ->
            val reply = clientEventToReply(event) ?: return@transformWhile true
            emit(reply)
            val terminal = reply is ReplyEvent.End || reply is ReplyEvent.Error
            if (reply is ReplyEvent.Error) emit(ReplyEvent.End)
            !terminal // keep collecting until a terminal reply
        },
    )
}

/**
 * The shell's mock source: starts from [MockData.messagesDefault] and answers
 * every turn with the same canned reply after a 1.1s "thinking" beat — matching
 * the iOS `MockConversationSource.send` simulation exactly.
 */
class MockConversationSource : ConversationSource {

    override fun initialMessages(): List<Message> = MockData.messagesDefault

    override fun submit(text: String): Flow<ReplyEvent> = flow {
        emit(ReplyEvent.Thinking)
        delay(1100)
        emit(
            ReplyEvent.Completed(
                Message(role = com.lingxi.code.model.Role.Ai, tag = "思考了 8 秒", text = "已记入。继续追问。"),
            ),
        )
    }
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
    private val events: MutableSharedFlow<ClientEvent>,
    private val permissions: MutableStateFlow<PermissionPromptState?>,
    private val models: MutableStateFlow<EngineModelState>,
    private val sessions: MutableStateFlow<EngineSessionState>,
) : ConversationSource {

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

    override suspend fun refreshSessions() {
        try {
            handle.submit(ClientCommand.ListSessions(limit = null))
        } catch (_: Throwable) {
            // A ListSessions that can't be delivered leaves the catalog as-is;
            // the drawer keeps whatever it last rendered (mock list if empty).
        }
    }

    override suspend fun resumeSession(uuid: String) {
        if (uuid.isBlank()) return
        try {
            handle.submit(ClientCommand.ResumeSession(sessionId = uuid, cwd = null))
        } catch (_: Throwable) {
            // A ResumeSession that can't be delivered leaves the active session
            // unchanged; the engine never emits SessionResumed, so the UI keeps
            // whatever it locally selected (no optimistic transcript swap).
        }
    }

    override suspend fun newSession() {
        try {
            handle.submit(ClientCommand.NewSession(cwd = null, model = null))
        } catch (_: Throwable) {
            // A NewSession that can't be delivered leaves the current session in
            // place; the engine never emits SessionStarted, so the UI keeps its
            // transcript (the local reset still ran for snappiness — see ViewModel).
        }
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
        resolvePermission(requestId) {
            handle.submit(ClientCommand.ApprovePermission(requestId = requestId, response = response))
        }
    }

    override suspend fun denyPermission(requestId: ULong) {
        resolvePermission(requestId) {
            handle.submit(ClientCommand.DenyPermission(requestId = requestId))
        }
    }

    /**
     * Run [submit] to resolve the parked request `requestId`, then clear the
     * pending prompt (only when it is still the request we resolved — a CAS-style
     * guard so a fast follow-up request isn't dismissed). A submit failure (no
     * such parked request) still clears the prompt so the UI never wedges.
     */
    private suspend inline fun resolvePermission(
        requestId: ULong,
        submit: () -> Unit,
    ) {
        try {
            submit()
        } catch (_: Throwable) {
            // The gate may have already unwound (cancel / timeout); dropping the
            // prompt below keeps the UI consistent regardless.
        }
        permissions.compareAndSet(
            expect = permissions.value?.takeIf { it.requestId == requestId },
            update = null,
        )
    }

    override fun submit(text: String): Flow<ReplyEvent> =
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
                    handle.submit(
                        ClientCommand.SendPrompt(
                            text = text, promptMode = null, images = emptyList(), turnId = null,
                        ),
                    )
                } catch (t: Throwable) {
                    // Inject the build/submit failure into the same stream the
                    // collector is already reading, so the mapper terminates it.
                    emit(
                        ClientEvent.Error(
                            kind = ErrorKindDto.TRANSPORT,
                            message = "引擎错误：${t.message ?: t::class.simpleName}",
                        ),
                    )
                }
            },
        )

    override suspend fun cancel() {
        // Narrow `Cancel(turnId = null)` cancels the current turn (bindings doc:
        // "None cancels the current one"). The engine emits `TurnEnded`, which
        // flows back through the active `submit` stream as `ReplyEvent.End`.
        try {
            handle.submit(ClientCommand.Cancel(turnId = null))
        } catch (_: Throwable) {
            // A cancel that can't be delivered (no in-flight turn) is benign.
        }
    }

    companion object {
        /**
         * Build the engine + register the event-bridging listener, or return
         * `null` when the engine is unavailable (JVM host / missing cdylib /
         * `PlatformUnavailable`) so the caller can fall back to the mock — the
         * Android analog of the iOS `ConversationSourceFactory.make()` guard.
         */
        fun create(context: Context): EngineConversationSource? {
            // replay=0, large buffer + DROP_OLDEST so a slow collector never
            // suspends the engine's listener callback (the Rust runtime calls
            // onEvent on its own thread; back-pressure there would stall the turn).
            val events = MutableSharedFlow<ClientEvent>(
                replay = 0,
                extraBufferCapacity = 256,
                onBufferOverflow = kotlinx.coroutines.channels.BufferOverflow.DROP_OLDEST,
            )
            // The head parked permission request. The engine's outbound
            // `AndroidPermissionSink.onRequest` pushes each request here (mapped
            // to the UI render model); the prompt clears it on resolve. A plain
            // StateFlow (latest wins) is fine: only one request is parked per gate
            // at a time in the foundation (no concurrent worker permissions yet).
            val permissions = MutableStateFlow<PermissionPromptState?>(null)
            // The engine's REAL model catalog + active id (SHIP-BLOCKER #2). The
            // listener below folds every inbound `ModelList` / `ModelChanged`
            // into this StateFlow via the pure `reduceModelEvent`, so the picker
            // is driven by real wire ids out-of-band from the per-turn stream.
            // Starts empty → the UI shows MockData.models until the `ListModels`
            // submitted after build replies with the real catalog.
            val models = MutableStateFlow(EngineModelState())
            // The engine's REAL resumable-session catalog (sibling of `models`).
            // The listener below folds every inbound `SessionList` into this
            // StateFlow via the pure `reduceSessionEvent`, so the drawer is driven
            // by real history out-of-band from the per-turn stream. Starts empty →
            // the drawer shows the MockData session list until the `ListSessions`
            // submitted after build replies with the real catalog.
            val sessions = MutableStateFlow(EngineSessionState())
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
            // No key anywhere → fall back to the mock (the caller swaps in
            // MockConversationSource). Keeps the chat usable on a fresh install
            // before the user sets a key, instead of an engine that only 401s.
            if (creds.apiKey.isBlank()) return null
            val handle = buildVoiceEngine(
                context = context,
                apiBase = creds.apiBase,
                apiKey = creds.apiKey,
                // Empty `creds.model` → the engine starts on MobileConfig.default_model
                // (a real Anthropic wire id), never a branded `lx-*` mock id. A
                // dev-set LINGXI_MODEL still overrides; the secure store doesn't
                // persist a model, so a fresh install always uses the real default.
                model = creds.model,
                onEvent = { event ->
                    // The OUT-OF-BAND state paths: fold model + session catalog
                    // events into the StateFlows the picker / drawer observe,
                    // BEFORE forwarding to the per-turn stream. Both reducers are
                    // no-ops for unrelated events, so every event still reaches
                    // `events` unchanged (lifecycle events like SessionStarted ride
                    // the per-turn stream; the ViewModel acts on them there).
                    models.value = reduceModelEvent(models.value, event)
                    sessions.value = reduceSessionEvent(sessions.value, event)
                    events.emit(event)
                },
                onPermission = { request -> permissions.value = permissionRequestToPrompt(request) },
            ) ?: return null
            // Ask the engine for its REAL catalog now that the handle exists; the
            // reply (`ModelList`) flows back through the listener above into the
            // `models` StateFlow, populating the picker with real wire ids.
            // `handle.submit` is suspend, so fire it off the calling thread — a
            // failed ListModels just leaves the catalog empty (UI shows the mock
            // list); it never blocks building the source.
            CoroutineScope(Dispatchers.Default).launch {
                try {
                    handle.submit(ClientCommand.ListModels)
                } catch (_: Throwable) {
                    // benign: no catalog → picker keeps MockData.models
                }
                // Same out-of-band priming for the session catalog: the reply
                // (`SessionList`) flows back through the listener into `sessions`,
                // populating the drawer with real history. A failed ListSessions
                // just leaves the catalog empty (drawer shows MockData).
                try {
                    handle.submit(ClientCommand.ListSessions(limit = null))
                } catch (_: Throwable) {
                    // benign: no catalog → drawer keeps MockData session list
                }
            }
            return EngineConversationSource(handle, events, permissions, models, sessions)
        }
    }
}
