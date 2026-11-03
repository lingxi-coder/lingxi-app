package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class ConversationBackgroundExecutionTest {
    @Test fun staleStopUsesCurrentTasksWithoutResurrectingFinishedTasks() {
        val spec = ConversationRecoverySpec(null, null, com.lingxi.code.model.SessionMode.Chat,
            com.lingxi.code.settings.LinuxRuntimeMode.MobileLinux, workspaceKey = "global",
            transcriptPath = "/guest/exact.jsonl")
        val oldStop = ConversationBackgroundSnapshot("session-a", 7L, "Working", spec)
        val running = oldStop.copy(activeTaskIds = setOf("task-x"))
        assertEquals(setOf("task-x"), reconcileConversationAction(oldStop,
            mapOf(spec.ownerKey to oldStop, "ui" to running)).activeTaskIds)
        assertTrue(reconcileConversationAction(running,
            mapOf(spec.ownerKey to running, "ui" to oldStop)).activeTaskIds.isEmpty())
    }

    @Test fun slowDurableFlushDoesNotHoldOwnerLock() {
        val lock = Any()
        val entered = java.util.concurrent.CountDownLatch(1)
        val finish = java.util.concurrent.CountDownLatch(1)
        val otherFinished = java.util.concurrent.CountDownLatch(1)
        val failure = java.util.concurrent.atomic.AtomicReference<Throwable?>()
        val writer = Thread {
            try {
                flushConversationRecovery(stage = {
                    synchronized(lock) {
                        val flush: () -> Boolean = { entered.countDown(); finish.await(); true }
                        flush
                    }
                }, revalidate = { synchronized(lock) { Unit } })
            } catch (problem: Throwable) { failure.set(problem) }
        }
        val other = Thread { synchronized(lock) { otherFinished.countDown() } }
        writer.start()
        try {
            assertTrue(entered.await(3, java.util.concurrent.TimeUnit.SECONDS))
            other.start()
            assertTrue(otherFinished.await(3, java.util.concurrent.TimeUnit.SECONDS))
        } finally {
            finish.countDown()
            writer.join(3_000)
            if (other.isAlive) other.join(3_000)
        }
        assertNull(failure.get())
        assertFalse(writer.isAlive)
    }

    @Test fun newTurnAlignsUiAndServiceSnapshotsAndRejectsStaleStop() {
        val spec = ConversationRecoverySpec(null, null, com.lingxi.code.model.SessionMode.Chat,
            com.lingxi.code.settings.LinuxRuntimeMode.MobileLinux, workspaceKey = "global",
            transcriptPath = "/guest/exact.jsonl")
        val old = ConversationBackgroundSnapshot("session-a", 6L, "Working", spec, setOf("background-task"))
        val next = ConversationBackgroundSnapshot("session-a", 7L, "Working", spec)
        val updated = submissionOwnerSnapshots("ui", next, mapOf(spec.ownerKey to old, "ui" to next))
        assertEquals(7L, updated.getValue(spec.ownerKey).turnId)
        assertEquals(setOf("background-task"), updated.getValue(spec.ownerKey).activeTaskIds)
        assertTrue(runCatching { reconcileConversationAction(old, updated) }.isFailure)
        assertEquals(next, reconcileConversationAction(next, updated))
    }

    @Test fun incompleteStopSnapshotRetainsOnlyItsOwnConfirmedSource() {
        val spec = ConversationRecoverySpec(null, null, com.lingxi.code.model.SessionMode.Chat,
            com.lingxi.code.settings.LinuxRuntimeMode.MobileLinux, workspaceKey = "global",
            transcriptPath = "/guest/exact.jsonl")
        val stored = ConversationBackgroundSnapshot("session-a", 7L, "Working", spec, setOf("task-a"))
        val incomplete = stored.copy(recoverySpec = spec.copy(transcriptPath = null))
        assertEquals("/guest/exact.jsonl", reconcileConversationSnapshot(incomplete, listOf(stored)).recoverySpec?.transcriptPath)
        assertEquals(setOf("task-a"), reconcileConversationSnapshot(incomplete, listOf(stored)).activeTaskIds)
        assertNull(reconcileConversationSnapshot(incomplete.copy(sessionId = "session-b"), listOf(stored)).recoverySpec?.transcriptPath)
        assertTrue(runCatching {
            reconcileConversationSnapshot(stored.copy(recoverySpec = spec.copy(transcriptPath = "/guest/other.jsonl")), listOf(stored))
        }.isFailure)
    }

    @Test
    fun cancellationRequiresItsOwnCorrelatedOwnerAndTurn() {
        val owner = ConversationRecoverySpec(null, null, com.lingxi.code.model.SessionMode.Chat,
            com.lingxi.code.settings.LinuxRuntimeMode.MobileLinux, workspaceKey = "global")
        val target = ConversationBackgroundSnapshot("session-a", 7L, "Working", owner)
        assertNull(cancellationTarget(null))
        assertNull(cancellationTarget(target.copy(turnId = null)))
        assertNull(cancellationTarget(target.copy(turnId = 0L)))
        assertNull(cancellationTarget(target.copy(recoverySpec = null)))
        assertSame(target, cancellationTarget(target))
    }

    @Test
    fun promotionFailureClearsActiveStateAndRetries() {
        var starts = 0
        var stops = 0
        val retries = mutableListOf<() -> Unit>()
        val lease = ConversationServiceLease(
            startService = {
                starts++
                true
            },
            stopService = { stops++ },
            scheduleRetry = { retry -> retries += retry },
        )

        lease.setTurnActive(true)
        assertFalse(lease.isActive)
        assertTrue(lease.isStartPending)

        lease.onPromotionResult(success = false)
        assertFalse(lease.isActive)
        assertFalse(lease.isStartPending)
        assertEquals(1, retries.size)

        retries.single().invoke()
        assertEquals(2, starts)
        assertTrue(lease.isStartPending)

        lease.onPromotionResult(success = true)
        assertTrue(lease.isActive)
        assertFalse(lease.isStartPending)

        lease.setTurnActive(false)
        assertFalse(lease.isActive)
        assertEquals(1, stops)
    }

    @Test
    fun successfulPromotionWithoutLiveLeaseMustStopService() {
        assertTrue(
            shouldStopConversationService(
                action = ConversationTurnService.ACTION_START,
                promoted = true,
                promotionAcknowledged = false,
                snapshot = null,
            ),
        )
        assertFalse(
            shouldStopConversationService(
                action = ConversationTurnService.ACTION_START,
                promoted = true,
                promotionAcknowledged = true,
                snapshot = null,
            ),
        )
        assertTrue(
            shouldStopConversationService(
                action = ConversationTurnService.ACTION_START,
                promoted = false,
                promotionAcknowledged = true,
                snapshot = null,
            ),
        )
    }

    @Test
    fun redeliveredSnapshotDoesNotStopPromotedServiceWithoutHandshake() {
        val snapshot = ConversationBackgroundSnapshot(
            sessionId = "session-a",
            turnId = 7L,
            statusText = "Working",
        )

        assertFalse(
            shouldStopConversationService(
                action = ConversationTurnService.ACTION_START,
                promoted = true,
                promotionAcknowledged = false,
                snapshot = snapshot,
            ),
        )
        assertFalse(
            shouldStopConversationService(
                action = ConversationTurnService.ACTION_UPDATE,
                promoted = true,
                promotionAcknowledged = false,
                snapshot = snapshot,
            ),
        )
        assertTrue(
            shouldStopConversationService(
                action = ConversationTurnService.ACTION_UPDATE,
                promoted = true,
                promotionAcknowledged = false,
                snapshot = null,
            ),
        )
    }

    @Test
    fun coldWaitingOrAttachRunningDoesNotClaimHeadlessExecutorBeforeResume() {
        assertFalse(
            shouldMarkHeadlessExecutorActive(
                resumeSession = true,
                recoveryState = TurnRecoveryStateDto.WAITING_FOR_USER,
                stateIndex = 2L,
                minimumActionStateIndex = 2L,
            ),
        )
        assertFalse(
            shouldMarkHeadlessExecutorActive(
                resumeSession = true,
                recoveryState = TurnRecoveryStateDto.RUNNING,
                stateIndex = 1L,
                minimumActionStateIndex = 2L,
            ),
        )
        assertTrue(
            shouldMarkHeadlessExecutorActive(
                resumeSession = true,
                recoveryState = TurnRecoveryStateDto.RUNNING,
                stateIndex = 2L,
                minimumActionStateIndex = 2L,
            ),
        )
    }

    @Test
    fun recoveryWaitingRetainsOnlyAnExecutorProvenToBeRunning() {
        var retained = false
        retained = headlessExecutorActiveAfterRecoveryState(
            currentActive = retained,
            recoveryState = TurnRecoveryStateDto.RUNNING,
            resumeSession = true,
            stateIndex = 1L,
            minimumActionStateIndex = 2L,
        )
        retained = headlessExecutorActiveAfterRecoveryState(
            currentActive = retained,
            recoveryState = TurnRecoveryStateDto.RUNNING,
            resumeSession = true,
            stateIndex = 2L,
            minimumActionStateIndex = 2L,
        )
        retained = headlessExecutorActiveAfterRecoveryState(
            currentActive = retained,
            recoveryState = TurnRecoveryStateDto.WAITING_FOR_USER,
            resumeSession = true,
            stateIndex = 3L,
            minimumActionStateIndex = 2L,
        )
        assertTrue(retained)

        var coldWaiting = false
        coldWaiting = headlessExecutorActiveAfterRecoveryState(
            currentActive = coldWaiting,
            recoveryState = TurnRecoveryStateDto.RUNNING,
            resumeSession = true,
            stateIndex = 1L,
            minimumActionStateIndex = 2L,
        )
        coldWaiting = headlessExecutorActiveAfterRecoveryState(
            currentActive = coldWaiting,
            recoveryState = TurnRecoveryStateDto.WAITING_FOR_USER,
            resumeSession = true,
            stateIndex = 2L,
            minimumActionStateIndex = 2L,
        )
        assertFalse(coldWaiting)
    }
}
