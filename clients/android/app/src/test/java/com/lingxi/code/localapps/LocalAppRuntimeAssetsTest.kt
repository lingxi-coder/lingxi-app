package com.lingxi.code.localapps

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalAppRuntimeAssetsTest {
    /**
     * `prepare` is evaluated inside the `viewModel { initializer { … } }` block
     * that Compose runs on the MAIN thread, so a cold first launch must not
     * park it for the length of a ~200 MB asset extraction. The fake stage
     * returns on its own so a regression fails the assertions instead of
     * hanging the suite.
     */
    @Test
    fun `a cold staging run does not block the caller past its budget`() {
        val started = CountDownLatch(1)
        val runs = AtomicInteger()
        val stage = {
            runs.incrementAndGet()
            started.countDown()
            Thread.sleep(STAGE_MS)
            STAGED_ROOT
        }

        val begin = System.nanoTime()
        val first = LocalAppRuntimeAssets.prepareWithin(BUDGET_MS, stage)
        val elapsedMs = (System.nanoTime() - begin) / 1_000_000

        assertNull("a staging run that outlives the budget must not be awaited", first)
        assertTrue("caller was parked for ${elapsedMs}ms", elapsedMs < STAGE_MS / 2)
        assertTrue("staging must actually have started", started.await(5, TimeUnit.SECONDS))

        // The same run keeps going; a later engine build joins it rather than
        // extracting the tree a second time.
        val second = LocalAppRuntimeAssets.prepareWithin(10_000) { error("must not re-stage") }
        assertEquals(STAGED_ROOT, second)
        assertEquals(1, runs.get())

        // Memoised once it succeeded.
        assertEquals(STAGED_ROOT, LocalAppRuntimeAssets.prepareWithin(0) { error("must not re-stage") })
    }

    private companion object {
        const val BUDGET_MS = 150L
        const val STAGE_MS = 2_000L
        const val STAGED_ROOT = "/data/user/0/com.lingxi.code/files/local-app-runtime"
    }
}
