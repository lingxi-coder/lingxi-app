package com.lingxi.code.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

/**
 * Brand theme.
 *
 * Wires a Material 3 [MaterialTheme] (typography + shapes + a derived color
 * scheme) and, crucially, provides the full brand [Palette] through the
 * [LocalPalette] CompositionLocal — the Compose analog of the iOS
 * `@Environment(\.theme)` injection. Surfaces read `LocalPalette.current` (or
 * the [theme] convenience accessor) to reach the exact brand tokens that the
 * Material color scheme can't fully express (text2/3/4, surfaceHover, ambient,
 * status dots, …).
 *
 * The active palette is resolved from the appearance ([darkTheme]) and the
 * selected [accentId], so theme + accent are live and single-sourced.
 */

/**
 * CompositionLocal carrying the resolved brand [Palette]. Defaults to the dark
 * palette so previews and detached composables still render with brand color.
 */
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
    DesignTokens.palette(dark).withAccent(Accents.color(forId = accentId))

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

    CompositionLocalProvider(LocalPalette provides palette) {
        MaterialTheme(
            colorScheme = colorScheme,
            typography = LXTypography,
            shapes = LXShapes,
            content = content,
        )
    }
}

/**
 * Project the brand [Palette] onto a Material 3 color scheme so stock M3
 * components (TopAppBar, Scaffold, Switch, NavigationDrawer) pick up brand
 * color out of the box. Brand-only tokens stay on [Palette] / [LocalPalette].
 */
private fun Palette.toColorScheme() = if (isDark) {
    darkColorScheme(
        primary = accent,
        onPrimary = Color.White,
        secondary = accent2,
        tertiary = accent3,
        background = windowBg,
        onBackground = text,
        surface = surface,
        onSurface = text,
        surfaceVariant = surfaceHover,
        onSurfaceVariant = text2,
        surfaceContainer = surface,
        surfaceContainerHigh = surfaceHover,
        surfaceContainerHighest = surfaceActive,
        outline = border,
        outlineVariant = borderStrong,
        error = danger,
    )
} else {
    lightColorScheme(
        primary = accent,
        onPrimary = Color.White,
        secondary = accent2,
        tertiary = accent3,
        background = windowBg,
        onBackground = text,
        surface = surface,
        onSurface = text,
        surfaceVariant = surfaceHover,
        onSurfaceVariant = text2,
        surfaceContainer = surface,
        surfaceContainerHigh = surfaceHover,
        surfaceContainerHighest = surfaceActive,
        outline = border,
        outlineVariant = borderStrong,
        error = danger,
    )
}
