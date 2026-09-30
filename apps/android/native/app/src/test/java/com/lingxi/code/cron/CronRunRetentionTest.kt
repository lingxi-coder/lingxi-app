package com.lingxi.code.cron

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CronRunRetentionTest {
    @Test
    fun retainsTwentyNewestRunsPerScopedTask() {
        val records = (0 until 25).map { index ->
            record(
                id = "run-$index",
                taskId = "task",
                scopeId = "project",
                time = index.toLong(),
            )
        }

        val retained = retainCronHistory(records)

        assertEquals(20, retained.size)
        assertEquals("run-24", retained.first().runId)
        assertFalse(retained.any { it.runId == "run-0" })
    }

    @Test
    fun appliesGlobalCapAcrossTasks() {
        val records = (0 until 600).map { index ->
            record(
                id = "run-$index",
                taskId = "task-${index % 40}",
                scopeId = "project-${index % 3}",
                time = index.toLong(),
            )
        }

        assertEquals(500, retainCronHistory(records).size)
    }

    @Test
    fun activeRunsAreNeverEvictedByTerminalHistoryChurn() {
        val active = record(
            id = "active",
            taskId = "task-active",
            scopeId = "project",
            time = 0,
        ).copy(status = CronRunStatus.Queued)
        val terminal = (1..600).map { index ->
            record(
                id = "run-$index",
                taskId = "task-${index % 40}",
                scopeId = "project",
                time = index.toLong(),
            )
        }

        val retained = retainCronHistory(terminal + active)

        assertTrue(retained.any { it.runId == "active" })
        assertEquals(500, retained.size)
    }

    @Test
    fun utf8TruncationNeverSplitsSurrogatePairOrExceedsLimit() {
        val truncated = truncateUtf8("开始🙂结束", 8)

        assertTrue(truncated.toByteArray(Charsets.UTF_8).size <= 8)
        assertFalse(truncated.lastOrNull()?.let(Character::isHighSurrogate) ?: false)
    }

    private fun record(
        id: String,
        taskId: String,
        scopeId: String,
        time: Long,
    ) = CronRunRecord(
        runId = id,
        taskId = taskId,
        scopeId = scopeId,
        projectId = null,
        projectName = scopeId,
        prompt = "test",
        scheduledAtMs = time,
        triggeredAtMs = time,
        status = CronRunStatus.Succeeded,
    )
}
