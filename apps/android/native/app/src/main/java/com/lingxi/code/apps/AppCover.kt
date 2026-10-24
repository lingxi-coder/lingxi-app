package com.lingxi.code.apps

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

private val CoverColors = mapOf(
    "tools" to Color(0xFF6E96FF),
    "productivity" to Color(0xFF00C2C3),
    "games" to Color(0xFFCE74E3),
    "education" to Color(0xFFDE9300),
    "visualization" to Color(0xFFE5687A),
)
private val OtherCoverColor = Color(0xFF8A8F98)

/** A cover in the market's single-character style, tinted by category. */
@Composable
internal fun AppCover(name: String, category: String, size: Dp, modifier: Modifier = Modifier) {
    val color = CoverColors[category] ?: OtherCoverColor
    val shape = RoundedCornerShape(size * 0.26f)
    Box(
        contentAlignment = Alignment.Center,
        modifier = modifier
            .size(size)
            .clip(shape)
            .background(color.copy(alpha = 0.16f))
            .border(0.5.dp, color.copy(alpha = 0.35f), shape),
    ) {
        Text(appInitial(name), color = color, fontSize = (size.value * 0.42f).sp, fontWeight = FontWeight.Bold)
    }
}
