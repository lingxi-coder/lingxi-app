package com.lingxi.code.components

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.theme.LingXiTheme

/**
 * Pill (chip) — ported from the iOS `Pill`. A small rounded label used for
 * counts, tags and status chips. When [color] is given it tints itself; with no
 * color it falls back to the neutral surface + border + text3 styling.
 */
@Composable
fun Pill(
    text: String,
    modifier: Modifier = Modifier,
    color: Color? = null,
) {
    val t = LingXiTheme.palette
    val fg = color ?: t.text3
    val shape = RoundedCornerShape(6.dp)
    val bg = if (color != null) color.tint(0.16f) else t.surface
    val borderColor = if (color != null) color.tint(0.30f) else t.border

    Text(
        text = text,
        color = fg,
        fontSize = 11.5f.sp,
        fontWeight = FontWeight.Medium,
        modifier = modifier
            .background(bg, shape)
            .border(0.5.dp, borderColor, shape)
            .padding(horizontal = 9.dp, vertical = 3.dp),
    )
}
