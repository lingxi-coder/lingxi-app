package com.lingxi.code.conversation

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
 * Conversation ViewModel. Holds the mock conversation as a [StateFlow] and
 * exposes intent functions ([send], [newChat], [openSession], [selectModel])
 * the composables call. All engine/network concerns sit behind the injected
 * [ConversationSource], so swapping in the real UniFFI source later requires no
 * changes here beyond the constructor argument.
 */
class ChatViewModel(
    private val source: ConversationSource = MockConversationSource(),
) : ViewModel() {

    private val _state = MutableStateFlow(
        ChatState(
            session = MockData.allSessions.first(),
            messages = source.initialMessages(),
            model = MockData.models.first(),
        ),
    )
    val state: StateFlow<ChatState> = _state.asStateFlow()

    init {
        // Observe the engine's OUT-OF-BAND model state (SHIP-BLOCKER #2): a real
        // `ModelList` populates the picker with wire ids; `ModelChanged` (or the
        // `ListModels` reply's `current`) selects the active row. Mock sources
        // keep an empty state forever, so this never disturbs MockData.models.
        viewModelScope.launch {
            source.modelState.collect { engine -> applyModelState(engine) }
        }
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

    /** Switch to another session: cancel any in-flight turn and reset state. */
    fun openSession(ref: SessionRef) {
        turnJob?.cancel()
        turnJob = null
        streamingIndex = null
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
        turnJob?.cancel()
        turnJob = null
        streamingIndex = null
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

        streamingIndex = null
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
            source.submit(trimmed).collect { event -> reduce(event) }
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
     * Reduce one [ReplyEvent] into [ChatState]. Extracted from [send] so it is
     * unit-testable with a fake source (no engine). Mirrors the iOS
     * `EngineConversationSource.apply(_:)` switch.
     */
    internal fun reduce(event: ReplyEvent) {
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
}
