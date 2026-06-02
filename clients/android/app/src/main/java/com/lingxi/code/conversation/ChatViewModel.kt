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

    /** Switch to another session: reset to the default mock conversation. */
    fun openSession(ref: SessionRef) {
        _state.update {
            it.copy(
                session = ref,
                messages = source.initialMessages(),
                isNew = false,
                streaming = false,
            )
        }
    }

    /** Start a fresh, empty chat (top-bar "edit" / new-chat button). */
    fun newChat() {
        _state.update {
            it.copy(
                session = SessionRef(id = "new", title = "新对话"),
                messages = emptyList(),
                isNew = true,
                streaming = false,
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

        _state.update {
            it.copy(
                isNew = false,
                messages = it.messages + Message(role = Role.User, text = trimmed),
            )
        }

        viewModelScope.launch {
            source.submit(trimmed).collect { event ->
                when (event) {
                    is ReplyEvent.Thinking -> _state.update { it.copy(streaming = true) }
                    is ReplyEvent.Delta -> Unit // reserved for the real engine stream
                    is ReplyEvent.Completed -> _state.update {
                        it.copy(streaming = false, messages = it.messages + event.message)
                    }
                }
            }
        }
    }
}
