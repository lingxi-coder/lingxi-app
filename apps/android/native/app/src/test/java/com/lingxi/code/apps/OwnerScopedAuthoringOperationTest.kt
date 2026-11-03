package com.lingxi.code.apps

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test

class OwnerScopedAuthoringOperationTest {
    @Test fun admittedImportCompletesAfterPageExitWithoutOpeningAStaleResult() = runBlocking {
        val owner = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val page = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val started = CompletableDeferred<Unit>()
        val finishCopy = CompletableDeferred<Unit>()
        var pageGeneration = 0
        val originalGeneration = pageGeneration
        var imported = false
        var opened = false
        val job = launchOwnerScopedAuthoringOperation(owner,
            work = { started.complete(Unit); finishCopy.await(); imported = true; "app-a" },
            isCurrent = { pageGeneration == originalGeneration }, hiddenFailure = { error("unexpected failure") },
            complete = { result, visible -> if (visible) opened = result.getOrThrow() == "app-a" })
        started.await()
        pageGeneration++ // Leave and reopen the page; a boolean visibility check would match again.
        pageGeneration++
        page.cancel()
        assertFalse(job.isCancelled)
        finishCopy.complete(Unit)
        job.join()
        assertTrue(imported)
        assertFalse(opened)
        owner.cancel()
    }

    @Test fun admittedPublishKeepsItsReceiptAndCannotUpdateANewerPage() = runBlocking {
        val owner = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val uploaded = CompletableDeferred<Unit>()
        val reply = CompletableDeferred<Unit>()
        var current = true
        var receipt = ""
        var displayed = false
        val job = launchOwnerScopedAuthoringOperation(owner,
            work = { uploaded.complete(Unit); reply.await(); receipt = "published-a"; receipt },
            isCurrent = { current }, hiddenFailure = { error("unexpected failure") },
            complete = { result, visible -> if (visible) displayed = result.getOrThrow() == "published-a" })
        uploaded.await()
        current = false
        reply.complete(Unit)
        job.join()
        assertEquals("published-a", receipt)
        assertFalse(displayed)
        owner.cancel()
    }

    @Test fun currentPageReceivesSuccessAndHiddenFailureStaysWithItsOwner() = runBlocking {
        val owner = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        var opened = ""
        var ownerError = ""
        val success = launchOwnerScopedAuthoringOperation(owner,
            work = { "app-a" }, isCurrent = { true }, hiddenFailure = { ownerError = it.message.orEmpty() },
            complete = { result, visible -> if (visible) opened = result.getOrThrow() })
        success.join()
        assertEquals("app-a", opened)
        val failure = launchOwnerScopedAuthoringOperation<String>(owner,
            work = { error("import failed") }, isCurrent = { false }, hiddenFailure = { ownerError = it.message.orEmpty() },
            complete = { _, visible -> if (visible) opened = "wrong-page" })
        failure.join()
        assertEquals("import failed", ownerError)
        assertEquals("app-a", opened)
        owner.cancel()
    }

    @Test fun recreatedRootCannotConsumeTheOldPagesFailureOrSuccess() = runBlocking {
        val owner = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val oldRootAlive = java.util.concurrent.atomic.AtomicBoolean(true)
        val release = CompletableDeferred<Unit>()
        var oldOwnerError = ""
        var oldPageUpdated = false
        val job = launchOwnerScopedAuthoringOperation<String>(owner,
            work = { release.await(); error("failed after recreation") },
            isCurrent = { oldRootAlive.get() }, hiddenFailure = { oldOwnerError = it.message.orEmpty() },
            complete = { _, visible -> if (visible) oldPageUpdated = true })
        oldRootAlive.set(false)
        release.complete(Unit)
        job.join()
        assertEquals("failed after recreation", oldOwnerError)
        assertFalse(oldPageUpdated)
        val success = launchOwnerScopedAuthoringOperation(owner,
            work = { "completed" }, isCurrent = { oldRootAlive.get() }, hiddenFailure = { error("unexpected") },
            complete = { _, visible -> if (visible) oldPageUpdated = true })
        success.join()
        assertFalse(oldPageUpdated)
        owner.cancel()
    }

    @Test fun pickerTicketSurvivesRootRecreationAndIsConsumedOnlyOnce() = runBlocking {
        val owner = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val tickets = AuthoringPickerTickets()
        val oldRootAlive = java.util.concurrent.atomic.AtomicBoolean(true)
        var copied = false
        var oldPageOpened = false
        val original = object : AppAuthoringOperationHost {
            override fun <T> launch(work: suspend () -> T, complete: (Result<T>, Boolean) -> Unit) {
                launchOwnerScopedAuthoringOperation(owner, work, { oldRootAlive.get() }, { error("unexpected") }, complete)
            }
        }
        val savedPickerId = tickets.retain(original)
        oldRootAlive.set(false)
        val restoredPickerId = savedPickerId
        dispatchAuthoringPickerResult(restoredPickerId, "chosen-uri", tickets::take,
            start = { operation, _ -> operation.launch(work = { copied = true; "imported" }) { _, current ->
                if (current) oldPageOpened = true
            } }, missing = { error("original ticket was lost") })
        assertTrue(copied)
        assertFalse(oldPageOpened)
        assertNull(tickets.take(restoredPickerId))
        owner.cancel()
    }
}
