package com.lingxi.code.notify

import com.lingxi.code.model.NotifConfig
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the borrowed upstream constants and the gate mapping.
 *
 * These numbers are not ours to choose: they are Claude Code 2.1.270's `RJe`
 * and `DEFAULT_GLOBAL_CONFIG.messageIdleNotifThresholdMs`, read out of the
 * shipped binary. A change here is a deliberate divergence, not a tweak.
 */
class NotificationPolicyTest {

    @Test
    fun constantsMatchUpstream() {
        assertEquals(60_000L, NotificationPolicy.DEFAULT_IDLE_NOTIF_THRESHOLD_MS)
        assertEquals(6_000L, NotificationPolicy.PERMISSION_PROMPT_NOTIFY_DELAY_MS)
    }

    @Test
    fun thresholdIsClampedRatherThanTrusted() {
        // The floor keeps a mistyped `1` from turning every finished turn into
        // an instant banner; the ceiling keeps a mistyped value from silently
        // disabling the notification instead of saying so through `enabled`.
        assertEquals(5_000L, NotificationPolicy.clampIdleThreshold(1L))
        assertEquals(3_600_000L, NotificationPolicy.clampIdleThreshold(Long.MAX_VALUE))
        assertEquals(60_000L, NotificationPolicy.clampIdleThreshold(60_000L))
    }

    @Test
    fun everyKindDefaultsOnAndEveryKindRespectsTheMasterSwitch() {
        val on = NotifConfig()
        val off = NotifConfig(enabled = false)
        for (kind in NotificationKind.entries) {
            assertTrue("$kind defaults on", on.allows(kind))
            assertFalse("$kind must respect the master switch", off.allows(kind))
        }
    }

    @Test
    fun inputNeededGatesBothPermissionAndQuestionsButNotIdle() {
        val config = NotifConfig(inputNeededNotifEnabled = false)
        assertFalse(config.allows(NotificationKind.PermissionPrompt))
        assertFalse(config.allows(NotificationKind.AgentNeedsInput))
        assertTrue(config.allows(NotificationKind.IdlePrompt))
        assertTrue(config.allows(NotificationKind.AgentCompleted))
    }

    @Test
    fun taskCompleteGatesOnlyFinishedTasks() {
        val config = NotifConfig(taskCompleteNotifEnabled = false)
        assertFalse(config.allows(NotificationKind.AgentCompleted))
        assertTrue(config.allows(NotificationKind.PermissionPrompt))
    }

    @Test
    fun wireNamesAreUpstreamsDiscriminators() {
        // The port's CLI already fires the `Notification` hook with these exact
        // strings (`idle_notify.rs`, `permission_prompt_notify.rs`); a third
        // spelling here would be the drift this file exists to prevent.
        assertEquals("idle_prompt", NotificationKind.IdlePrompt.wireName)
        assertEquals("permission_prompt", NotificationKind.PermissionPrompt.wireName)
        assertEquals("agent_needs_input", NotificationKind.AgentNeedsInput.wireName)
        assertEquals("agent_completed", NotificationKind.AgentCompleted.wireName)
    }
}
