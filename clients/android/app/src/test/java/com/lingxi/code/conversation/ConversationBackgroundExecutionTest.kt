package com.lingxi.code.conversation

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
        assertTrue(shouldStopConversationService(promoted = true, promotionAcknowledged = false))
        assertFalse(shouldStopConversationService(promoted = true, promotionAcknowledged = true))
        assertTrue(shouldStopConversationService(promoted = false, promotionAcknowledged = true))
    }
}
