package com.lingxi.code.cron

import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.util.UUID
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

class CronRunHistoryStoreInstrumentedTest {
    private lateinit var root: File
    private lateinit var store: CronRunHistoryStore
    private var clock = 1_000L
    private var sequence = 0

    @Before
    fun setUp() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        root = File(context.cacheDir, "cron-history-${UUID.randomUUID()}")
        store = CronRunHistoryStore(
            historyRoot = root,
            now = { clock++ },
            newId = { "run-${sequence++}" },
        )
    }

    @After
    fun tearDown() {
        root.deleteRecursively()
    }

    @Test
    fun unfinishedOccurrenceDeduplicatesAndTerminalRunAllowsNextOccurrence() {
        val scope = CronScope("global", null, "全局", root.path, "/workspace/global")
        val first = store.enqueue(scope, "task", "prompt", 900L)

        assertNotNull(first)
        assertNull(store.enqueue(scope, "task", "prompt", 900L))

        store.markTerminal(first!!.runId, CronRunStatus.Succeeded, resultText = "ok")
        assertNotNull(store.enqueue(scope, "task", "prompt", 1_800L))
    }

    @Test
    fun atomicIndexRoundTripsAndBoundsResultBytes() {
        val scope = CronScope("global", null, "全局", root.path, "/workspace/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markTerminal(
            run.runId,
            CronRunStatus.Succeeded,
            resultText = "🙂".repeat(MAX_CRON_RESULT_BYTES),
        )

        val reloaded = CronRunHistoryStore(root).record(run.runId)

        assertNotNull(reloaded)
        assertTrue(reloaded!!.resultText!!.toByteArray(Charsets.UTF_8).size <= MAX_CRON_RESULT_BYTES)
        assertEquals(CronRunStatus.Succeeded, reloaded.status)
    }

    @Test
    fun checksumFailureRecoversLastGoodBackupAndQuarantinesPrimary() {
        val scope = CronScope("global", null, "全局", root.path, "/workspace/global")
        val run = store.enqueue(scope, "task", "prompt", 900L)!!
        store.markTerminal(run.runId, CronRunStatus.Succeeded, resultText = "durable")
        File(root, "index.json").writeText(
            """{"version":2,"payload":"{}","sha256":"tampered"}""",
            Charsets.UTF_8,
        )

        val recovered = CronRunHistoryStore(root).record(run.runId)

        assertEquals("durable", recovered?.resultText)
        assertTrue(
            root.listFiles().orEmpty().any { it.name.startsWith("index.corrupt.") },
        )
        assertEquals(
            "durable",
            CronRunHistoryStore(root).record(run.runId)?.resultText,
        )
    }
}
