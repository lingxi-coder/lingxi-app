package com.lingxi.code.conversation

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/** Admission for automatic speech; an already admitted conversation retains its queued reply. */
internal class AutomaticReplySpeechGate(private val blocked: () -> Boolean) {
    private data class Configuration(val enabled: Boolean = false, val foreground: Boolean = false)
    private val configuration = MutableStateFlow(Configuration())
    private val revision = MutableStateFlow(0L)

    fun configure(enabled: Boolean, foreground: Boolean) {
        configuration.value = Configuration(enabled, foreground)
        signal()
    }

    fun setForeground(foreground: Boolean) {
        configuration.update { it.copy(foreground = foreground) }
        signal()
    }

    fun acceptsCompletion(): Boolean = configuration.value.let { it.enabled && it.foreground }
    fun mayStart(): Boolean = acceptsCompletion() && !blocked()
    fun signal() { revision.update { it + 1 } }

    suspend fun awaitStart(): Boolean {
        while (true) {
            val seen = revision.value
            if (!configuration.value.enabled) return false
            if (mayStart()) return true
            // Some audio operations finish without emitting a new visible phase.
            withTimeoutOrNull(1_000) { revision.first { it != seen } }
        }
    }
}

/** The subscription belongs to the conversation owner, not to the visible screen. */
internal fun CoroutineScope.observeOwnerTurnCompletions(
    completions: Flow<ConversationTurnCompletion>,
    autoPlay: () -> Boolean,
    flowVisible: () -> Boolean,
    onFlow: (ConversationTurnCompletion) -> Unit,
    onAutomaticReply: (String) -> Unit,
): Job = launch(start = CoroutineStart.UNDISPATCHED) {
    completions.collect { completion ->
        if (completion.origin == ConversationTurnOrigin.Flow) {
            onFlow(completion)
        } else if (completion.outcome == ConversationTurnOutcome.Completed && autoPlay() && !flowVisible()) {
            completion.finalAssistantText.trim().takeIf(String::isNotEmpty)?.let(onAutomaticReply)
        }
    }
}
