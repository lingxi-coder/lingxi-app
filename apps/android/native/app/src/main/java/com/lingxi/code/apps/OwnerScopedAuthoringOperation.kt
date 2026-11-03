package com.lingxi.code.apps

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/** Captured only at a confirmed Code entry; its work outlives the calling page. */
interface AppAuthoringOperationHost {
    fun <T> launch(work: suspend () -> T, complete: (Result<T>, Boolean) -> Unit)
}

/** One-use picker admissions survive Activity recreation for the life of this process. */
internal class AuthoringPickerTickets {
    private val entries = mutableMapOf<String, AppAuthoringOperationHost>()
    fun retain(operation: AppAuthoringOperationHost): String = synchronized(entries) {
        java.util.UUID.randomUUID().toString().also { entries[it] = operation }
    }
    fun take(ticket: String): AppAuthoringOperationHost? = synchronized(entries) { entries.remove(ticket) }
}

/** Consume the original admission even when the picker returns to a recreated page. */
internal fun <T : Any> dispatchAuthoringPickerResult(ticket: String?, result: T?,
    take: (String) -> AppAuthoringOperationHost?, start: (AppAuthoringOperationHost, T) -> Unit,
    missing: () -> Unit) {
    val operation = ticket?.let(take)
    if (result != null && operation != null) start(operation, result)
    else if (result != null) missing()
}

internal fun <T> launchOwnerScopedAuthoringOperation(
    scope: CoroutineScope,
    work: suspend () -> T,
    isCurrent: () -> Boolean,
    hiddenFailure: (Throwable) -> Unit,
    complete: (Result<T>, Boolean) -> Unit,
): Job = scope.launch {
    val result = try { Result.success(work()) }
    catch (cancelled: CancellationException) { throw cancelled }
    catch (failure: Throwable) { Result.failure(failure) }
    val current = isCurrent()
    if (!current) result.exceptionOrNull()?.let(hiddenFailure)
    complete(result, current)
}
