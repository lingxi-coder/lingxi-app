package com.lingxi.code.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.foundation.isSystemInDarkTheme

/**
 * App theme wrapper.
 *
 * A1 scaffold: intentionally minimal — wraps Material 3's default color schemes
 * so the empty app compiles and renders. A2 replaces these with the brand
 * [DesignTokens] (the exact oklch→sRGB palette ported from iOS), the custom
 * typography/shapes, and a live theme + accent switch wired through a
 * CompositionLocal + DataStore.
 */
@Composable
fun LingXiTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    val colorScheme = if (darkTheme) darkColorScheme() else lightColorScheme()
    MaterialTheme(
        colorScheme = colorScheme,
        content = content,
    )
}
