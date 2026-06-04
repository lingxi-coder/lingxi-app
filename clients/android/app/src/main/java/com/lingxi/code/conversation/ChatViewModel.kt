package com.lingxi.code.conversation

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.Message
import com.lingxi.code.model.MockData
import com.lingxi.code.model.ModelOption
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/**
 * Immutable UI state for the conversation surface. Hoisted out of the
 * composables and driven entirely by [ChatViewModel] / [ConversationSource].
 */
data class ChatState(
    val session: SessionRef,
    val messages: List<Message>,
    /** A brand-new (empty) chat shows the empty-state hero instead of a list. */
    val isNew: Boolean = false,
    /** True while the assistant reply streams — renders the pulsing dots row. */
    val streaming: Boolean = false,
    /** The model selected in the composer chip (the active engine id, or a mock row). */
    val model: ModelOption,
    /**
     * The catalog the picker shows. Driven by the engine's REAL `ModelList`
     * (out-of-band, via [ConversationSource.modelState]); falls back to the
     * branded [MockData.models] when the engine hasn't reported a catalog yet
     * (mock mode / before the first `ModelList`). The active row is [model].
     */
    val availableModels: List<ModelOption> = MockData.models,
    /**
     * A transient, user-visible status line (tool activity). `null` hides the
     * row. Mirrors the iOS `ConversationModel.statusLine`. Errors no longer ride
     * this dim line — they surface in [error] as a persistent banner.
     */
    val statusLine: String? = null,
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

/**
 * Minimal, Bundle-safe (string) serialization for the transcript persisted into
 * [SavedStateHandle], so the conversation survives process death without pulling
 * in a serialization library or making [Message] `Parcelable`.
 *
 * Each [Message] becomes ONE line of `roletagidtext`
 * (`` is a control char that never appears in user text); a blank `tag`
 * round-trips back to `null`. The [text] is placed LAST and is the only field
 * allowed to contain newlines, so the list is stored as an `ArrayList<String>`
 * (one entry per message) — a primitive the saved-state `Bundle` persists
 * verbatim across process death. PURE (no Android types) so it is unit-testable
 * on the plain JVM.
 */
internal object TranscriptCodec {
    private const val FS = '' // field separator (never in user text)

    fun encode(messages: List<Message>): ArrayList<String> =
        ArrayList(
            messages.map { m ->
                // role, tag, id, text — text last (the only newline-bearing field).
                "${m.role.name}$FS${m.tag.orEmpty()}$FS${m.id}$FS${m.text}"
            },
        )

    fun decode(lines: List<String>?): List<Message> {
        if (lines.isNullOrEmpty()) return emptyList()
        return lines.mapNotNull { line ->
            // limit=4 so a text field containing the (improbable) separator or any
            // newline is preserved intact as the final segment.
            val parts = line.split(FS, limit = 4)
            if (parts.size < 4) return@mapNotNull null
            val role = when (parts[0]) {
                Role.Ai.name -> Role.Ai
                Role.User.name -> Role.User
                else -> return@mapNotNull null
            }
            val tag = parts[1].ifEmpty { null }
            Message(role = role, text = parts[3], tag = tag, id = parts[2])
        }
    }
}

/**
 * Conversation ViewModel. Holds the conversation as a [StateFlow] and exposes
 * intent functions ([send], [newChat], [openSession], [selectModel]) the
 * composables call. All engine/network concerns sit behind the injected
 * [ConversationSource], so swapping in the real UniFFI source later requires no
 * changes here beyond the constructor argument.
 *
 * The optional [savedState] persists the live transcript, the composer draft and
 * the active session id so they survive process death (low-memory kill while the
 * app is backgrounded). It is keyed by primitive/`ArrayList<String>` values only,
 * so the saved-state `Bundle` round-trips them without a serialization library.
 * `null` (the default, and what the reducer unit tests pass) disables persistence
 * — the ViewModel then behaves exactly as before.
 */
class ChatViewModel(
    private val source: ConversationSource = MockConversationSource(),
    private val savedState: SavedStateHandle? = null,
) : ViewModel() {

    private val _state = MutableStateFlow(
        run {
            // Restore the persisted transcript + session if process death dropped
            // us; otherwise start from the source's initial transcript. A restored
            // (possibly empty) transcript wins over `initialMessages()` so a user
            // who had cleared to a fresh chat doesn't get the mock seed back.
            val restored: List<Message>? =
                savedState?.takeIf { it.contains(KEY_TRANSCRIPT) }
                    ?.let { TranscriptCodec.decode(it.get<ArrayList<String>>(KEY_TRANSCRIPT)) }
            val sessionId = savedState?.get<String>(KEY_SESSION_ID)
            val sessionTitle = savedState?.get<String>(KEY_SESSION_TITLE)
            val session =
                if (sessionId != null && sessionTitle != null) SessionRef(sessionId, sessionTitle)
                else MockData.allSessions.first()
            val isNew = savedState?.get<Boolean>(KEY_IS_NEW) ?: false
            ChatState(
                session = session,
                messages = restored ?: source.initialMessages(),
                isNew = isNew,
                model = MockData.models.first(),
            )
        },
    )
    val state: StateFlow<ChatState> = _state.asStateFlow()

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

    init {
        // Observe the engine's OUT-OF-BAND model state (SHIP-BLOCKER #2): a real
        // `ModelList` populates the picker with wire ids; `ModelChanged` (or the
        // `ListModels` reply's `current`) selects the active row. Mock sources
        // keep an empty state forever, so this never disturbs MockData.models.
        viewModelScope.launch {
            source.modelState.collect { engine -> applyModelState(engine) }
        }
        // Keep the persisted transcript + session id in lock-step with state, so a
        // process-death kill at any moment restores the latest committed transcript.
        if (savedState != null) {
            viewModelScope.launch {
                _state.collect { s -> persist(s) }
            }
        }
    }

    /** Write the durable slice of [ChatState] into [savedState] (Bundle-safe values). */
    private fun persist(s: ChatState) {
        val sv = savedState ?: return
        sv[KEY_TRANSCRIPT] = TranscriptCodec.encode(s.messages)
        sv[KEY_SESSION_ID] = s.session.id
        sv[KEY_SESSION_TITLE] = s.session.title
        sv[KEY_IS_NEW] = s.isNew
    }

    /**
     * Fold the engine's [EngineModelState] into [ChatState]: build the picker
     * rows from the REAL wire ids and select the active one. Empty catalog →
     * leave the mock list + selection untouched (mock mode). Extracted so the
     * mapping is exercised directly in unit tests with a fake source.
     */
    internal fun applyModelState(engine: EngineModelState) {
        if (!engine.hasCatalog) return // mock mode: keep MockData.models + its selection
        val options = EngineModelCatalog.options(engine.available)
        val active = options.firstOrNull { it.id == engine.active } ?: options.first()
        _state.update { it.copy(availableModels = options, model = active) }
    }

    /**
     * Index into [ChatState.messages] of the assistant message currently being
     * streamed (deltas append into it). `null` between turns / before the first
     * delta of a turn. Mirrors the iOS `EngineConversationSource.streamingIndex`.
     */
    private var streamingIndex: Int? = null

    /**
     * The coroutine collecting the active turn's reply stream. Held so [cancel]
     * (and a session switch) can stop local collection, and so [send] can detect
     * an in-flight turn to ignore an overlapping submit. `null` between turns.
     */
    private var turnJob: Job? = null

    /**
     * Monotonic turn generation, bumped by EVERY action that abandons the
     * in-flight turn ([openSession], [newChat], [send], [cancel]). Each collecting
     * coroutine captures the token live at its launch and stamps it onto every
     * [reduce] call; [reduce] DROPS any event whose token is stale.
     *
     * ORPHANED-TURN FIX: `Job.cancel()` is cooperative — it can't stop a `reduce`
     * that is already executing on the collector thread when the session switches,
     * so a late `Delta`/`End` from the old turn could otherwise mutate the NEW
     * session's transcript. The token closes that race deterministically: a stale
     * turn's events are ignored even if its coroutine briefly outlives the switch.
     */
    private var turnToken: Long = 0L

    /** Switch to another session: cancel any in-flight turn and reset state. */
    fun openSession(ref: SessionRef) {
        abandonInFlightTurn()
        _state.update {
            it.copy(
                session = ref,
                messages = source.initialMessages(),
                isNew = false,
                streaming = false,
                statusLine = null,
                error = null,
            )
        }
    }

    /** Start a fresh, empty chat (top-bar "edit" / new-chat button). */
    fun newChat() {
        abandonInFlightTurn()
        _state.update {
            it.copy(
                session = SessionRef(id = "new", title = "新对话"),
                messages = emptyList(),
                isNew = true,
                streaming = false,
                statusLine = null,
                error = null,
            )
        }
    }

    /**
     * Cancel the in-flight collecting coroutine and reset the per-turn cursors so
     * NO stale event can mutate the next session's transcript. Bumping [turnToken]
     * is the deterministic half (events from the old turn are dropped by [reduce]
     * even if its coroutine hasn't observed cancellation yet); cancelling
     * [turnJob] is the eager half (stop collecting promptly). Shared by
     * [openSession] / [newChat].
     */
    private fun abandonInFlightTurn() {
        turnToken++
        turnJob?.cancel()
        turnJob = null
        streamingIndex = null
    }

    /**
     * Change the active model. Reflects the pick locally immediately (snappy
     * chip), then submits `SetModel(id)` to the engine with the REAL wire id —
     * the engine confirms with `ModelChanged`, which re-selects the row via
     * [applyModelState]. For the mock source `setModel` is a no-op, so the local
     * selection stands.
     */
    fun selectModel(model: ModelOption) {
        _state.update { it.copy(model = model) }
        viewModelScope.launch { source.setModel(model.id) }
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
        turnToken++
        val job = turnJob
        turnJob = null
        viewModelScope.launch {
            // Tell the engine first (best-effort), then drop local collection.
            source.cancel()
            job?.cancel()
        }
        streamingIndex = null
        _state.update { it.copy(streaming = false, statusLine = null) }
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
            is ReplyEvent.Thinking -> _state.update { it.copy(streaming = true) }

            is ReplyEvent.Delta -> _state.update { s ->
                val i = streamingIndex
                if (i != null && s.messages.indices.contains(i)) {
                    // Append into the in-flight assistant message.
                    val updated = s.messages.toMutableList()
                    val prev = updated[i]
                    updated[i] = prev.copy(text = prev.text + event.text)
                    s.copy(streaming = true, messages = updated)
                } else {
                    // First delta of the turn: open a new assistant message.
                    val opened = s.messages + Message(role = Role.Ai, text = event.text)
                    streamingIndex = opened.size - 1
                    s.copy(streaming = true, messages = opened)
                }
            }

            is ReplyEvent.ToolActivity -> _state.update { it.copy(statusLine = event.label) }

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
                    )
                }
            }

            is ReplyEvent.Completed -> {
                streamingIndex = null
                turnJob = null
                _state.update {
                    it.copy(streaming = false, messages = it.messages + event.message)
                }
            }

            is ReplyEvent.End -> {
                streamingIndex = null
                turnJob = null
                _state.update { it.copy(streaming = false) }
            }
        }
    }

    private companion object {
        // SavedStateHandle keys for the durable conversation slice (process death).
        const val KEY_TRANSCRIPT = "chat.transcript" // ArrayList<String>, see TranscriptCodec
        const val KEY_DRAFT = "chat.draft" // String — unsent composer text
        const val KEY_SESSION_ID = "chat.session.id" // String
        const val KEY_SESSION_TITLE = "chat.session.title" // String
        const val KEY_IS_NEW = "chat.isNew" // Boolean — empty-state hero vs list
    }
}
