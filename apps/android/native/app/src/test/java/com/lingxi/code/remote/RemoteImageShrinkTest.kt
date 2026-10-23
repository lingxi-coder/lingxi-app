package com.lingxi.code.remote

import org.junit.Assert.assertEquals
import org.junit.Test

class RemoteImageShrinkTest {
    @Test fun fitWithinKeepsSmallImagesAndScalesLargeOnesByTheLongEdge() {
        assertEquals(800 to 600, fitWithin(800, 600, 1568))
        assertEquals(1568 to 784, fitWithin(4000, 2000, 1568))
        assertEquals(784 to 1568, fitWithin(2000, 4000, 1568))
    }

    @Test fun fitWithinNeverReturnsAnEmptyEdge() {
        assertEquals(1568 to 1, fitWithin(100000, 10, 1568))
    }

    @Test fun shrinkLowersQualityFirstThenScaleAndStartsOver() {
        assertEquals(75 to 1.0, nextShrinkAttempt(90, 1.0))
        assertEquals(60 to 1.0, nextShrinkAttempt(75, 1.0))
        assertEquals(45 to 1.0, nextShrinkAttempt(60, 1.0))
        // At the quality floor the picture shrinks and the quality starts over.
        assertEquals(80 to 0.75, nextShrinkAttempt(45, 1.0))
        assertEquals(80 to 0.5625, nextShrinkAttempt(40, 0.75))
    }
}
