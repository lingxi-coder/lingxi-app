package com.lingxi.code.theme

import androidx.compose.ui.graphics.Color

/**
 * Brand design tokens — the single source of truth for color, ported 1:1 from
 * the iOS `DesignTokens.swift`.
 *
 * oklch → sRGB note
 * -----------------
 * Neither SwiftUI nor Compose has an oklch color space. Every token below was
 * converted once (in the iOS sources) from its original CSS `oklch(L C H)`
 * value to sRGB (D65) using the standard OKLab → linear-sRGB matrix + sRGB
 * gamma. The Swift form `Color(.sRGB, red:R, green:G, blue:B, opacity:O)` maps
 * directly to Compose `Color(red = R, green = G, blue = B, alpha = O)`. The
 * original oklch string is kept in a trailing comment for traceability — do NOT
 * recompute these; they are the canonical brand values shared across clients.
 */

/** The radial/linear ambient gradient stops painted behind the chat view. */
data class AmbientStops(val top: Color, val bottom: Color)

/**
 * A resolved palette for one appearance (dark or light). Mirrors the
 * `tokens(dark)` factory in the web prototype and the iOS `Palette` struct.
 *
 * [accent] is intentionally a `var`-style field copied via [withAccent] so the
 * Appearance accent picker can override it on a resolved palette.
 */
data class Palette(
    val appBg: Color,
    val windowBg: Color,
    val sidebarBg: Color,
    val surface: Color,
    val surfaceHover: Color,
    val surfaceActive: Color,
    val border: Color,
    val borderStrong: Color,
    val text: Color,
    val text2: Color,
    val text3: Color,
    val text4: Color,
    val accent: Color,
    val accent2: Color,
    val accent3: Color,
    val ok: Color,
    val composerBg: Color,
    val ambient: AmbientStops,
    // Status dots (shared across dark/light in the prototype)
    val statusConnected: Color, // oklch(70% 0.15 155)
    val statusIdle: Color,      // == text4
    val statusTesting: Color,   // oklch(75% 0.15 75)
    val statusError: Color,     // oklch(65% 0.20 25)
    val danger: Color,          // oklch(65% 0.20 25)
    /** True when this palette is the dark appearance. */
    val isDark: Boolean,
) {
    /** Return a copy with the accent swapped (Appearance accent override). */
    fun withAccent(newAccent: Color): Palette = copy(accent = newAccent)
}

/** Brand color tokens: the dark + light palettes. */
object DesignTokens {

    // MARK: Dark palette  (tokens(true))
    val dark = Palette(
        appBg = Color(red = 0.0392f, green = 0.0392f, blue = 0.0470f),                 // #0a0a0c
        windowBg = Color(red = 0.0358f, green = 0.0430f, blue = 0.0640f),              // oklch(15% 0.012 270)
        sidebarBg = Color(red = 0.0226f, green = 0.0279f, blue = 0.0471f),             // oklch(13% 0.012 270)
        surface = Color(red = 0.0585f, green = 0.0680f, blue = 0.0955f),               // oklch(18% 0.015 270)
        surfaceHover = Color(red = 0.0897f, green = 0.1029f, blue = 0.1415f),          // oklch(22% 0.020 270)
        surfaceActive = Color(red = 0.1372f, green = 0.1579f, blue = 0.2193f),         // oklch(28% 0.030 270)
        border = Color(red = 0.1485f, green = 0.1594f, blue = 0.1902f, alpha = 0.6f),  // oklch(28% 0.015 270 / 0.6)
        borderStrong = Color(red = 0.2130f, green = 0.2283f, blue = 0.2717f, alpha = 0.8f), // oklch(35% 0.020 270 / 0.8)
        text = Color(red = 0.9423f, green = 0.9475f, blue = 0.9616f),                  // oklch(96% 0.005 270)
        text2 = Color(red = 0.6303f, green = 0.6445f, blue = 0.6837f),                 // oklch(72% 0.015 270)
        text3 = Color(red = 0.3931f, green = 0.4103f, blue = 0.4583f),                 // oklch(52% 0.020 270)
        text4 = Color(red = 0.2434f, green = 0.2591f, blue = 0.3034f),                 // oklch(38% 0.020 270)
        accent = Color(red = 0.4340f, green = 0.5865f, blue = 1.0000f),                // oklch(70% 0.18 268)
        accent2 = Color(red = 0.8090f, green = 0.4552f, blue = 0.8891f),               // oklch(70% 0.18 320)
        accent3 = Color(red = 0.0000f, green = 0.7601f, blue = 0.7664f),               // oklch(72% 0.16 195)
        ok = Color(red = 0.2305f, green = 0.7257f, blue = 0.4545f),                    // oklch(70% 0.15 155)
        composerBg = Color(red = 0.0756f, green = 0.0855f, blue = 0.1137f),            // oklch(20% 0.015 270)
        ambient = AmbientStops(
            top = Color(red = 0.1700f, green = 0.1300f, blue = 0.4200f, alpha = 0.20f), // oklch(40% 0.15 268 / .20)
            bottom = Color.Transparent,
        ),
        statusConnected = Color(red = 0.2305f, green = 0.7257f, blue = 0.4545f),       // oklch(70% 0.15 155)
        statusIdle = Color(red = 0.2434f, green = 0.2591f, blue = 0.3034f),            // text4
        statusTesting = Color(red = 0.8959f, green = 0.6198f, blue = 0.1315f),         // oklch(75% 0.15 75)
        statusError = Color(red = 0.9436f, green = 0.3038f, blue = 0.2990f),           // oklch(65% 0.20 25)
        danger = Color(red = 0.9436f, green = 0.3038f, blue = 0.2990f),                // oklch(65% 0.20 25)
        isDark = true,
    )

    // MARK: Light palette  (tokens(false))
    val light = Palette(
        appBg = Color(red = 0.8118f, green = 0.7882f, blue = 0.8471f),                 // #cfc9d8
        windowBg = Color(red = 0.9804f, green = 0.9725f, blue = 0.9608f),              // #faf8f5
        sidebarBg = Color(red = 0.9423f, green = 0.9475f, blue = 0.9616f),             // oklch(96% 0.005 270)
        surface = Color(red = 1.0f, green = 1.0f, blue = 1.0f),                        // #ffffff
        surfaceHover = Color(red = 0.9392f, green = 0.9475f, blue = 0.9700f),          // oklch(96% 0.008 270)
        surfaceActive = Color(red = 0.8751f, green = 0.8952f, blue = 0.9508f),         // oklch(92% 0.020 270)
        border = Color(red = 0.8361f, green = 0.8441f, blue = 0.8662f),                // oklch(88% 0.008 270)
        borderStrong = Color(red = 0.7563f, green = 0.7681f, blue = 0.8005f),          // oklch(82% 0.012 270)
        text = Color(red = 0.0736f, green = 0.0852f, blue = 0.1191f),                  // oklch(20% 0.018 270)
        text2 = Color(red = 0.2640f, green = 0.2799f, blue = 0.3248f),                 // oklch(40% 0.020 270)
        text3 = Color(red = 0.4608f, green = 0.4785f, blue = 0.5279f),                 // oklch(58% 0.020 270)
        text4 = Color(red = 0.6062f, green = 0.6203f, blue = 0.6592f),                 // oklch(70% 0.015 270)
        accent = Color(red = 0.1913f, green = 0.3056f, blue = 0.8696f),                // oklch(50% 0.22 268)
        accent2 = Color(red = 0.6518f, green = 0.1966f, blue = 0.7474f),               // oklch(55% 0.22 320)
        accent3 = Color(red = 0.0000f, green = 0.5200f, blue = 0.5362f),               // oklch(52% 0.18 195)
        ok = Color(red = 0.0000f, green = 0.5455f, blue = 0.2695f),                    // oklch(55% 0.16 155)
        composerBg = Color(red = 1.0f, green = 1.0f, blue = 1.0f),                     // #ffffff
        ambient = AmbientStops(
            top = Color(red = 0.7400f, green = 0.7900f, blue = 0.9700f, alpha = 0.45f), // oklch(80% 0.10 268 / .45)
            bottom = Color.Transparent,
        ),
        statusConnected = Color(red = 0.2305f, green = 0.7257f, blue = 0.4545f),
        statusIdle = Color(red = 0.6062f, green = 0.6203f, blue = 0.6592f),
        statusTesting = Color(red = 0.8959f, green = 0.6198f, blue = 0.1315f),
        statusError = Color(red = 0.9436f, green = 0.3038f, blue = 0.2990f),
        danger = Color(red = 0.9436f, green = 0.3038f, blue = 0.2990f),
        isDark = false,
    )

    fun palette(isDark: Boolean): Palette = if (isDark) dark else light
}

// MARK: - Accent options (Appearance screen) -------------------------------

/**
 * One of the 6 accent swatches from the prototype's Appearance page.
 * [id] is the original oklch string and doubles as the persisted selection key.
 */
data class AccentOption(
    val id: String,
    val name: String,
    val color: Color,
)

/**
 * The 6 brand accents offered by the Appearance picker.
 *
 * [AccentOption.name] stays the literal zh-Hans copy — this is a plain data
 * object with no `Context`. The real localized text is resolved at the one
 * render site ([com.lingxi.code.settings.AppearancePage]'s `AccentOption.
 * localizedName()`) via `stringResource`, reusing the existing
 * `settings_accent_*` catalog keys.
 */
object Accents {
    val all: List<AccentOption> = listOf(
        AccentOption("oklch(70% 0.18 268)", "靛紫", Color(red = 0.4340f, green = 0.5865f, blue = 1.0000f)),
        AccentOption("oklch(70% 0.18 320)", "玫红", Color(red = 0.8090f, green = 0.4552f, blue = 0.8891f)),
        AccentOption("oklch(72% 0.16 195)", "青蓝", Color(red = 0.0000f, green = 0.7601f, blue = 0.7664f)),
        AccentOption("oklch(72% 0.16 155)", "青绿", Color(red = 0.2085f, green = 0.7571f, blue = 0.4656f)),
        AccentOption("oklch(74% 0.16 75)", "琥珀", Color(red = 0.8960f, green = 0.6013f, blue = 0.0000f)),
        AccentOption("oklch(70% 0.20 30)", "砖红", Color(red = 1.0000f, green = 0.3802f, blue = 0.3010f)),
    )

    /** The default accent id (matches the dark/light palette accent: 靛紫). */
    const val DEFAULT_ID: String = "oklch(70% 0.18 268)"

    fun color(forId: String): Color =
        all.firstOrNull { it.id == forId }?.color ?: all[0].color
}
