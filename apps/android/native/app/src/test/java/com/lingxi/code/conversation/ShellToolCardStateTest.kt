package com.lingxi.code.conversation

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ShellToolCardStateTest {
    @Test
    fun startParsesCommandAndCwd() {
        val update = shellStarted(
            id = "task-1",
            inputJson = """{"command":"printf hello","cwd":"/workspace"}""",
        )

        assertEquals("task-1", update.taskId)
        assertEquals("printf hello", update.command)
        assertEquals("/workspace", update.cwd)
    }

    @Test
    fun finishKeepsStreamsSeparateAndReportsExitStatus() {
        val update = shellFinished(
            id = "task-1",
            resultJson = """
                {"exit_code":7,"stdout":"out\n","stderr":"err\n","timed_out":false}
            """.trimIndent(),
            isError = false,
        )

        assertEquals("out\n", update.stdout)
        assertEquals("err\n", update.stderr)
        assertEquals(7, update.exitCode)
        assertEquals(ShellToolStatus.Failed, update.status)
    }

    @Test
    fun timeoutWinsOverGenericFailure() {
        val update = shellFinished(
            id = "task-2",
            resultJson = """{"data":{"timed_out":true,"stderr":"late"}}""",
            isError = true,
        )

        assertEquals(ShellToolStatus.TimedOut, update.status)
        assertTrue(update.stderr.contains("late"))
    }

    @Test
    fun maskedPermissionDenialReportedByToolMapsToFailed() {
        val update = shellFinished(
            id = "task-denied",
            resultJson = """
                {"data":{"stdout":"Access denied","stderr":"","exit_code":0,"is_error":true}}
            """.trimIndent(),
            isError = false,
        )

        assertEquals(ShellToolStatus.Failed, update.status)
        assertEquals(0, update.exitCode)
    }
}
