package com.lingxi.code.voice.audio

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

internal enum class AndroidMediaPermissionOutcome {
    Granted,
    Denied,
    Unavailable,
    Busy,
}

/** ActivityResult bridge for media requests that can originate in the native host. */
internal class AndroidMediaPermissionCoordinator(
    private val dispatcher: CoroutineDispatcher = Dispatchers.Main.immediate,
    private val timeoutMs: Long = 60_000L,
) {
    private data class ActivityHost(
        val owner: Any,
        val isGranted: (String) -> Boolean,
        val isForeground: () -> Boolean,
        val requestPermissions: (Array<String>) -> Unit,
    )

    private data class PendingRequest(
        val host: ActivityHost,
        val permissions: Set<String>,
        val result: CompletableDeferred<Map<String, Boolean>>,
    )

    private var host: ActivityHost? = null
    private var pending: PendingRequest? = null

    fun attach(
        owner: Any,
        isGranted: (String) -> Boolean,
        isForeground: () -> Boolean,
        requestPermissions: (Array<String>) -> Unit,
    ) {
        val previous = host
        if (previous?.owner === owner) return
        if (previous != null) detach(previous.owner)
        host = ActivityHost(owner, isGranted, isForeground, requestPermissions)
    }

    fun detach(owner: Any) {
        val current = host
        if (current?.owner !== owner) return
        host = null
        pending?.takeIf { it.host.owner === owner }?.let { request ->
            pending = null
            request.result.complete(emptyMap())
        }
    }

    suspend fun request(permissions: Set<String>): AndroidMediaPermissionOutcome = withContext(dispatcher) {
        if (permissions.isEmpty()) return@withContext AndroidMediaPermissionOutcome.Granted
        val current = host ?: return@withContext AndroidMediaPermissionOutcome.Unavailable
        val missing = permissions.filterNot(current.isGranted).toSet()
        if (missing.isEmpty()) return@withContext AndroidMediaPermissionOutcome.Granted
        if (!current.isForeground()) return@withContext AndroidMediaPermissionOutcome.Unavailable
        if (pending != null) return@withContext AndroidMediaPermissionOutcome.Busy

        val request = PendingRequest(current, missing, CompletableDeferred())
        pending = request
        try {
            current.requestPermissions(missing.sorted().toTypedArray())
        } catch (_: Throwable) {
            if (pending === request) pending = null
            return@withContext AndroidMediaPermissionOutcome.Unavailable
        }

        // ActivityResult permissions cannot be cancelled by the caller. Keep
        // the slot until this launch's OS result arrives, even after timeout or
        // coroutine cancellation, so a late result cannot answer a newer open.
        val grants = withTimeoutOrNull(timeoutMs) { request.result.await() }
            ?: return@withContext AndroidMediaPermissionOutcome.Unavailable
        if (host?.owner !== current.owner) return@withContext AndroidMediaPermissionOutcome.Unavailable
        if (permissions.all(current.isGranted) && missing.all { grants[it] == true }) {
            AndroidMediaPermissionOutcome.Granted
        } else {
            AndroidMediaPermissionOutcome.Denied
        }
    }

    /** Must be called by the ActivityResult callback for the same Activity owner. */
    fun onPermissionResult(owner: Any, grants: Map<String, Boolean>) {
        val request = pending?.takeIf { it.host.owner === owner } ?: return
        pending = null
        request.result.complete(grants.filterKeys { it in request.permissions })
    }

    companion object {
        val shared by lazy(LazyThreadSafetyMode.SYNCHRONIZED) {
            AndroidMediaPermissionCoordinator(dispatcher = Dispatchers.Main.immediate)
        }
    }
}
