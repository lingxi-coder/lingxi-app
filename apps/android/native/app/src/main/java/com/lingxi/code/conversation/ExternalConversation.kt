package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.AskUserQuestionRequestDto
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.model.Message
import kotlinx.coroutines.flow.Flow

/**
 * A source whose turns run somewhere else, so the phone does not drive them: a
 * desktop session followed over Remote Control. [ChatViewModel] still renders it
 * with the same reducers as a local engine, but takes everything from
 * [externalFeed] instead of a per-turn [ConversationSource.submit] stream. A turn
 * the phone did not start (typed on the desktop, scheduled, sent by another
 * device) is therefore rendered exactly like one it started itself.
 *
 * Like [SlashCommandConversationSource], this is a capability of a
 * [ConversationSource], discovered with an `as?` cast.
 */
interface ExternalTurnConversationSource {
    /**
     * Everything the followed session reports, in order. Collected for as long as
     * the conversation is shown; the source follows the session while it is collected.
     */
    val externalFeed: Flow<ExternalFeedItem>

    /** Send a prompt to the session; its row comes back through [externalFeed]. */
    suspend fun sendExternalPrompt(text: String, images: List<ImageRefDto>)

    /** Drop a prompt that failed to send from the source's outgoing list. */
    suspend fun dismissExternalPrompt(promptId: String)
}

/** One change of a followed session, already lowered to what [ChatViewModel] reduces. */
sealed interface ExternalFeedItem {
    /** The held conversation is replaced (first load, new snapshot, reconnect). */
    data class Base(val session: ActivatedSession, val title: String?) : ExternalFeedItem

    /** A page of older history, to put in front of the held messages. */
    data class OlderMessages(val messages: List<Message>) : ExternalFeedItem

    /** A user prompt of any origin; the engine's event stream does not carry them. */
    data class UserPrompt(
        val turnId: Long,
        val text: String,
        val imageCount: Int,
        val originLabel: String?,
    ) : ExternalFeedItem

    /** Images of the prompt of [turnId], once fetched from the desktop. */
    data class PromptImages(val turnId: Long, val images: List<ImageRefDto>) : ExternalFeedItem

    /** An engine event of the session. */
    data class Event(val event: ClientEvent) : ExternalFeedItem

    /** Prompts this device sent whose own record has not come back yet. */
    data class Outgoing(val prompts: List<ExternalOutgoingPrompt>) : ExternalFeedItem

    /** The complete set of questions waiting for an answer. */
    data class PendingQuestions(val questions: List<AskUserQuestionRequestDto>) : ExternalFeedItem

    /** Whether a turn runs on the other side; settles a turn the events did not. */
    data class Turn(val running: Boolean) : ExternalFeedItem

    /** Whether the composer may send, and the line to show while it may not. */
    data class Readiness(val ready: Boolean, val statusLine: String?) : ExternalFeedItem

    /** A short line about something the transcript cannot show (an item too large to send). */
    data class Notice(val text: String) : ExternalFeedItem
}

/** A prompt sent from this device that the desktop has not echoed back yet. */
data class ExternalOutgoingPrompt(
    val id: String,
    val text: String,
    val images: List<ImageRefDto>,
    val failed: Boolean,
)
