package com.lingxi.code.localapps

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalAppsScreenTest {

    // The create form is gone: 「+」 creates an empty shell outright and the
    // conversation settles the rest, so there is no brief, no name field, no
    // surface picker and no workflow-model row left to test. What the screen
    // still owns pure-function-wise is the draft/formed label split and the
    // session catalog's display order.

    /**
     * A DRAFT card must show neither the stored name nor the stored brief.
     *
     * The fixture below is exactly what the engine writes for a shell: the
     * non-localized `"untitled"` placeholder and an empty brief. Asserting
     * "the title is the draft copy" alone would still pass if the subtitle
     * leaked, so both are pinned, and the placeholder is pinned by ABSENCE.
     */
    @Test
    fun `a draft card renders the placeholder copy and leaks neither name nor brief`() {
        val shell = app(scaffolded = false, name = "untitled", brief = "")

        val text = localAppCardText(shell, draftTitle = "新应用", draftSubtitle = "创建中")

        assertEquals("新应用", text.title)
        assertEquals("创建中", text.subtitle)
        assertTrue("the placeholder name must not reach the card", "untitled" !in text.title)
        assertEquals(
            "新应用",
            localAppDisplayName(shell, draftTitle = "新应用", fallback = "回退"),
        )
    }

    /** A formed app is unaffected: it still renders its own identity. */
    @Test
    fun `a formed card renders the app's own name and brief`() {
        val formed = app(scaffolded = true, name = "喝水记录", brief = "记录每天喝水量")

        val text = localAppCardText(formed, draftTitle = "新应用", draftSubtitle = "创建中")

        assertEquals("喝水记录", text.title)
        assertEquals("记录每天喝水量", text.subtitle)
        assertEquals(
            "喝水记录",
            localAppDisplayName(formed, draftTitle = "新应用", fallback = "回退"),
        )
    }

    /**
     * "No app" is NOT "a draft". A top bar with no record yet falls back to its
     * own screen title, never to the draft copy.
     */
    @Test
    fun `an absent app falls back rather than borrowing the draft copy`() {
        assertEquals(
            "回退",
            localAppDisplayName(null, draftTitle = "新应用", fallback = "回退"),
        )
    }

    /** The widget snapshot drops shells outright — it does not relabel them. */
    @Test
    fun `the widget snapshot keeps only scaffolded apps`() {
        val apps = listOf(
            app(id = "formed", scaffolded = true),
            app(id = "shell", scaffolded = false),
        )

        assertEquals(listOf("formed"), appsForWidgetSnapshot(apps).map { it.id })
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

    private fun app(
        id: String = "a",
        name: String = "客户跟进",
        brief: String = "记录客户跟进情况",
        scaffolded: Boolean,
    ) = LocalAppItem(
        id = id,
        name = name,
        brief = brief,
        workflow = LocalAppWorkflow.Draft,
        updatedAtMs = 1,
        scaffolded = scaffolded,
    )

    private fun row(uuid: String, isInit: Boolean = false) = LocalAppSessionRow(
        uuid = uuid,
        title = "会话 $uuid",
        relativeTime = "刚刚",
        messageCount = 2,
        isInit = isInit,
    )
}
