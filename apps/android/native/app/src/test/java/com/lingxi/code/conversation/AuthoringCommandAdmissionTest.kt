package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.ClientCommand
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking

class AuthoringCommandAdmissionTest {
    @Test fun sourcePinsFirstSessionAndNeverLetsAnotherSessionInheritTheGrant() {
        val a = UUID.randomUUID().toString()
        val b = UUID.randomUUID().toString()
        val boundary = AuthoringSessionBoundary(null)
        assertFalse(boundary.permits(a))
        assertTrue(boundary.observe(a))
        assertTrue(boundary.permits("sess:$a"))
        assertFalse(boundary.observe(b))
        assertFalse(boundary.permits(b))
        assertTrue(boundary.permits(a))
        val restored = AuthoringSessionBoundary(a)
        assertFalse(restored.observe(b))
        assertTrue(restored.observe(a))
    }

    @Test fun pendingOwnerBlocksSideAndWorkflowButAllowsReadOnlyStatus() {
        assertTrue(runCatching { requireSideChatParentAdmission(false) }.exceptionOrNull() is SideChatNotAdmittedException)
        requireSideChatParentAdmission(true)
        assertFalse(maySubmitWhileAuthoringPending(ClientCommand.ResumeWorkflow("task")))
        assertFalse(maySubmitWhileAuthoringPending(ClientCommand.ForceCompact))
        assertTrue(maySubmitWhileAuthoringPending(ClientCommand.ListModels))
    }

    @Test fun retainedSelectionFencesNewAdmissionsUntilTheSameSourceIsReleased() {
        val a = AuthoringSelectionFence()
        val b = AuthoringSelectionFence()
        assertTrue(a.reserve())
        assertFalse(a.ready)
        assertFalse(a.reserve())
        assertTrue(b.ready)
        assertTrue(runCatching { requireSideChatParentAdmission(a.ready) }.isFailure)
        a.release()
        assertTrue(a.ready)
        assertTrue(a.reserve())
        a.release()
    }

    @Test fun delayedWorkspaceChangeDoesNotActivateOrSeedTheRetainedOwner() = runBlocking {
        val fence = AuthoringSelectionFence()
        val readStarted = CompletableDeferred<Unit>()
        val releaseRead = CompletableDeferred<Unit>()
        var liveCwd = "/source/a"
        var selected = "owner-b"
        var draft = "existing draft"
        val attempt = async {
            val snapshot = reserveValidatedAuthoringSelection(
                reserve = fence::reserve,
                readWorkspace = { readStarted.complete(Unit); releaseRead.await(); liveCwd },
                matches = { it == "/source/a" && liveCwd == it },
                release = fence::release,
            )
            if (snapshot != null) { selected = "owner-a"; draft = "seed"; fence.release() }
        }
        readStarted.await()
        assertFalse(fence.ready)
        liveCwd = "/source/other"
        releaseRead.complete(Unit)
        attempt.await()
        assertEquals("owner-b", selected)
        assertEquals("existing draft", draft)
        assertTrue(fence.ready)
    }

    @Test fun workspaceMutationAndSelectionExcludeEachOtherUntilNativeCompletes() = runBlocking {
        val fence = AuthoringSelectionFence()
        val entered = CompletableDeferred<Unit>()
        val finish = CompletableDeferred<Unit>()
        val mutation = async { fence.mutateWorkspace { entered.complete(Unit); finish.await(); "changed" } }
        entered.await()
        assertFalse(fence.reserve())
        assertFalse(fence.ready)
        finish.complete(Unit)
        assertEquals("changed", mutation.await())
        assertTrue(fence.reserve())
        assertTrue(runCatching { fence.mutateWorkspace { error("must not run") } }.isFailure)
        fence.release()
        assertTrue(fence.ready)
    }

    @Test fun uncertainWorkspaceMutationNeverReopensSelection() = runBlocking {
        val fence = AuthoringSelectionFence()
        assertTrue(runCatching { fence.mutateWorkspace<Unit> { error("native outcome unknown") } }.isFailure)
        assertFalse(fence.ready)
        assertFalse(fence.reserve())
        assertTrue(runCatching { fence.mutateWorkspace {} }.isFailure)
    }
}
