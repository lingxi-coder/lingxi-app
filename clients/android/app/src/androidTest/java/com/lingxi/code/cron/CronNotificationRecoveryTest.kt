package com.lingxi.code.cron

import android.Manifest
import android.app.NotificationManager
import android.os.Build
import android.os.SystemClock
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.util.UUID
import org.junit.Assert.assertEquals
import org.junit.Test

class CronNotificationRecoveryTest {
    @Test
    fun restartPostsOnlyPendingResultsOnceWithoutExecutingTasks() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.POST_NOTIFICATIONS)
        }
        val root = File(context.cacheDir, "cron-notifications-${UUID.randomUUID()}")
        val manager = context.getSystemService(NotificationManager::class.java)
        val posted = mutableListOf<String>()
        try {
            val history = CronRunHistoryStore(root)
            val scope = CronScope("global", null, "None", root.path, "/workspace/global")
            fun terminal(task: String, policy: String, status: CronRunStatus): String {
                val run = history.enqueue(scope, task, "Saved prompt", 900L)!!
                history.attachExecution(run.runId, notificationPolicy = policy)
                history.markTerminal(run.runId, status, resultText = "Saved result")
                return run.runId
            }
            val success = terminal("success", "all", CronRunStatus.Succeeded)
            val failure = terminal("failure", "failed", CronRunStatus.Failed)
            terminal("off", "none", CronRunStatus.Failed)
            terminal("filtered", "failed", CronRunStatus.Succeeded)
            val queued = history.enqueue(scope, "queued", "prompt", 900L)!!
            repeat(2) {
                CronRunHistoryStore(root).postPendingNotifications { record ->
                    posted.add(record.runId)
                    CronNotifications.postResult(context, record.prompt, record.resultText!!,
                        "cron-${record.runId}", record.runId)
                }
            }
            assertEquals(setOf(success, failure), posted.toSet())
            assertEquals(2, posted.size)
            val expectedTags = posted.map { "cron-$it" }.toSet()
            val deadline = SystemClock.elapsedRealtime() + 5_000L
            fun actualTags() = manager.activeNotifications.map { it.tag }.filter { it in expectedTags }.toSet()
            while (actualTags() != expectedTags && SystemClock.elapsedRealtime() < deadline) {
                SystemClock.sleep(50L)
            }
            assertEquals(expectedTags, actualTags())
            assertEquals(CronRunStatus.Queued, history.record(queued.runId)?.status)
        } finally {
            posted.forEach { runId -> manager.cancel("cron-$runId", "cron-$runId".hashCode()) }
            root.deleteRecursively()
        }
    }
}
