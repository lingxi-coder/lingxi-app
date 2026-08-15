package com.lingxi.code.localapps

import androidx.compose.ui.graphics.Color
import com.lingxi.code.model.ModelOption
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalAppsScreenTest {

    // v3 (agent-driven local apps): the designer/plan/generation surfaces are
    // gone; what the screen still owns pure-function-wise is the create entry
    // and the session catalog's display order.

    @Test
    fun `the create screen asks only for a description`() {
        assertEquals(1, createScreenInputCount())
    }

    @Test
    fun `an empty description cannot be submitted`() {
        assertFalse(canSubmitBrief("   "))
        assertTrue(canSubmitBrief("一个记事本 app"))
    }

    @Test
    fun `workflow model row shows both model and provider without changing its wire id`() {
        val option = ModelOption(
            id = "deepseek/deepseek-v3.2",
            name = "DeepSeek V3.2",
            desc = "deepseek-v3.2",
            tag = "",
            color = Color.Blue,
            providerId = "deepseek",
            providerName = "DeepSeek",
        )

        assertEquals("DeepSeek V3.2 · DeepSeek", workflowModelOptionLabel(option))
        assertEquals("deepseek/deepseek-v3.2", option.id)
    }

    @Test
    fun `the init session is pinned first regardless of where the engine paged it`() {
        val rows = listOf(
            row("s-newest"),
            row("s-init", isInit = true),
            row("s-older"),
        )
        assertEquals(
            listOf("s-init", "s-newest", "s-older"),
            sessionRowsForDisplay(rows).map { it.uuid },
        )
    }

    @Test
    fun `non-init rows keep the engine's modified-descending order`() {
        val rows = listOf(row("s-3"), row("s-2"), row("s-1"))
        assertEquals(
            listOf("s-3", "s-2", "s-1"),
            sessionRowsForDisplay(rows).map { it.uuid },
        )
    }

    @Test
    fun `a catalog with no init row is displayed unchanged`() {
        val rows = listOf(row("s-2"), row("s-1"))
        assertEquals(rows, sessionRowsForDisplay(rows))
    }

    private fun row(uuid: String, isInit: Boolean = false) = LocalAppSessionRow(
        uuid = uuid,
        title = "会话 $uuid",
        relativeTime = "刚刚",
        messageCount = 2,
        isInit = isInit,
    )
}
