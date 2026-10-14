package com.lingxi.code.conversation

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Constraints
import kotlin.math.floor

/** Keep the ghost on the draft's final visual line, even when the editor scrolls. */
internal fun fitSlashArgumentHint(
    draft: String,
    hint: String?,
    width: Int,
    style: TextStyle,
    measurer: TextMeasurer,
): String? {
    if (hint.isNullOrEmpty() || width <= 0) return null
    val layout = measurer.measure(draft, style, constraints = Constraints(maxWidth = width))
    val remaining = floor(width - layout.getCursorRect(draft.length).left).toInt()
    return clipSlashArgumentHint(hint, remaining) { suffix ->
        measurer.measure(suffix, style, softWrap = false).size.width
    }
}

internal fun clipSlashArgumentHint(hint: String, available: Int, widthOf: (String) -> Int): String? {
    if (available <= 0) return null
    if (widthOf(hint) <= available) return hint
    val ellipsis = "…"
    if (widthOf(ellipsis) > available) return null
    var low = 0
    var high = hint.codePointCount(0, hint.length)
    while (low < high) {
        val middle = (low + high + 1) / 2
        val prefix = hint.substring(0, hint.offsetByCodePoints(0, middle))
        if (widthOf(prefix + ellipsis) <= available) low = middle else high = middle - 1
    }
    return hint.substring(0, hint.offsetByCodePoints(0, low)) + ellipsis
}

/** The suffix participates in text layout, but never enters the editable draft. */
internal class SlashArgumentHintTransformation(
    private val hint: String?,
    private val color: Color,
) : VisualTransformation {
    override fun filter(text: AnnotatedString): TransformedText {
        if (hint.isNullOrEmpty()) return TransformedText(text, OffsetMapping.Identity)
        val rendered = buildAnnotatedString {
            append(text)
            withStyle(SpanStyle(color = color)) { append(hint) }
        }
        val offsets = object : OffsetMapping {
            override fun originalToTransformed(offset: Int): Int = offset.coerceIn(0, text.length)
            override fun transformedToOriginal(offset: Int): Int = offset.coerceIn(0, text.length)
        }
        return TransformedText(rendered, offsets)
    }
}
