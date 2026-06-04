package com.lingxi.code.conversation

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.lingxi.code.model.Message
import com.lingxi.code.model.MockData
import com.lingxi.code.model.ModelOption
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
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
    /** The model selected in the composer chip. */
    val model: ModelOption,
    /**
     * A transient, user-visible status line (tool activity, engine errors).
     * `null` hides the row. Mirrors the iOS `ConversationModel.statusLine`.
     */
    val statusLine: String? = null,
)

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

    /**
     * Index into [ChatState.messages] of the assistant message currently being
     * streamed (deltas append into it). `null` between turns / before the first
     * delta of a turn. Mirrors the iOS `EngineConversationSource.streamingIndex`.
     */
    private var streamingIndex: Int? = null

    /** Switch to another session: reset to the default mock conversation. */
    fun openSession(ref: SessionRef) {
        streamingIndex = null
        _state.update {
            it.copy(
                session = ref,
                messages = source.initialMessages(),
                isNew = false,
                streaming = false,
                statusLine = null,
            )
        }
    }

    /** Start a fresh, empty chat (top-bar "edit" / new-chat button). */
    fun newChat() {
        streamingIndex = null
        _state.update {
            it.copy(
                session = SessionRef(id = "new", title = "新对话"),
                messages = emptyList(),
                isNew = true,
                streaming = false,
                statusLine = null,
            )
        }
    }

    /** Change the composer's selected model. */
    fun selectModel(model: ModelOption) {
        _state.update { it.copy(model = model) }
    }

    /**
     * Submit a user turn. Appends the user message, flips [ChatState.streaming]
     * on, and collects the [ConversationSource] reply stream — appending the
     * completed assistant message and clearing the streaming flag.
     */
    fun send(text: String) {
        val trimmed = text.trim()
        if (trimmed.isEmpty()) return

        streamingIndex = null
        _state.update {
            it.copy(
                isNew = false,
                statusLine = null,
                messages = it.messages + Message(role = Role.User, text = trimmed),
            )
        }

        viewModelScope.launch {
            source.submit(trimmed).collect { event -> reduce(event) }
        }
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
                _state.update { it.copy(streaming = false, statusLine = "错误：${event.message}") }
            }

            is ReplyEvent.Completed -> {
                streamingIndex = null
                _state.update {
                    it.copy(streaming = false, messages = it.messages + event.message)
                }
            }

            is ReplyEvent.End -> {
                streamingIndex = null
                _state.update { it.copy(streaming = false) }
            }
        }
    }
}
