package com.lingxi.code.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.luminance

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

/**
 * Resolve foregrounds and containers together, including custom accents. Every role is taken from the palette, so none falls back to the stock
 * (purple) scheme: a container is its tone washed into [Palette.windowBg], and the ink on a tone ([onTone]) is the palette's own background or
 * text colour, whichever reads better, so a custom accent never needs a hard-coded black or white. `ThemeColorRolesTest` holds every pair at 4.5:1.
 */
internal fun Palette.toColorScheme(): ColorScheme {
    val base = if (isDark) darkColorScheme() else lightColorScheme()
    // The primary container keeps its ratio; the secondary one is the accent again, quieter, so a chip, a tonal button or the selected tab follows
    // the chosen accent instead of staying lavender; the tertiary and error ones wash their own tones.
    fun wash(tone: Color, dark: Float, light: Float) = lerp(windowBg, tone, if (isDark) dark else light)
    return base.copy(
        primary = accent, onPrimary = onTone(accent),
        primaryContainer = wash(accent, 0.22f, 0.12f), onPrimaryContainer = text,
        secondary = accent2, onSecondary = onTone(accent2),
        secondaryContainer = wash(accent, 0.14f, 0.08f), onSecondaryContainer = text,
        tertiary = accent3, onTertiary = onTone(accent3),
        tertiaryContainer = wash(accent3, 0.22f, 0.12f), onTertiaryContainer = text,
        error = danger, onError = onTone(danger),
        errorContainer = wash(danger, 0.22f, 0.12f), onErrorContainer = text,
        background = windowBg, onBackground = text,
        surface = windowBg, onSurface = text,
        surfaceVariant = surfaceActive, onSurfaceVariant = text2,
        // The tonal ladder: the three tokens the palette has, one rung darker than the window below them and one lighter than the active tone above.
        surfaceContainerLowest = if (isDark) lerp(windowBg, Color.Black, 0.25f) else Color.White,
        surfaceContainerLow = surface, surfaceContainer = surfaceHover,
        surfaceContainerHigh = surfaceActive,
        surfaceContainerHighest = lerp(surfaceActive, text, if (isDark) 0.06f else 0.03f),
        surfaceDim = if (isDark) windowBg else lerp(windowBg, text, 0.14f),
        surfaceBright = if (isDark) lerp(windowBg, text, 0.19f) else windowBg,
        inverseSurface = text, inverseOnSurface = windowBg,
        outline = borderStrong, outlineVariant = border,
        inversePrimary = Accents.color(Accents.DEFAULT_ID, !isDark),
        surfaceTint = accent,
    )
}

/** The ink for content on [tone]: the palette's background or text colour, whichever contrasts more with it. */
internal fun Palette.onTone(tone: Color): Color = if (contrastRatio(windowBg, tone) >= contrastRatio(text, tone)) windowBg else text

/** WCAG 2.x contrast ratio of two opaque colours, 1 to 21. */
internal fun contrastRatio(a: Color, b: Color): Float {
    val x = a.luminance()
    val y = b.luminance()
    return (maxOf(x, y) + 0.05f) / (minOf(x, y) + 0.05f)
}
