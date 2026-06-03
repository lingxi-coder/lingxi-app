package com.lingxi.code.conversation

import com.lingxi.code.model.Message
import com.lingxi.code.model.MockData
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow

/**
 * The seam where the future UniFFI listener will feed real `ClientEvent`s.
 *
 * Today the [ChatViewModel] talks only to a [ConversationSource] — it never
 * reaches into mock data or a network directly. When the `engine-mobile` `.aar`
 * lands (the mobile analog of the Electron↔bridge work), a real implementation
 * will replace [MockConversationSource] by wrapping the UniFFI `submit()` call
 * and mapping streamed engine events into [ReplyEvent]s. No screen or ViewModel
 * code changes — only this binding.
 */
interface ConversationSource {

    /** The conversation a freshly-opened session starts with. */
    fun initialMessages(): List<Message>

    /**
     * Submit a user turn and observe the assistant's reply as a stream of
     * [ReplyEvent]s. A real engine source would emit incremental
     * [ReplyEvent.Delta]s; the mock emits a single [ReplyEvent.Thinking] then a
     * [ReplyEvent.Completed] after a short delay to simulate latency.
     */
    fun submit(text: String): Flow<ReplyEvent>
}

/** Streamed assistant-reply events (the mock analog of engine `ClientEvent`s). */
sealed interface ReplyEvent {
    /** The model is "thinking" — render the pulsing dots row. */
    data object Thinking : ReplyEvent

    /** An incremental text delta (unused by the mock; reserved for the engine). */
    data class Delta(val text: String) : ReplyEvent

    /** The final assistant message (with its optional thinking-time tag). */
    data class Completed(val message: Message) : ReplyEvent
}

/**
 * The shell's mock source: starts from [MockData.messagesDefault] and answers
 * every turn with the same canned reply after a 1.1s "thinking" beat — matching
 * the iOS `ChatView.send` simulation exactly.
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
