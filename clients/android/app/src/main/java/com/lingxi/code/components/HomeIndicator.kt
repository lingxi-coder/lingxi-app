package com.lingxi.code.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import com.lingxi.code.theme.LingXiTheme

/**
 * HomeIndicator — the brand "grab handle" pill, ported from the iOS
 * `HomeIndicator`. On Android the OS owns the real gesture-nav bar, so this is
 * used purely as a decorative handle on full-screen brand overlays (e.g. the
 * voice flow and bottom sheets) to echo the prototype's look. Color tracks the
 * appearance (white on dark, black on light) matching the iOS version.
 */
@Composable
fun HomeIndicator(modifier: Modifier = Modifier) {
    val t = LingXiTheme.palette
    val color = if (t.isDark) Color.White else Color.Black
    Box(
        modifier = modifier
            .fillMaxWidth()
            .height(28.dp),
        contentAlignment = Alignment.BottomCenter,
    ) {
        Box(
            modifier = Modifier
                .padding(bottom = 8.dp)
                .size(width = 134.dp, height = 5.dp)
                .background(color, RoundedCornerShape(99.dp)),
        )
    }
}
