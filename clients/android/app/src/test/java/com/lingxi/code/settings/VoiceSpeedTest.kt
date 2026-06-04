package com.lingxi.code.settings

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Pure-logic checks for the VoicePage speed slider's [snapVoiceSpeed] grid
 * (0.1 steps clamped to 0.5x..2.0x — the iOS `Slider(in: 0.5...2, step: 0.1)`).
 */
class VoiceSpeedTest {

    @Test
    fun snapsToNearestTenth() {
        assertEquals(1.0f, snapVoiceSpeed(1.03f), 1e-4f)
        assertEquals(1.1f, snapVoiceSpeed(1.06f), 1e-4f)
        assertEquals(1.5f, snapVoiceSpeed(1.49f), 1e-4f)
    }

    @Test
    fun clampsBelowMin() {
        assertEquals(0.5f, snapVoiceSpeed(0.1f), 1e-4f)
        assertEquals(0.5f, snapVoiceSpeed(0.5f), 1e-4f)
    }

    @Test
    fun clampsAboveMax() {
        assertEquals(2.0f, snapVoiceSpeed(2.4f), 1e-4f)
        assertEquals(2.0f, snapVoiceSpeed(2.0f), 1e-4f)
    }
}
