package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.model.canonicalSessionId
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.withTimeout

/** Refresh display history once when a headless source first gains a UI. */
internal suspend fun refreshHeadlessConversationSnapshot(
    activation: ActivatedSession,
    events: SharedFlow<ClientEvent>,
    submit: suspend (ClientCommand) -> Unit,
    strings: ConversationStrings = DefaultConversationStrings,
    timeoutMillis: Long = 20_000L,
): ActivatedSession {
    if (activation.retainedProjection != null || activation.kind != SessionActivationKind.Resumed) return activation
    val sessionId = canonicalSessionId(activation.sessionId)
    val snapshot = withTimeout(timeoutMillis) {
        // Native may publish the reply before submit returns. Register first;
        // a failed/cancelled submit must release this subscriber and propagate.
        events.onSubscription {
            submit(ClientCommand.LoadSessionAgentTranscript("main"))
        }.first { event ->
            event is ClientEvent.SessionAgentTranscript && event.agentId == "main" &&
                canonicalSessionId(event.sessionId) == sessionId
        } as ClientEvent.SessionAgentTranscript
    }
    val rows = snapshot.messages.sortedBy { it.messageIndex }
    check(rows.map { it.messageIndex }.distinct().size == rows.size &&
        rows.map { it.messageUuid }.distinct().size == rows.size &&
        rows.all { it.messageUuid.isNotBlank() && it.messageIndex < snapshot.nextMessageIndex }) {
        "The current conversation transcript has conflicting row identities"
    }
    return activation.copy(
        transcript = transcriptFromDtos(rows.map { it.message.copy(transcriptUuid = it.messageUuid) }, strings),
        transcriptComplete = true,
    )
}
