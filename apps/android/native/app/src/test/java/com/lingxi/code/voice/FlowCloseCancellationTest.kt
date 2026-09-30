package com.lingxi.code.voice

import org.junit.Assert.assertEquals
import org.junit.Test

class FlowCloseCancellationTest {
    @Test
    fun explicitCloseAndVisibilityExitCancelOncePerVisibleSession() {
        val closeCancellation = FlowCloseCancellation()
        var cancellationCount = 0
        val onCancel = { cancellationCount += 1 }

        closeCancellation.onVisibilityChanged(false, onCancel)
        assertEquals(0, cancellationCount)

        closeCancellation.onVisibilityChanged(true, onCancel)
        closeCancellation.cancelOnce(onCancel)
        closeCancellation.onVisibilityChanged(false, onCancel)
        closeCancellation.onVisibilityChanged(false, onCancel)
        assertEquals(1, cancellationCount)

        closeCancellation.onVisibilityChanged(true, onCancel)
        closeCancellation.onVisibilityChanged(false, onCancel)
        assertEquals(2, cancellationCount)
    }
}
