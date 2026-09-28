package com.lingxi.code.conversation

import com.lingxi.code.bindings.PermissionRequest
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.launch

internal inline fun submitTurnCancellation(
    permissionIngress: PermissionIngress,
    submit: () -> Unit,
) {
    val permissionSnapshot = permissionIngress.beginCancellation()
    try {
        submit()
    } catch (error: Throwable) {
        permissionIngress.restoreAfterFailedCancellation(permissionSnapshot)
        throw error
    }
}

/**
 * Process-wide FIFO for permission callbacks. Workflow children can request
 * permission while the main turn is idle, so main-turn lifecycle events must
 * not suppress or clear their prompts. The engine's correlated
 * `PermissionRequestResolved` event removes the exact request that completed.
 */
internal class PermissionIngress(
    private val permissions: MutableStateFlow<PermissionPromptState?>,
    private val strings: ConversationStrings = DefaultConversationStrings,
) {
    /** Multiple workflow children can park independently; preserve callback order. */
    private val queued = linkedMapOf<ULong, PermissionPromptState>()

    internal class CancellationSnapshot internal constructor()

    @Synchronized
    fun beginTurn() = Unit

    @Synchronized
    fun confirmTurnStarted() = Unit

    @Synchronized
    fun beginCancellation(): CancellationSnapshot = CancellationSnapshot()

    @Synchronized
    fun restoreAfterFailedCancellation(@Suppress("UNUSED_PARAMETER") snapshot: CancellationSnapshot) = Unit

    @Synchronized
    fun endTurn() = Unit

    @Synchronized
    fun publish(request: PermissionRequest) {
        queued[request.requestId] = permissionRequestToPrompt(request, strings)
        publishHead()
    }

    @Synchronized
    fun resolve(requestId: ULong) {
        queued.remove(requestId)
        publishHead()
    }

    private fun publishHead() {
        permissions.value = queued.values.firstOrNull()
    }
}

/**
 * Non-blocking callback ingress with lossless, ordered delivery to a Flow.
 *
 * Native event callbacks must return promptly, while assistant deltas and turn
 * boundaries must never be discarded. An unlimited channel decouples the
 * callback thread from a single suspending SharedFlow pump: slow collectors add
 * bounded-by-turn memory pressure instead of blocking Rust or dropping tokens.
 */
internal class LosslessEventRelay<T>(
    scope: CoroutineScope,
) {
    private val queue = Channel<T>(capacity = Channel.UNLIMITED)
    private val shared = MutableSharedFlow<T>(extraBufferCapacity = 64)
    val events: SharedFlow<T> = shared.asSharedFlow()

    init {
        scope.launch {
            for (event in queue) shared.emit(event)
        }
    }

    /** Safe for a native callback thread; preserves FIFO order without suspension. */
    fun offer(event: T): Boolean = queue.trySend(event).isSuccess

    fun close() {
        queue.close()
    }
}
