package com.lingxi.code.theme

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import org.junit.Assert.*
import org.junit.Test

/** Validate readability and paired Material roles, rather than prototype RGB literals. */
class DesignTokensTest {
    private fun contrast(a: Color, b: Color): Float {
        val x = a.luminance(); val y = b.luminance()
        return (maxOf(x, y) + 0.05f) / (minOf(x, y) + 0.05f)
    }

    @Test fun textRemainsReadableAcrossAllTonalSurfaces() {
        for (palette in listOf(DesignTokens.light, DesignTokens.dark)) {
            for (background in listOf(palette.windowBg, palette.sidebarBg, palette.surface,
                palette.surfaceHover, palette.surfaceActive, palette.composerBg)) {
                for (text in listOf(palette.text, palette.text2, palette.text3, palette.text4)) {
                    assertTrue("Text contrast in dark=${palette.isDark}", contrast(text, background) >= 4.5f)
                }
            }
        }
    }

    @Test fun everyAccentHasReadableMaterialForegroundAndContainerPairs() {
        for (dark in listOf(false, true)) for (accent in Accents.all) {
            val scheme = resolvePalette(dark, accent.id).toColorScheme()
            for ((foreground, background) in listOf(
                scheme.onPrimary to scheme.primary,
                scheme.onPrimaryContainer to scheme.primaryContainer,
                scheme.onSurface to scheme.surface,
                scheme.onSurfaceVariant to scheme.surfaceContainerHighest,
                scheme.onError to scheme.error,
                scheme.onSecondary to scheme.secondary,
                scheme.onTertiary to scheme.tertiary,
            )) assertTrue("Paired contrast dark=$dark accent=${accent.id}", contrast(foreground, background) >= 4.5f)
        }
    }

    @Test fun defaultsFollowSystemAndExplicitPreferencesRemainSupported() {
        assertEquals(ThemeMode.System, ThemeMode.from(null))
        assertEquals(ThemeMode.System, AppearancePrefs().themeMode)
        assertEquals(ThemeMode.Light, ThemeMode.from("light"))
        assertEquals(ThemeMode.Dark, ThemeMode.from("dark"))
        assertEquals(ThemeMode.System, ThemeMode.from("system"))
    }

    @Test fun paletteFactoryAndAccentOverridePreserveAppearance() {
        assertEquals(DesignTokens.dark, DesignTokens.palette(true))
        assertEquals(DesignTokens.light, DesignTokens.palette(false))
        val changed = DesignTokens.dark.withAccent(Color.Red)
        assertEquals(Color.Red, changed.accent)
        assertEquals(DesignTokens.dark.text, changed.text)
        assertTrue(changed.isDark)
        assertNotEquals(DesignTokens.dark.windowBg, DesignTokens.light.windowBg)
        assertEquals(0f, DesignTokens.dark.ambient.top.alpha)
        assertEquals(0f, DesignTokens.light.ambient.top.alpha)
    }
}
