package com.lingxi.code.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

/** Shared Material palette used by native and custom conversation surfaces. */
val LocalPalette = staticCompositionLocalOf { DesignTokens.dark }

/** Convenience accessor mirroring the iOS `@Environment(\.theme) var t`. */
object LingXiTheme {
    val palette: Palette
        @Composable get() = LocalPalette.current
}

/**
 * Resolve the [Palette] for an appearance + accent. Pure — usable from previews
 * and tests without a DataStore.
 */
fun resolvePalette(dark: Boolean, accentId: String): Palette =
    DesignTokens.palette(dark).withAccent(Accents.color(forId = accentId, dark = dark))

/**
 * Theme wrapper.
 *
 * @param darkTheme whether to use the dark palette (caller resolves
 *   [ThemeMode.System] against [isSystemInDarkTheme] before passing in).
 * @param accentId the selected accent's oklch id (see [Accents]).
 */
@Composable
fun LingXiTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    accentId: String = Accents.DEFAULT_ID,
    content: @Composable () -> Unit,
) {
    val palette = resolvePalette(dark = darkTheme, accentId = accentId)
    val colorScheme = palette.toColorScheme()

    CompositionLocalProvider(LocalPalette provides palette,
        androidx.compose.material3.LocalContentColor provides palette.text) {
        MaterialTheme(
            colorScheme = colorScheme,
            typography = LXTypography,
            shapes = LXShapes,
            content = content,
        )
    }
}

/** Resolve foregrounds and containers together, including custom accents. */
internal fun Palette.toColorScheme(): androidx.compose.material3.ColorScheme {
    val base = if (isDark) darkColorScheme() else lightColorScheme()
    val container = androidx.compose.ui.graphics.lerp(windowBg, accent, if (isDark) 0.22f else 0.12f)
    return base.copy(
        primary = accent,
        onPrimary = if (isDark) Color(0xFF1C1B1F) else Color.White,
        primaryContainer = container, onPrimaryContainer = text,
        secondary = accent2, tertiary = accent3,
        background = windowBg, onBackground = text,
        surface = windowBg, onSurface = text,
        surfaceVariant = surfaceActive, onSurfaceVariant = text2,
        surfaceContainerLow = surface, surfaceContainer = surfaceHover,
        surfaceContainerHigh = surfaceActive,
        surfaceContainerHighest = base.surfaceContainerHighest,
        outline = borderStrong, outlineVariant = border,
        inversePrimary = Accents.color(Accents.DEFAULT_ID, !isDark),
        surfaceTint = accent, error = danger,
    )
}
