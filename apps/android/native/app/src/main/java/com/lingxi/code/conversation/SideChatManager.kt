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
    val attachment = MutableStateFlow<ComposerAttachment?>(null)
    val rejected = MutableStateFlow<List<RejectedComposerDraft>>(emptyList())
    private var draftRevision = 0L
    private var attachmentRevision = 0L
    private var retired = false
    var pendingRestore: ComposerRestoreGate? = null
    val closing = MutableStateFlow(false)
    var endInFlight = false

    fun updateDraft(value: String) { draftRevision++; draft.value = value }
    fun updateAttachment(value: ComposerAttachment?) { attachmentRevision++; attachment.value = value }
    fun submit(text: String, image: ComposerAttachment?, clearImmediately: Boolean = false) {
        val item = RejectedComposerDraft(text, image, viewModel.state.value.visualizationChip)
        fun preserve() { if (!retired) rejected.value = rejected.value + item }
        val gate = ComposerRestoreGate(
            revision = { Triple(draftRevision, attachmentRevision, viewModel.visualizationDraftRevision()) },
            preserve = ::preserve,
            restore = {
                if (!retired && draft.value.isEmpty() && attachment.value == null && viewModel.state.value.visualizationChip == null) {
                    updateDraft(item.text)
                    updateAttachment(item.attachment)
                    viewModel.restoreVisualizationDraft(item.visualization)
                } else preserve()
            },
        )
        pendingRestore = gate
        viewModel.send(text, images = image?.toImageRef()?.let(::listOf).orEmpty(), onNotAdmitted = gate::rejected)
        if (clearImmediately) { gate.cleared(); pendingRestore = null }
    }
    fun restoreRejected() {
        val item = rejected.value.firstOrNull() ?: return
        if (draft.value.isNotEmpty() || attachment.value != null || viewModel.state.value.visualizationChip != null) return
        updateDraft(item.text)
        updateAttachment(item.attachment)
        viewModel.restoreVisualizationDraft(item.visualization)
        rejected.value = rejected.value.drop(1)
    }
    fun retire() { retired = true; pendingRestore = null; rejected.value = emptyList() }
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
    val isCreating = MutableStateFlow(false)
    private val foreground = SideChatForegroundGate()
    val hasLiveChats: Boolean get() = creating > 0 || entries.value.isNotEmpty()
    var whenVacant: (() -> Unit)? = null
    private fun notifyVacant() {
        if (!hasLiveChats) whenVacant?.also { whenVacant = null }?.invoke()
    }
    fun create(
        input: String,
        draftOnFailure: String?,
        newSource: () -> SideChatSource,
        restore: () -> Unit,
    ) {
        failedDraft.value = null
        error.value = null
        creating++
        isCreating.value = true
        val originalGeneration = generation
        val foregroundToken = foreground.capture()
        scope.launch {
            withContext(NonCancellable) {
                var pendingSource: SideChatSource? = null
                try {
                    val source = newSource().also { pendingSource = it }
                    source.create(UUID.randomUUID().toString())
                    if (generation != originalGeneration) { source.end(); return@withContext }
                    val entry = SideChatEntry(source, ChatViewModel(source))
                    source.onEnded = { scope.launch { retire(entry) } }
                    entries.value = entries.value + entry
                    selected.value = entry
                    visible.value = true
                    source.setForeground(foreground.isForeground)
                    if (input.isNotEmpty()) {
                        if (foreground.maySubmit(foregroundToken)) entry.submit(input, null, clearImmediately = true)
                        else entry.updateDraft(input)
                    }
                } catch (failure: Throwable) {
                    pendingSource?.close()
                    error.value = failure.message ?: "Unable to create side chat"
                    failedDraft.value = draftOnFailure
                    restore()
                } finally {
                    creating--
                    isCreating.value = creating > 0
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
        entry.retire()
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
            entry.retire()
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
