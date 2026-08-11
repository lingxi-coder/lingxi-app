package com.lingxi.code.conversation

import com.lingxi.code.bindings.CodeSegmentDto
import com.lingxi.code.bindings.DiffLineKindDto
import com.lingxi.code.bindings.DiffRowDto
import com.lingxi.code.bindings.HeadlineKindDto
import com.lingxi.code.bindings.PlanTaskDto
import com.lingxi.code.bindings.PlanTaskStateDto
import com.lingxi.code.bindings.StructuredDiffDto
import com.lingxi.code.bindings.SyntaxClassDto
import com.lingxi.code.bindings.ToolHeaderDto
import com.lingxi.code.bindings.ToolResultDisplayDto
import com.lingxi.code.bindings.ToolSubLineDto
import com.lingxi.code.bindings.ToolVerbDto
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The PURE tool-display mappers: wire DTO → render model, plus the two pieces of
 * derived layout the renderers depend on (the plan window and the diff gutter).
 *
 * These only CONSTRUCT generated UniFFI data classes — never call an exported
 * function — so no native `.so` is loaded and the whole file runs on the plain
 * JVM next to [ClientEventMapperTest].
 */
class ToolCallStateTest {

    // --- header -----------------------------------------------------------

    @Test
    fun toolHeaderDto_lowersEveryField_includingTheCountedShellVerb() {
        val header = ToolHeaderDto(
            verb = ToolVerbDto.SHELL,
            label = "Running shell command",
            primary = null,
            qualifier = null,
            count = 3u,
            subLine = ToolSubLineDto(prefix = "$", text = "cargo test --all"),
            title = "Running shell command",
        ).toUi()

        assertEquals(ToolVerbUi.Shell, header.verb)
        assertEquals("Running shell command", header.label)
        assertEquals(3, header.count)
        assertEquals("$", header.subLine?.prefix)
        assertEquals("cargo test --all", header.subLine?.text)
    }

    @Test
    fun everyWireVerb_hasADistinctRenderVerb() {
        val mapped = ToolVerbDto.values().map { it.toUi() }
        assertEquals(ToolVerbDto.values().size, mapped.toSet().size)
        assertEquals(ToolVerbUi.Generic, ToolVerbDto.GENERIC.toUi())
        assertEquals(ToolVerbUi.Update, ToolVerbDto.UPDATE.toUi())
    }

    @Test
    fun composeToolTitle_matchesTheEngineShape() {
        assertEquals(
            "修改(src/host.rs) (3 edits)",
            composeToolTitle("修改", "src/host.rs", " (3 edits)"),
        )
        // No primary ⇒ no empty parentheses.
        assertEquals("终止", composeToolTitle("终止", null, null))
        assertEquals("搜索(TODO) in src", composeToolTitle("搜索", "TODO", " in src"))
    }

    // --- diff -------------------------------------------------------------

    @Test
    fun segmentsAreConcatenatedNotIndexed_soMultibyteTextSurvives() {
        // Rust splits by UTF-8 byte and Kotlin by UTF-16 code unit; the wire
        // carries pre-split runs precisely so neither side has to agree on an
        // index. Rejoining them must reproduce the line EXACTLY — including a
        // surrogate pair and CJK, where any offset arithmetic would mis-slice.
        val row = DiffRowDto(
            kind = DiffLineKindDto.ADD,
            lineNo = 42u,
            hunk = 0u,
            wordDiffed = true,
            segments = listOf(
                segment("let 名前 = ", SyntaxClassDto.PLAIN),
                segment("\"🚀🚀\"", SyntaxClassDto.STRING_LIT, emph = true),
                segment("; // 完了", SyntaxClassDto.COMMENT),
            ),
        ).toUi()

        assertEquals("let 名前 = \"🚀🚀\"; // 完了", row.text)
        assertEquals(3, row.segments.size)
        assertEquals(SyntaxClassUi.StringLit, row.segments[1].syntax)
        assertTrue(row.segments[1].emph)
        assertFalse(row.segments[0].emph)
        assertEquals(42, row.lineNo)
    }

    @Test
    fun everyWireSyntaxClass_hasADistinctRenderClass() {
        val mapped = SyntaxClassDto.values().map { it.toUi() }
        assertEquals(SyntaxClassDto.values().size, mapped.toSet().size)
        assertEquals(SyntaxClassUi.TypeName, SyntaxClassDto.TYPE_NAME.toUi())
    }

    @Test
    fun codeSegmentRgb_lowersToPackedInt_andSurvivesTheHighBit() {
        // 0x00RRGGBB fits an Int, but a UInt near the top of the range must not
        // wrap into a negative surprise for the palette's fallback path.
        val segment = segment("x", SyntaxClassDto.PLAIN).copy(rgb = 0xFFFFFFu).toUi()
        assertEquals(0xFFFFFF, segment.rgb)
        assertNull(segment("y", SyntaxClassDto.PLAIN).toUi().rgb)
    }

    @Test
    fun structuredDiffDto_lowersCountsAndRows() {
        val diff = StructuredDiffDto(
            filePath = "src/host.rs",
            language = "rust",
            gutterWidth = 4u,
            additions = 18u,
            removals = 4u,
            truncatedRows = 7u,
            rows = listOf(
                DiffRowDto(DiffLineKindDto.CONTEXT, 1u, 0u, false, listOf(segment("a", SyntaxClassDto.PLAIN))),
                DiffRowDto(DiffLineKindDto.REMOVE, 2u, 1u, false, listOf(segment("b", SyntaxClassDto.PLAIN))),
            ),
        ).toUi()

        assertEquals("src/host.rs", diff.filePath)
        assertEquals(4, diff.gutterWidth)
        assertEquals(18, diff.additions)
        assertEquals(4, diff.removals)
        assertEquals(7, diff.truncatedRows)
        assertEquals(listOf(DiffLineKindUi.Context, DiffLineKindUi.Remove), diff.rows.map { it.kind })
        // The hunk index CHANGES between these two rows — that gap is where the
        // renderer draws `⋯`. There is no separator row on the wire.
        assertEquals(listOf(0, 1), diff.rows.map { it.hunk })
    }

    @Test
    fun gutterLabel_rightAlignsTheLineNumberAndCarriesTheKindSign() {
        fun row(kind: DiffLineKindUi, lineNo: Int) =
            DiffRowUi(kind = kind, lineNo = lineNo, hunk = 0, wordDiffed = false, segments = emptyList())

        assertEquals("   7+ ", gutterLabel(row(DiffLineKindUi.Add, 7), gutterWidth = 4))
        assertEquals("1024- ", gutterLabel(row(DiffLineKindUi.Remove, 1024), gutterWidth = 4))
        assertEquals("   9  ", gutterLabel(row(DiffLineKindUi.Context, 9), gutterWidth = 4))
    }

    // --- result -----------------------------------------------------------

    @Test
    fun toolResultDisplayDto_lowersHeadlineArgsAndCollapseVerdict() {
        val display = ToolResultDisplayDto(
            headline = "Added 18 lines, removed 4 lines",
            headlineKind = HeadlineKindDto.ADDED_REMOVED,
            headlineArgs = listOf(18u, 4u),
            diff = null,
            body = "one\ntwo",
            bodyLines = 240u,
            bodyTruncated = true,
            collapsed = true,
        ).toUi()

        assertEquals(HeadlineKindUi.AddedRemoved, display.headlineKind)
        assertEquals(listOf(18, 4), display.headlineArgs)
        assertEquals(240, display.bodyLines) // count BEFORE clamping
        assertTrue(display.bodyTruncated)
        assertTrue(display.collapsed)
        assertTrue(display.hasExpandableContent)
    }

    @Test
    fun everyWireHeadlineKind_hasADistinctRenderKind() {
        val mapped = HeadlineKindDto.values().map { it.toUi() }
        assertEquals(HeadlineKindDto.values().size, mapped.toSet().size)
        assertEquals(HeadlineKindUi.LinesReadPartial, HeadlineKindDto.LINES_READ_PARTIAL.toUi())
    }

    @Test
    fun displayWithNeitherBodyNorDiff_hasNothingToExpand() {
        val display = ToolResultDisplayDto(
            headline = "(No content)",
            headlineKind = HeadlineKindDto.NO_CONTENT,
            headlineArgs = emptyList(),
            diff = null,
            body = null,
            bodyLines = 0u,
            bodyTruncated = false,
            collapsed = false,
        ).toUi()
        assertFalse(display.hasExpandableContent)
    }

    // --- plan -------------------------------------------------------------

    @Test
    fun planTaskDto_lowersIdSubjectActiveFormAndState() {
        val task = PlanTaskDto(
            id = "task-1",
            subject = "Ship it",
            activeForm = "Shipping it",
            state = PlanTaskStateDto.IN_PROGRESS,
        ).toUi()

        assertEquals("task-1", task.id)
        assertEquals("Shipping it", task.activeForm)
        assertEquals(PlanTaskStateUi.InProgress, task.state)
        assertEquals("◼", task.state.glyph)
        assertEquals("◻", PlanTaskStateUi.Pending.glyph)
        assertEquals("✔", PlanTaskStateUi.Completed.glyph)
    }

    @Test
    fun planWindow_capsAtFiveAndCountsTheHiddenRemainderByState() {
        val tasks = listOf(
            task("a", PlanTaskStateUi.Completed),
            task("b", PlanTaskStateUi.Completed),
            task("c", PlanTaskStateUi.InProgress),
            task("d", PlanTaskStateUi.Pending),
            task("e", PlanTaskStateUi.Pending),
            // hidden from here on
            task("f", PlanTaskStateUi.Pending),
            task("g", PlanTaskStateUi.Pending),
            task("h", PlanTaskStateUi.InProgress),
            task("i", PlanTaskStateUi.Completed),
        )
        val window = planWindow(tasks)

        assertEquals(MAX_VISIBLE_PLAN_TASKS, window.visible.size)
        assertEquals(listOf("a", "b", "c", "d", "e"), window.visible.map { it.subject })
        assertEquals(1, window.hiddenInProgress)
        assertEquals(2, window.hiddenPending)
        assertEquals(1, window.hiddenCompleted)
        assertEquals(4, window.hiddenCount)
        assertTrue(window.hasOverflow)
    }

    @Test
    fun planWindow_shorterThanTheCap_hidesNothing() {
        val window = planWindow(listOf(task("only", PlanTaskStateUi.Pending)))
        assertEquals(1, window.visible.size)
        assertFalse(window.hasOverflow)
        assertEquals(0, window.hiddenCount)
    }

    @Test
    fun planWindow_expandedToFullSize_hidesNothing() {
        val tasks = (1..9).map { task("t$it", PlanTaskStateUi.Pending) }
        val window = planWindow(tasks, max = tasks.size)
        assertEquals(9, window.visible.size)
        assertFalse(window.hasOverflow)
    }

    // --- fixtures ---------------------------------------------------------

    private fun segment(
        text: String,
        syntax: SyntaxClassDto,
        emph: Boolean = false,
    ) = CodeSegmentDto(
        text = text,
        `class` = syntax,
        rgb = null,
        bold = false,
        italic = false,
        underline = false,
        emph = emph,
    )

    private fun task(subject: String, state: PlanTaskStateUi) =
        PlanTaskUi(id = null, subject = subject, activeForm = null, state = state)
}
