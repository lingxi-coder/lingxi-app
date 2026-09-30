package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.McpServerDto
import com.lingxi.code.bindings.client.McpStatusDto
import com.lingxi.code.bindings.client.MessageDto
import com.lingxi.code.model.ConnStatus
import com.lingxi.code.model.DefaultSessionCatalogStrings
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.EngineSessionState
import com.lingxi.code.model.MCPServer
import com.lingxi.code.model.Message
import com.lingxi.code.model.SessionCatalog
import com.lingxi.code.model.SessionCatalogStrings
import com.lingxi.code.model.SessionMode
import com.lingxi.code.model.canonicalSessionId
import com.lingxi.code.model.toUi

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
                    mode = dto.mode.toUi(),
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
    val mode: SessionMode = SessionMode.Code,
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
        mode = event.mode.toUi(),
        transcript = emptyList(),
        kind = SessionActivationKind.Started,
    )
    is ClientEvent.SessionResumed -> ActivatedSession(
        sessionId = canonicalSessionId(event.sessionId),
        mode = event.mode.toUi(),
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
