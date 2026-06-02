package com.lingxi.code.theme

import androidx.compose.ui.graphics.Color
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * oklch → sRGB sanity checks for the brand [DesignTokens].
 *
 * These pin the canonical converted sRGB values copied verbatim from the iOS
 * `DesignTokens.swift` (the single cross-client source of truth). A regression
 * here means a token was edited away from the shared brand value — which would
 * silently drift the Android client's look from iOS/web. The `Color` channels
 * are read back through its value-class accessors (sRGB), so the assertions are
 * framework-free and run on the plain JVM.
 */
class DesignTokensTest {

    /**
     * Compose `Color` in the standard sRGB space packs each channel into 8 bits
     * (1/255 steps), so a literal like 0.0640f reads back as 16/255 ≈ 0.0627.
     * The epsilon must therefore tolerate one quantization step (~1/255).
     */
    private val eps = 1.0f / 255f + 1e-4f

    private fun assertColor(
        c: Color,
        r: Float,
        g: Float,
        b: Float,
        a: Float = 1f,
        msg: String = "",
    ) {
        assertEquals("$msg red", r, c.red, eps)
        assertEquals("$msg green", g, c.green, eps)
        assertEquals("$msg blue", b, c.blue, eps)
        assertEquals("$msg alpha", a, c.alpha, eps)
    }

    // --- Dark palette canonical values ------------------------------------

    @Test
    fun darkPalette_keyTokens_matchIosConvertedSrgb() {
        val d = DesignTokens.dark
        assertColor(d.appBg, 0.0392f, 0.0392f, 0.0470f, msg = "dark.appBg #0a0a0c")
        assertColor(d.windowBg, 0.0358f, 0.0430f, 0.0640f, msg = "dark.windowBg oklch(15% .012 270)")
        assertColor(d.sidebarBg, 0.0226f, 0.0279f, 0.0471f, msg = "dark.sidebarBg oklch(13% .012 270)")
        assertColor(d.text, 0.9423f, 0.9475f, 0.9616f, msg = "dark.text oklch(96% .005 270)")
        assertColor(d.accent, 0.4340f, 0.5865f, 1.0000f, msg = "dark.accent oklch(70% .18 268)")
        assertColor(d.accent2, 0.8090f, 0.4552f, 0.8891f, msg = "dark.accent2 oklch(70% .18 320)")
        assertColor(d.accent3, 0.0000f, 0.7601f, 0.7664f, msg = "dark.accent3 oklch(72% .16 195)")
        assertColor(d.ok, 0.2305f, 0.7257f, 0.4545f, msg = "dark.ok oklch(70% .15 155)")
    }

    @Test
    fun darkPalette_carriesAlphaOnBorderAndAmbient() {
        val d = DesignTokens.dark
        // border / borderStrong carry the "/ 0.6" and "/ 0.8" oklch alpha.
        assertColor(d.border, 0.1485f, 0.1594f, 0.1902f, a = 0.6f, msg = "dark.border")
        assertColor(d.borderStrong, 0.2130f, 0.2283f, 0.2717f, a = 0.8f, msg = "dark.borderStrong")
        // Ambient top is oklch(40% .15 268 / .20); bottom is fully transparent.
        assertColor(d.ambient.top, 0.1700f, 0.1300f, 0.4200f, a = 0.20f, msg = "dark.ambient.top")
        assertEquals("dark.ambient.bottom alpha", 0f, d.ambient.bottom.alpha, eps)
    }

    @Test
    fun darkPalette_isMarkedDark_andStatusIdleEqualsText4() {
        val d = DesignTokens.dark
        assertTrue("dark.isDark", d.isDark)
        // statusIdle resolves to text4 in the dark palette (per the iOS comment).
        assertColor(d.statusIdle, d.text4.red, d.text4.green, d.text4.blue, msg = "dark.statusIdle == text4")
    }

    // --- Light palette canonical values -----------------------------------

    @Test
    fun lightPalette_keyTokens_matchIosConvertedSrgb() {
        val l = DesignTokens.light
        assertColor(l.appBg, 0.8118f, 0.7882f, 0.8471f, msg = "light.appBg #cfc9d8")
        assertColor(l.windowBg, 0.9804f, 0.9725f, 0.9608f, msg = "light.windowBg #faf8f5")
        assertColor(l.surface, 1.0f, 1.0f, 1.0f, msg = "light.surface #ffffff")
        assertColor(l.text, 0.0736f, 0.0852f, 0.1191f, msg = "light.text oklch(20% .018 270)")
        assertColor(l.accent, 0.1913f, 0.3056f, 0.8696f, msg = "light.accent oklch(50% .22 268)")
        assertColor(l.composerBg, 1.0f, 1.0f, 1.0f, msg = "light.composerBg #ffffff")
    }

    @Test
    fun lightPalette_isNotDark_andDiffersFromDark() {
        val l = DesignTokens.light
        assertTrue("light is not dark", !l.isDark)
        // The two appearances must not be the same palette object/values.
        assertNotEquals("light.windowBg != dark.windowBg", DesignTokens.dark.windowBg, l.windowBg)
        assertNotEquals("light.text != dark.text", DesignTokens.dark.text, l.text)
    }

    @Test
    fun statusColors_sharedAcrossPalettes() {
        // statusConnected / statusTesting / statusError / danger are identical
        // in both appearances (the prototype shares them).
        val d = DesignTokens.dark
        val l = DesignTokens.light
        assertEquals("statusConnected shared", d.statusConnected, l.statusConnected)
        assertEquals("statusTesting shared", d.statusTesting, l.statusTesting)
        assertEquals("statusError shared", d.statusError, l.statusError)
        assertEquals("danger shared", d.danger, l.danger)
        // danger == statusError (both oklch(65% .20 25)).
        assertEquals("danger == statusError", d.statusError, d.danger)
    }

    // --- palette() factory + withAccent override --------------------------

    @Test
    fun paletteFactory_selectsByIsDark() {
        assertEquals(DesignTokens.dark, DesignTokens.palette(isDark = true))
        assertEquals(DesignTokens.light, DesignTokens.palette(isDark = false))
    }

    @Test
    fun withAccent_overridesOnlyAccent() {
        val base = DesignTokens.dark
        val swapped = base.withAccent(Color(red = 1f, green = 0f, blue = 0f))
        assertColor(swapped.accent, 1f, 0f, 0f, msg = "withAccent overrides accent")
        // Everything else is untouched.
        assertEquals("text unchanged", base.text, swapped.text)
        assertEquals("accent2 unchanged", base.accent2, swapped.accent2)
        assertEquals("isDark unchanged", base.isDark, swapped.isDark)
    }
}
