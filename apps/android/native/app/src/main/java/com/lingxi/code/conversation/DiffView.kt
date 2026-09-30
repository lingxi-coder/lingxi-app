package com.lingxi.code.conversation

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.R
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.theme.Palette
import com.lingxi.code.theme.segmentColor

/**
 * The structured-diff renderer for a tool result's `⎿` block.
 *
 * Everything it draws is pre-derived by the engine: rows, hunks, the gutter
 * width, and — critically — the PRE-SPLIT [CodeSegmentUi] runs of each line.
 *
 * ### The three contracts this file exists to honor
 *
 *  1. **Never index a row's string.** Rust splits by UTF-8 byte, Kotlin by
 *     UTF-16 code unit; a `withStyle(start, end)` over a reconstructed line
 *     would mis-slice the moment a row contains an emoji or a CJK glyph. The
 *     attributed string is built by APPENDING segments in order, so it is
 *     byte-for-byte the row and every style lands on exactly its own run.
 *  2. **Color comes from `segment.syntax`, never `segment.rgb`.** See
 *     [com.lingxi.code.theme.segmentColor] — `rgb` is a dark-terminal bake and
 *     is consulted only as the dark-mode fallback for an unclassified run.
 *  3. **Backgrounds are derived here, not received.** The wire carries none, on
 *     purpose: the terminal's are alpha-over-black blends. Row washes come from
 *     [DiffRowUi.kind] and the intra-line emphasis from [CodeSegmentUi.emph].
 *
 * ### Scrolling
 *
 * ONE [horizontalScroll] wraps the whole row stack so every row shares a single
 * offset — a per-row scroll would let lines drift out of alignment, which is
 * exactly what makes a diff unreadable. There is deliberately NO `verticalScroll`
 * here: this composable is realized inside the transcript `LazyColumn`, and a
 * nested vertical scroller there throws `IllegalStateException: Vertically
 * scrollable component was measured with an infinity maximum height constraints`
 * at runtime, only once the row is realized.
 */
@Composable
internal fun DiffView(
    diff: StructuredDiffUi,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    val scroll = rememberScrollState()
    // The gutter is padded to the width the engine computed across ALL hunks, so
    // line numbers stay right-aligned without measuring anything here.
    val gutterWidth = diff.gutterWidth.coerceIn(1, MAX_GUTTER_WIDTH)

    Column(
        modifier = modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(t.diffSurface),
    ) {
        if (diff.filePath != null || diff.additions > 0 || diff.removals > 0) {
            DiffCaption(diff)
        }
        Column(modifier = Modifier.horizontalScroll(scroll)) {
            // Intrinsic max width makes every row as wide as the widest one, so
            // the add/remove wash spans the full line instead of stopping at the
            // end of each row's own text. `fillMaxWidth` alone cannot do this:
            // inside a horizontal scroller the incoming maxWidth is infinite.
            Column(modifier = Modifier.width(IntrinsicSize.Max)) {
                var previousHunk: Int? = null
                diff.rows.forEach { row ->
                    // `hunk` is a 0-based index and there is no separator ROW on
                    // the wire — a CHANGE between consecutive rows is where the
                    // dim `⋯` belongs.
                    if (previousHunk != null && row.hunk != previousHunk) {
                        HunkSeparator(gutterWidth = gutterWidth)
                    }
                    previousHunk = row.hunk
                    DiffRowView(row = row, gutterWidth = gutterWidth, palette = t)
                }
            }
        }
        if (diff.truncatedRows > 0) {
            Text(
                text = stringResource(R.string.chat_diff_more_rows_label, diff.truncatedRows),
                color = t.text4,
                fontSize = 11.5f.sp,
                fontFamily = FontFamily.Monospace,
                modifier = Modifier.padding(horizontal = 10.dp, vertical = 5.dp),
            )
        }
    }
}

/** `src/host.rs · +18 −4` — the diff's provenance line. */
@Composable
private fun DiffCaption(diff: StructuredDiffUi) {
    val t = LingXiTheme.palette
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 10.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        diff.filePath?.let {
            Text(
                text = it,
                color = t.text3,
                fontSize = 11.5f.sp,
                fontFamily = FontFamily.Monospace,
                modifier = Modifier.weight(1f, fill = false),
            )
        }
        if (diff.additions > 0) {
            Text(text = "+${diff.additions}", color = t.ok, fontSize = 11.5f.sp, fontFamily = FontFamily.Monospace)
        }
        if (diff.removals > 0) {
            Text(text = "−${diff.removals}", color = t.danger, fontSize = 11.5f.sp, fontFamily = FontFamily.Monospace)
        }
    }
}

/** The dim `⋯` drawn between two rows whose `hunk` index differs. */
@Composable
private fun HunkSeparator(gutterWidth: Int) {
    val t = LingXiTheme.palette
    Text(
        text = hunkSeparatorLabel(gutterWidth),
        color = t.diffRule,
        fontSize = DIFF_FONT_SIZE.sp,
        lineHeight = DIFF_LINE_HEIGHT.sp,
        fontFamily = FontFamily.Monospace,
        modifier = Modifier.padding(horizontal = 10.dp),
    )
}

@Composable
private fun DiffRowView(row: DiffRowUi, gutterWidth: Int, palette: Palette) {
    val background = when (row.kind) {
        DiffLineKindUi.Add -> palette.diffAddBg
        DiffLineKindUi.Remove -> palette.diffRemoveBg
        DiffLineKindUi.Context -> Color.Transparent
    }
    val emphBackground = when (row.kind) {
        DiffLineKindUi.Add -> palette.diffAddEmphBg
        DiffLineKindUi.Remove -> palette.diffRemoveEmphBg
        DiffLineKindUi.Context -> Color.Transparent
    }
    val content = remember(row, palette) {
        annotateDiffRow(row = row, palette = palette, emphBackground = emphBackground)
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(background)
            .padding(horizontal = 10.dp),
    ) {
        Text(
            text = gutterLabel(row, gutterWidth),
            color = palette.diffGutter,
            fontSize = DIFF_FONT_SIZE.sp,
            lineHeight = DIFF_LINE_HEIGHT.sp,
            fontFamily = FontFamily.Monospace,
            softWrap = false,
        )
        Text(
            text = content,
            fontSize = DIFF_FONT_SIZE.sp,
            lineHeight = DIFF_LINE_HEIGHT.sp,
            fontFamily = FontFamily.Monospace,
            softWrap = false,
        )
    }
}

/** `"  42 +"` — the right-aligned line number plus the kind sign. */
internal fun gutterLabel(row: DiffRowUi, gutterWidth: Int): String {
    val sign = when (row.kind) {
        DiffLineKindUi.Add -> '+'
        DiffLineKindUi.Remove -> '-'
        DiffLineKindUi.Context -> ' '
    }
    return row.lineNo.toString().padStart(gutterWidth) + sign + " "
}

/**
 * `"     ⋯"` — the hunk separator, indented to the row TEXT column.
 *
 * The `⋯` must line up with where a row's content starts, so the pad has to be
 * exactly [gutterLabel]'s width: `gutterWidth` digits + the kind sign + the
 * separating space. Padding `gutterWidth + 1` put the glyph one column early,
 * under the +/− sign. Derived from [gutterLabel] rather than restated, so the
 * two can no longer drift. PURE, for JVM tests.
 */
internal fun hunkSeparatorLabel(gutterWidth: Int): String =
    " ".repeat(gutterLabelWidth(gutterWidth)) + "⋯"

/** The exact rendered width of every [gutterLabel] at this gutter width. */
internal fun gutterLabelWidth(gutterWidth: Int): Int = gutterWidth + 2

/**
 * Build one row's attributed text by APPENDING its pre-split segments in order.
 *
 * This is the whole point of the wire format: `append` + `withStyle` per segment
 * means no offset arithmetic ever happens on this side, so a row containing CJK,
 * emoji, or combining marks styles correctly without the client and the engine
 * having to agree on what an "index" is. Concatenating the appended runs
 * reproduces [DiffRowUi.text] exactly.
 *
 * PURE (no `@Composable`) so the styling decisions are unit-testable.
 */
internal fun annotateDiffRow(
    row: DiffRowUi,
    palette: Palette,
    emphBackground: Color,
): AnnotatedString = buildAnnotatedString {
    row.segments.forEach { segment ->
        withStyle(
            SpanStyle(
                color = palette.segmentColor(segment.syntax, segment.rgb),
                fontWeight = if (segment.bold) FontWeight.Bold else null,
                fontStyle = if (segment.italic) FontStyle.Italic else null,
                textDecoration = if (segment.underline) TextDecoration.Underline else null,
                // Word-diff emphasis is a CLIENT derivation: the engine ships the
                // `emph` flag, never a background, because the terminal's blend is
                // only valid over a black ground.
                background = if (segment.emph) emphBackground else Color.Unspecified,
            ),
        ) {
            append(segment.text)
        }
    }
}

private const val DIFF_FONT_SIZE = 12f
private const val DIFF_LINE_HEIGHT = 17f

/** Defensive clamp: a corrupt `gutter_width` must not blow up the row layout. */
private const val MAX_GUTTER_WIDTH = 12
