package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.TurnRecoveryStateDto
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ConversationBackgroundExecutionTest {

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
