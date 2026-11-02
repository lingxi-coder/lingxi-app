package com.lingxi.code.conversation

import org.junit.Assert.assertEquals
import org.junit.Test

class ImagePreviewGeometryTest {
    @Test fun largeAndLongImagesHaveBoundedDisplayAllocation() {
        for ((width, height) in listOf(8000 to 6000, 800 to 20000, Int.MAX_VALUE to Int.MAX_VALUE, 1 to 1)) {
            val (w, h) = imagePreviewDecodeSize(width, height)
            org.junit.Assert.assertTrue(w in 1..4096 && h in 1..4096 && w.toLong() * h <= 4_194_304)
        }
        assertEquals(40 to 30, imagePreviewDecodeSize(40, 30))
    }
    private val portrait = ImagePreviewGeometry(780f, 1688f, 360f, 600f)

    @Test fun portraitAndLandscapeFitWithoutCroppingOrUpscaling() {
        assertEquals(600f / 1688, portrait.fit, .0001f)
        assertEquals(360f / 1600, ImagePreviewGeometry(1600f, 900f, 360f, 600f).fit, .0001f)
        assertEquals(1f, ImagePreviewGeometry(40f, 40f, 360f, 600f).fit, 0f)
    }
    @Test fun zoomKeepsTheContentUnderThePinchCentroid() {
        val current = ImagePreviewTransform(1f, 20f, -100f)
        val zoomed = portrait.zoom(current, 2f, 50f, -40f)
        assertEquals((50f - current.x) / current.scale!!, (50f - zoomed.x) / zoomed.scale!!, .0001f)
        assertEquals((-40f - current.y) / current.scale, (-40f - zoomed.y) / zoomed.scale, .0001f)
    }
    @Test fun panningIsBoundedAndSmallImagesStayCentered() {
        val panned = portrait.zoom(ImagePreviewTransform(1f), 1f, panX = 10_000f, panY = -10_000f)
        assertEquals((780f - 360) / 2, panned.x, 0f)
        assertEquals(-(1688f - 600) / 2, panned.y, 0f)
        val fitted = portrait.zoom(panned, portrait.fit)
        assertEquals(0f, fitted.x, .0001f)
        assertEquals(0f, fitted.y, .0001f)
    }
    @Test fun veryLongImagesCanFitBelowTenPercent() {
        val long = ImagePreviewGeometry(800f, 20_000f, 360f, 600f)
        assertEquals(.03f, long.minimum, .0001f)
        assertEquals(long.minimum, long.zoom(ImagePreviewTransform(), .001f).scale!!, .0001f)
    }
    @Test fun zoomButtonsHaveFiniteLimitsAndRespectFit() {
        assertEquals(.5f, portrait.step(ImagePreviewTransform(), true).scale!!, 0f)
        assertEquals(8f, portrait.step(ImagePreviewTransform(8f), true).scale!!, 0f)
        assertEquals(portrait.minimum, portrait.step(ImagePreviewTransform(.1f), false).scale!!, 0f)
    }
    @Test fun missingViewportCannotCreateInvalidCoordinates() {
        val geometry = portrait.copy(height = 0f)
        val initial = ImagePreviewTransform()
        assertEquals(initial, geometry.zoom(initial, 2f))
    }
}
