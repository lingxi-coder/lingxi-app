package com.lingxi.code.computeruse

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ComputerUseAudioPolicyTest {
    @Test
    fun playbackTimeoutIncludesExpectedDurationAndGrace() {
        assertEquals(5_000L, audioPlaybackTimeoutMs(frameCount = 0, sampleRate = 48_000))
        assertEquals(5_001L, audioPlaybackTimeoutMs(frameCount = 1, sampleRate = 48_000))
        assertEquals(6_000L, audioPlaybackTimeoutMs(frameCount = 48_000, sampleRate = 48_000))
    }

    @Test
    fun playbackTimeoutHasSafetyCeiling() {
        assertEquals(
            10 * 60 * 1_000L,
            audioPlaybackTimeoutMs(frameCount = Int.MAX_VALUE, sampleRate = 1),
        )
    }

    @Test
    fun microphoneForegroundTypeRequiresSettingAndRuntimePermission() {
        assertFalse(shouldActivateMicrophoneForegroundService(false, false))
        assertFalse(shouldActivateMicrophoneForegroundService(true, false))
        assertFalse(shouldActivateMicrophoneForegroundService(false, true))
        assertTrue(shouldActivateMicrophoneForegroundService(true, true))
    }
}
