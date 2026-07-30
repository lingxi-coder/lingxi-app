package com.lingxi.code.conversation

import android.content.Context
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.ListingKindDto
import com.lingxi.code.bindings.McpServerDto
import com.lingxi.code.bindings.McpStatusDto
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.MCPServer
import com.lingxi.code.bindings.ErrorKindDto
import com.lingxi.code.bindings.MessageBlockDto
import com.lingxi.code.bindings.MessageDto
import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.bindings.PermissionRequest
import com.lingxi.code.bindings.PermissionResponseDto
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionCatalog
import com.lingxi.code.model.canonicalSessionId
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
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.flow.transformWhile

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

    /** The conversation a freshly-opened session starts with. */
    fun initialMessages(): List<Message> = emptyList()

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
fun sessionActivationFrom(event: ClientEvent): ActivatedSession? = when (event) {
    is ClientEvent.SessionStarted -> ActivatedSession(
        sessionId = canonicalSessionId(event.sessionId),
        transcript = emptyList(),
        kind = SessionActivationKind.Started,
    )
    is ClientEvent.SessionResumed -> ActivatedSession(
        sessionId = canonicalSessionId(event.sessionId),
        transcript = event.messages.map(::messageDtoToMessage),
        kind = SessionActivationKind.Resumed,
    )
    else -> null
}

/**
 * Lower one wire [MessageDto] to the UI [Message] model. The UI bubble is
 * TEXT-ONLY (assistant text renders as Markdown; user text is plain), so the
 * ordered content [MessageBlockDto]s are flattened to a single body string via
 * [messageDtoText]. The wire `role` ("user" / "assistant" / "system") maps to
 * [Role]: "user" → [Role.User]; everything else (assistant / system) → [Role.Ai]
 * (the avatar+markdown bubble). PURE — no engine / Android dependency.
 */
fun messageDtoToMessage(dto: MessageDto): Message {
    val role = if (dto.role.equals("user", ignoreCase = true)) Role.User else Role.Ai
    return Message(role = role, text = messageDtoText(dto.blocks))
}

/**
 * Flatten a message's ordered content [MessageBlockDto]s into the single body
 * string the UI bubble renders, mirroring how the engine's live MessageComplete
 * synthesis collapses a turn to text. Each block kind folds to a readable line:
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
fun messageDtoText(blocks: List<MessageBlockDto>): String =
    blocks.mapNotNull { block ->
        val line: String = when (block) {
            is MessageBlockDto.Text -> block.text
            is MessageBlockDto.Thinking -> block.thinking
            is MessageBlockDto.RedactedThinking -> "[已折叠的思考]"
            is MessageBlockDto.CompactBoundary -> "对话已压缩"
            is MessageBlockDto.ToolUse -> "调用工具 ${block.tool}…"
            is MessageBlockDto.ToolResult ->
                if (block.isError) "工具失败" else "工具结果"
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
        val inputSummary: String? = null,
        val elapsedMs: Long? = null,
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

    /** A terminal error to surface (engine `Error`, or a build/submit failure). */
    data class Error(val message: String) : ReplyEvent

    /** The final assistant message (MessageComplete / no-stream path). */
    data class Completed(val message: Message) : ReplyEvent

    /** Terminal marker: the turn ended cleanly. The stream completes after this. */
    data object End : ReplyEvent
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
fun clientEventToReply(event: ClientEvent): ReplyEvent? = when (event) {
    is ClientEvent.TurnStarted -> ReplyEvent.Thinking
    is ClientEvent.TextDelta -> ReplyEvent.Delta(event.text)
    is ClientEvent.ThinkingDelta -> ReplyEvent.ReasoningDelta(event.thinking)
    is ClientEvent.SystemNotice -> ReplyEvent.Notice(event.message, event.isError)
    is ClientEvent.ToolUseStarted ->
        if (isShellTool(event.tool)) {
            ReplyEvent.ShellTool(shellStarted(event.id, event.inputJson))
        } else {
            ReplyEvent.ToolActivity(
                label = "调用工具 ${event.tool}…",
                id = event.id,
                tool = event.tool,
                status = AgentToolStatus.Running,
                inputSummary = summarizeToolInput(event.inputJson),
            )
        }
    is ClientEvent.ToolHeartbeat ->
        if (isShellTool(event.tool)) {
            ReplyEvent.ShellTool(
                ShellToolUpdate.Heartbeat(event.id, event.elapsedMs.toLong()),
            )
        } else {
            ReplyEvent.ToolActivity(
                label = "工具 ${event.tool} 运行中…",
                id = event.id,
                tool = event.tool,
                status = AgentToolStatus.Running,
                elapsedMs = event.elapsedMs.toLong(),
            )
        }
    is ClientEvent.ToolUseResult ->
        if (isShellTool(event.tool)) {
            ReplyEvent.ShellTool(shellFinished(event.id, event.resultJson, event.isError))
        } else if (event.isError) {
            ReplyEvent.ToolActivity(
                label = "工具 ${event.tool} 失败",
                id = event.id,
                tool = event.tool,
                status = AgentToolStatus.Failed,
            )
        } else {
            ReplyEvent.ToolActivity(
                label = "工具 ${event.tool} 完成",
                id = event.id,
                tool = event.tool,
                status = AgentToolStatus.Completed,
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
        ?.let { ReplyEvent.Completed(messageDtoToMessage(it)) }
        ?: ReplyEvent.End
    is ClientEvent.TurnEnded -> ReplyEvent.End
    is ClientEvent.Error -> ReplyEvent.Error(
        userFacingEngineError(event.kind, event.message),
    )
    else -> null // model / session / permission / listings ride out-of-band flows
}

/**
 * Convert transport diagnostics into concise, actionable mobile copy.
 *
 * The Android HTTP backend opts into reqwest's nested cause chain, so DNS,
 * timeout, and TLS failures are identifiable here. Provider responses such as
 * 401/404 are not rewritten: their original message still reaches the existing
 * auth/model error handling.
 */
internal fun userFacingEngineError(kind: ErrorKindDto, message: String): String {
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
            "无法解析模型服务地址。请检查 VPN、私人 DNS 或当前网络后重试。"

        listOf("certificate", "tls", "ssl").any(normalized::contains) ->
            "模型服务安全连接失败。请检查系统时间、VPN 或证书设置后重试。"

        listOf("timeout", "timed out").any(normalized::contains) ->
            "连接模型服务超时。请检查当前网络或 VPN 后重试。"

        listOf(
            "connection failed",
            "connect error",
            "error sending request",
        ).any(normalized::contains) ->
            "无法连接模型服务。请检查当前网络或 VPN 后重试。"

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
fun mapReplyStream(events: Flow<ClientEvent>): Flow<ReplyEvent> = flow {
    emit(ReplyEvent.Thinking)
    emitAll(
        events.transformWhile { event ->
            val reply = clientEventToReply(event) ?: return@transformWhile true
            emit(reply)
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
    internal val reason: String = "移动端引擎当前不可用",
) : ConversationSource {
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
    private val eventScope: CoroutineScope,
    private val permissions: MutableStateFlow<PermissionPromptState?>,
    private val models: MutableStateFlow<EngineModelState>,
    private val sessions: MutableStateFlow<EngineSessionState>,
    private val activeSession: MutableStateFlow<ActivatedSession?>,
    private val mcp: MutableStateFlow<List<MCPServer>>,
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
                "会话列表加载失败：${t.message ?: t::class.simpleName}"
            )
        }
    }

    override suspend fun resumeSession(uuid: String) {
        if (uuid.isBlank()) return
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
            title = title.ifBlank { "新对话" },
        )
    }

    override suspend fun newSession() {
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
        handle.submit(ClientCommand.Cancel(turnId = null))
    }

    override fun close() {
        eventRelay.close()
        eventScope.cancel()
        runCatching { handle.destroy() }
    }

    companion object {
        /**
         * Build the engine + register the event-bridging listener, or return an
         * explicit unavailable source when the engine is not usable.
         */
        fun create(
            context: Context,
            projectWorkspace: ProjectWorkspace? = null,
            linuxRuntimeMode: LinuxRuntimeMode = LinuxRuntimeMode.Legacy,
        ): ConversationSource {
            // Callback ingress is non-blocking and lossless. A dedicated pump may
            // suspend behind a slow collector without ever stalling Rust's event
            // callback or dropping assistant text / terminal events.
            val eventScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            val eventRelay = LosslessEventRelay<ClientEvent>(eventScope)
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
                    sessions.value = reduceSessionEvent(sessions.value, event)
                    sessionActivationFrom(event)?.let { activeSession.value = it }
                    // Out-of-band MCP listing: fold `McpServers` into its StateFlow.
                    if (event is ClientEvent.McpServers) mcp.value = event.servers.map { it.toMcpServer() }
                    eventRelay.offer(event)
                },
                onPermission = { request -> permissions.value = permissionRequestToPrompt(request) },
            ) ?: run {
                eventRelay.close()
                eventScope.cancel()
                return UnavailableConversationSource("移动端引擎不可用或未正确链接")
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
                        "会话列表加载失败：${t.message ?: t::class.simpleName}"
                    )
                }
            }
            return EngineConversationSource(
                handle = handle,
                events = eventRelay.events,
                eventRelay = eventRelay,
                eventScope = eventScope,
                permissions = permissions,
                models = models,
                sessions = sessions,
                activeSession = activeSession,
                mcp = mcp,
            )
        }
    }
}
