package com.lingxi.code.conversation

import androidx.lifecycle.ViewModelStore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.util.UUID

internal class SideChatEntry(val source: SideChatSource, val viewModel: ChatViewModel) {
    val store = ViewModelStore().apply { put("chat", viewModel) }
    val draft = MutableStateFlow("")
    var lastSubmission: String? = null
    val attachment = MutableStateFlow<ComposerAttachment?>(null)
    var lastSubmittedAttachment: ComposerAttachment? = null
    val closing = MutableStateFlow(false)
    var endInFlight = false
}

internal class SideChatManager(private val scope: CoroutineScope) {
    private val entries = MutableStateFlow<List<SideChatEntry>>(emptyList())
    val chats = entries.asStateFlow()
    val selected = MutableStateFlow<SideChatEntry?>(null)
    val visible = MutableStateFlow(false)
    val error = MutableStateFlow<String?>(null)
    val failedDraft = MutableStateFlow<String?>(null)
    private var generation = 0
    private var creating = 0
    private val foreground = SideChatForegroundGate()
    val hasLiveChats: Boolean get() = creating > 0 || entries.value.isNotEmpty()
    var whenVacant: (() -> Unit)? = null
    private fun notifyVacant() {
        if (!hasLiveChats) whenVacant?.also { whenVacant = null }?.invoke()
    }
    fun create(parent: EngineConversationSource, input: String, restore: () -> Unit) {
        failedDraft.value = null
        error.value = null
        creating++
        val originalGeneration = generation
        val foregroundToken = foreground.capture()
        scope.launch {
            withContext(NonCancellable) {
                val source = parent.newSideChatSource()
                try {
                    source.create(UUID.randomUUID().toString())
                    if (generation != originalGeneration) { source.end(); return@withContext }
                    val entry = SideChatEntry(source, ChatViewModel(source))
                    source.onEnded = { scope.launch { retire(entry) } }
                    entries.value = entries.value + entry
                    selected.value = entry
                    visible.value = true
                    source.setForeground(foreground.isForeground)
                    if (input.isNotEmpty()) {
                        entry.lastSubmission = input
                        if (foreground.maySubmit(foregroundToken)) entry.viewModel.send(input)
                        else entry.draft.value = input
                    }
                } catch (failure: Throwable) {
                    source.close()
                    error.value = failure.message ?: "Unable to create side chat"
                    failedDraft.value = if (input.isEmpty()) "/side" else "/side $input"
                    restore()
                } finally {
                    creating--
                    notifyVacant()
                }
            }
        }
    }
    /** Activity backgrounding cancels work; hiding the sheet never calls this. */
    fun cancelCurrentTurns() {
        foreground.background()
        entries.value.forEach { entry ->
            entry.source.beginBackgroundStop()
            val state = entry.viewModel.state.value
            if (state.streaming || state.liveTurnWaitingForUser) entry.viewModel.cancel()
            else launchCleanup {
                runCatching { entry.source.cancel() }
                    .onFailure { error.value = "Unable to stop side chat" }
            }
        }
    }
    fun resumeForeground() {
        foreground.resume()
        entries.value.forEach { it.source.setForeground(true) }
    }
    fun end(entry: SideChatEntry) {
        if (entry.endInFlight) return
        entry.endInFlight = true
        entry.closing.value = true
        scope.launch {
            withContext(NonCancellable) {
                try {
                    entry.source.end()
                    retire(entry)
                } catch (failure: Throwable) {
                    error.value = failure.message ?: "Side chat is closing"
                } finally {
                    entry.endInFlight = false
                }
            }
        }
    }
    private fun retire(entry: SideChatEntry) {
        entry.source.close()
        entry.draft.value = ""
        entry.attachment.value = null
        entry.lastSubmission = null
        entry.lastSubmittedAttachment = null
        entry.store.clear()
        entries.value = entries.value - entry
        if (selected.value === entry) selected.value = entries.value.lastOrNull()
        if (entries.value.isEmpty()) visible.value = false
        notifyVacant()
    }
    /** Finite cleanup survives UI scope cancellation and owns no service or saved state. */
    private fun launchCleanup(block: suspend () -> Unit) {
        val cleanupScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
        cleanupScope.launch {
            try { block() } finally { cleanupScope.cancel() }
        }
    }
    fun release() {
        whenVacant = null
        generation++
        entries.value.forEach { entry ->
            entry.source.close()
            entry.draft.value = ""
            entry.attachment.value = null
            entry.lastSubmission = null
            entry.lastSubmittedAttachment = null
            entry.store.clear()
            launchCleanup { runCatching { entry.source.end() } }
        }
        entries.value = emptyList()
        selected.value = null
        visible.value = false
    }
}

/** A completed create never auto-submits after any intervening background event. */
internal class SideChatForegroundGate {
    var isForeground = true
        private set
    private var generation = 0L
    fun capture(): Long = generation
    fun maySubmit(token: Long): Boolean = isForeground && token == generation
    fun background() { isForeground = false; generation++ }
    fun resume() { isForeground = true }
}
