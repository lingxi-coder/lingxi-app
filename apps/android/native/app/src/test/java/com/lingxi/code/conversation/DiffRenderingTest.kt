package com.lingxi.code.conversation

import androidx.compose.ui.graphics.Color
import com.lingxi.code.theme.DesignTokens
import com.lingxi.code.theme.SyntaxPalette
import com.lingxi.code.theme.segmentColor
import com.lingxi.code.theme.syntax
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The two WIRE CONTRACTS the structured-diff renderer exists to enforce, plus
 * the gutter alignment that depends on them. All three are decided by PURE
 * functions ([annotateDiffRow], [com.lingxi.code.theme.segmentColor],
 * [gutterLabel] / [hunkSeparatorLabel]) precisely so they can be pinned here
 * instead of only on a device.
 *
 * Both contracts were previously untested: mutating
 * `segmentColor`'s appearance guard to `if (rgb != null)`, or
 * `annotateDiffRow`'s `row.segments.forEach` to `row.segments.reversed()
 * .forEach`, left the whole suite green while every diff line rendered with
 * terminal-baked colors in light mode / with its runs scrambled.
 *
 * Compose `Color`, `SpanStyle` and `AnnotatedString` are framework-free value
 * types, so this runs on the plain JVM next to [ToolCallStateTest].
 */
class DiffRenderingTest {

    private val dark = DesignTokens.dark
    private val light = DesignTokens.light

    /** `0x00RRGGBB` as the wire carries it — a terminal-baked bright red. */
    private val wireRgb = 0x00FF0000

    private fun segment(
        text: String,
        syntax: SyntaxClassUi = SyntaxClassUi.Plain,
        rgb: Int? = null,
        bold: Boolean = false,
        emph: Boolean = false,
    ) = CodeSegmentUi(text = text, syntax = syntax, rgb = rgb, bold = bold, emph = emph)

    private fun row(
        vararg segments: CodeSegmentUi,
        kind: DiffLineKindUi = DiffLineKindUi.Add,
        lineNo: Int = 42,
    ) = DiffRowUi(kind = kind, lineNo = lineNo, hunk = 0, wordDiffed = false, segments = segments.toList())

    // --- contract 1: segments are APPENDED in wire order ------------------

    @Test
    fun annotateDiffRow_appendsEverySegmentInWireOrder_reproducingTheRowExactly() {
        // Deliberately multi-byte: a reversed / re-sliced build would corrupt
        // these first. `text` is the engine's own concatenation of the runs.
        val r = row(
            segment("let ", SyntaxClassUi.Keyword),
            segment("名字", SyntaxClassUi.Variable),
            segment(" = ", SyntaxClassUi.Operator),
            segment("\"🎉\"", SyntaxClassUi.StringLit),
        )

        val built = annotateDiffRow(row = r, palette = dark, emphBackground = Color.Transparent)

        assertEquals("let 名字 = \"🎉\"", r.text)
        assertEquals("the attributed string IS the row", r.text, built.text)
    }

    @Test
    fun annotateDiffRow_stylesLandOnTheirOwnRun_notAShiftedOne() {
        val segments = listOf(
            segment("fn ", SyntaxClassUi.Keyword),
            segment("名字", SyntaxClassUi.Function),
            segment("()", SyntaxClassUi.Punctuation),
        )
        val r = row(*segments.toTypedArray())

        val built = annotateDiffRow(row = r, palette = dark, emphBackground = Color.Transparent)

        assertEquals(segments.size, built.spanStyles.size)
        built.spanStyles.forEachIndexed { index, span ->
            val expected = segments[index]
            assertEquals(
                "span $index must cover exactly its own run",
                expected.text,
                built.text.substring(span.start, span.end),
            )
            assertEquals(
                "span $index must carry its own run's color",
                dark.syntax.color(expected.syntax),
                span.item.color,
            )
        }
    }

    @Test
    fun annotateDiffRow_emphBackgroundRidesOnlyTheChangedRun() {
        val emphBg = Color(0x5C2EA043)
        val r = row(
            segment("keep ", SyntaxClassUi.Plain),
            segment("changed", SyntaxClassUi.Plain, emph = true),
        )

        val built = annotateDiffRow(row = r, palette = dark, emphBackground = emphBg)

        assertEquals(Color.Unspecified, built.spanStyles[0].item.background)
        assertEquals(emphBg, built.spanStyles[1].item.background)
    }

    @Test
    fun annotateDiffRow_takesItsColorFromTheClass_evenWhenTheWireCarriedRgb() {
        // The call site must go through `segmentColor`, so a classified run's
        // terminal `rgb` never reaches the canvas in EITHER appearance.
        val r = row(segment("let", SyntaxClassUi.Keyword, rgb = wireRgb))

        listOf(dark, light).forEach { palette ->
            val built = annotateDiffRow(row = r, palette = palette, emphBackground = Color.Transparent)
            assertEquals(
                "isDark=${palette.isDark}: the class palette wins over the wire rgb",
                palette.syntax.keyword,
                built.spanStyles.single().item.color,
            )
        }
    }

    // --- contract 2: class-first color, `rgb` is the DARK-ONLY fallback ----

    @Test
    fun segmentColor_lightAppearance_neverPaintsTheDarkBakedRgb() {
        // The mutation this pins: dropping the `isDark && Plain` guard makes the
        // terminal's dark-baked foreground the primary color in BOTH themes.
        val plain = light.segmentColor(SyntaxClassUi.Plain, wireRgb)
        assertEquals("an unclassified run in light mode uses the light palette", SyntaxPalette.light.plain, plain)
        assertNotEquals("the dark-baked rgb must not reach the light canvas", Color(0xFFFF0000), plain)

        val keyword = light.segmentColor(SyntaxClassUi.Keyword, wireRgb)
        assertEquals(SyntaxPalette.light.keyword, keyword)
    }

    @Test
    fun segmentColor_darkAppearance_usesRgbOnlyForAnUnclassifiedRun() {
        // Plain + rgb is the ONE case the wire color wins — opaque, alpha forced.
        assertEquals(Color(0xFFFF0000), dark.segmentColor(SyntaxClassUi.Plain, wireRgb))
        // Every classified run stays on the class palette even in the dark.
        assertEquals(SyntaxPalette.dark.keyword, dark.segmentColor(SyntaxClassUi.Keyword, wireRgb))
        assertEquals(SyntaxPalette.dark.stringLit, dark.segmentColor(SyntaxClassUi.StringLit, wireRgb))
        // No rgb at all: back to the class palette.
        assertEquals(SyntaxPalette.dark.plain, dark.segmentColor(SyntaxClassUi.Plain, null))
    }

    @Test
    fun segmentColor_everyClass_resolvesToItsOwnAppearancesPalette() {
        SyntaxClassUi.entries.forEach { syntax ->
            assertEquals("dark/$syntax", SyntaxPalette.dark.color(syntax), dark.segmentColor(syntax, null))
            assertEquals("light/$syntax", SyntaxPalette.light.color(syntax), light.segmentColor(syntax, null))
        }
        // The two sets are genuinely different — otherwise the assertions above
        // would hold no matter which appearance the renderer picked.
        assertNotEquals(SyntaxPalette.dark.plain, SyntaxPalette.light.plain)
    }

    // --- the gutter the `⋯` has to line up with ---------------------------

    @Test
    fun hunkSeparator_indentsToTheTextColumn_notTheSignColumn() {
        // The engine sizes `gutterWidth` to the widest line number across ALL
        // hunks, so each pair below is a real (width, line number) combination.
        listOf(1 to 7, 3 to 42, 5 to 10234).forEach { (width, lineNo) ->
            val label = gutterLabel(row(segment("x"), kind = DiffLineKindUi.Context, lineNo = lineNo), width)
            val separator = hunkSeparatorLabel(width)
            assertEquals(
                "gutter width $width: the ⋯ must start where a row's text starts",
                label.length,
                separator.indexOf('⋯'),
            )
            assertTrue("the separator is padding plus the glyph", separator.endsWith("⋯"))
        }
    }

    @Test
    fun gutterLabel_isLineNumberThenSignThenSpace() {
        assertEquals(" 42+ ", gutterLabel(row(segment("x"), kind = DiffLineKindUi.Add), 3))
        assertEquals(" 42- ", gutterLabel(row(segment("x"), kind = DiffLineKindUi.Remove), 3))
        assertEquals(" 42  ", gutterLabel(row(segment("x"), kind = DiffLineKindUi.Context), 3))
        assertEquals(gutterLabelWidth(3), gutterLabel(row(segment("x")), 3).length)
    }
}
