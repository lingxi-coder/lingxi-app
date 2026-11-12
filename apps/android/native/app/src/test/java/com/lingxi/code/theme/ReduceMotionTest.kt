package com.lingxi.code.theme

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Reduce motion (U10) is the system's animator duration scale being exactly 0, which "Remove animations" sets. */
class ReduceMotionTest {
    @Test fun aZeroDurationScaleIsReduceMotion() {
        assertTrue(reduceMotionOf(0f))
    }

    @Test fun anyOtherScaleKeepsTheAnimations() {
        assertFalse(reduceMotionOf(1f))
        assertFalse(reduceMotionOf(0.5f))
        assertFalse(reduceMotionOf(10f))
    }
}
