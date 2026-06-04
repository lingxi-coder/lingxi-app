package com.lingxi.code.conversation

import android.content.Context
import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.ClientEvent
import com.lingxi.code.bindings.MobileEngineHandle
import com.lingxi.code.model.Message
import com.lingxi.code.model.MockData
import com.lingxi.code.voice.buildVoiceEngine
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.flow
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
) : ConversationSource {

    /** A fresh engine session starts empty (the engine streams the transcript). */
    override fun initialMessages(): List<Message> = emptyList()

    override fun submit(text: String): Flow<ReplyEvent> = flow {
        emit(ReplyEvent.Thinking)
        try {
            handle.submit(ClientCommand.SendPrompt(text = text, promptMode = null, images = emptyList(), turnId = null))
        } catch (t: Throwable) {
            emit(ReplyEvent.Error("引擎错误：${t.message ?: t::class.simpleName}"))
            emit(ReplyEvent.End)
            return@flow
        }
        // Map the shared engine stream until a terminal reply, then complete.
        // `transformWhile` emits each mapped reply and stops collecting upstream
        // once a terminal (End / Error) reply has been forwarded.
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
            // API key/base/model from the environment, mirroring the iOS
            // EngineConfig.fromEnvironment. An empty key is valid — slash commands
            // still work and a turn 401s at run time (iOS §). Never hardcoded.
            val env = System.getenv()
            val handle = buildVoiceEngine(
                context = context,
                apiBase = env["ANTHROPIC_BASE_URL"] ?: "",
                apiKey = env["ANTHROPIC_API_KEY"] ?: "",
                model = env["LINGXI_MODEL"] ?: "",
                onEvent = { event -> events.emit(event) },
            ) ?: return null
            return EngineConversationSource(handle, events)
        }
    }
}
