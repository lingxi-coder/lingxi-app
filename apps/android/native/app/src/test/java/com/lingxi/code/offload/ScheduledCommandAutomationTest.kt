package com.lingxi.code.offload

import com.lingxi.code.bindings.runtime.CronTaskDto
import com.lingxi.code.cron.CronAutomation
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ScheduledCommandAutomationTest {
    @Test
    fun requiresModeAndRetainsItsScheduledOwner() {
        assertNull(scheduledCommandAutomation(null, "provider/model", "global"))
        assertNull(scheduledCommandAutomation("unknown", "provider/model", "global"))
        assertNull(scheduledCommandAutomation("chat", null, "global"))
        assertNull(scheduledCommandAutomation("chat", "  ", "global"))
        val global = requireNotNull(scheduledCommandAutomation("chat", " provider/model ", "global"))
        assertEquals("chat", global.sessionMode)
        assertEquals("scheduled", global.sessionScopeKey)
        assertEquals("provider/model", global.model)
        val project = requireNotNull(scheduledCommandAutomation("code", "provider/other", "project-a"))
        assertEquals("code", project.sessionMode)
        assertEquals("project.project-a", project.sessionScopeKey)
        assertEquals("provider/other", project.model)
    }

    @Test
    fun commandPassesExplicitModeModelAndOwnerToCreateAndRejectsMissingValues() = runTest {
        val calls = mutableListOf<Pair<String, CronAutomation>>()
        val create: suspend (String, String, String, Boolean, CronAutomation) -> CronTaskDto =
            { scope, cron, prompt, recurring, automation ->
                calls += scope to automation
                CronTaskDto(automation.json, "task", cron, prompt, 0uL, null, recurring, null, "", true, null)
            }
        fun arguments(vararg extra: String) = listOf("--cron", "0 9 * * *", "--prompt", "hello") + extra

        assertEquals(NativeOffloadResult.EXIT_SUCCESS,
            executeScheduledCreate(arguments("--mode", "chat", "--model", "provider/chat"), create).exitCode)
        assertEquals(NativeOffloadResult.EXIT_SUCCESS,
            executeScheduledCreate(arguments("--mode", "code", "--model", "provider/code", "--scope", "project-a"), create).exitCode)
        assertEquals(listOf("global", "project-a"), calls.map { it.first })
        assertEquals(listOf("chat", "code"), calls.map { it.second.sessionMode })
        assertEquals(listOf("provider/chat", "provider/code"), calls.map { it.second.model })
        assertEquals(listOf("scheduled", "project.project-a"), calls.map { it.second.sessionScopeKey })

        for (extra in listOf(
            listOf("--model", "provider/chat"),
            listOf("--mode", "", "--model", "provider/chat"),
            listOf("--mode", "chat"),
            listOf("--mode", "chat", "--model", "  "),
        )) {
            assertEquals(NativeOffloadResult.EXIT_USAGE, executeScheduledCreate(arguments(*extra.toTypedArray()), create).exitCode)
        }
        assertEquals(2, calls.size)
    }
}
