package com.lingxi.code.apps

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalAppAuthoringPromptTest {
    @Test fun createAndModifyIncludeSafeAgentRecoveryGuidance() {
        val create = LocalAppAuthoringPrompt.compose("Reader", null, "")
        val modify = LocalAppAuthoringPrompt.compose("Reader", "app-1", "")
        assertTrue(create.contains("创建应用：Reader"))
        assertTrue(modify.contains("LocalApp id: app-1"))
        for (prompt in listOf(create, modify)) {
            assertTrue(prompt.contains("runId、threadId"))
            assertTrue(prompt.contains("只读方式重连同一 run 和 thread"))
            assertTrue(prompt.contains("不要自动提交、重发或重放工具"))
            assertTrue(prompt.contains("仅在宿主确认时显示正在运行"))
            assertTrue(prompt.contains("中断或未知状态"))
            assertTrue(prompt.contains("用户明确发起并使用新的 runId"))
        }
    }

    @Test fun modifyPreservesAnUnsentDraftAndDoesNotDuplicateTheGuidance() {
        val unsent = "请保留这一段。\n第二行是尚未发送的需求。"
        val first = LocalAppAuthoringPrompt.compose("Reader", "app-1", unsent)
        assertTrue(first.endsWith(unsent))
        assertEquals(first, LocalAppAuthoringPrompt.compose("Reader", "app-1", first))
    }
}
