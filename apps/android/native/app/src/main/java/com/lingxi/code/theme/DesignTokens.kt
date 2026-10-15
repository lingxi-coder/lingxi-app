package com.lingxi.code.theme

import androidx.compose.ui.graphics.Color

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
    // MARK: Structured-diff tokens
    //
    // The engine deliberately ships NO diff backgrounds: the terminal's are
    // alpha-over-black blends that are only valid over a black terminal. These
    // are the client-side derivation, keyed off the row kind and the word-diff
    // `emph` flag, and they must stay legible under BOTH appearances.
    /** Ground the diff rows are painted on (a code surface, not the chat surface). */
    val diffSurface: Color,
    /** Added-row wash. */
    val diffAddBg: Color,
    /** Removed-row wash. */
    val diffRemoveBg: Color,
    /** Changed WORD inside an added row — the stronger intra-line emphasis. */
    val diffAddEmphBg: Color,
    /** Changed WORD inside a removed row. */
    val diffRemoveEmphBg: Color,
    /** Line-number gutter foreground. */
    val diffGutter: Color,
    /** The dim `⋯` hunk separator and the diff's hairline rules. */
    val diffRule: Color,
    /** True when this palette is the dark appearance. */
    val isDark: Boolean,
) {
    /** Return a copy with the accent swapped (Appearance accent override). */
    fun withAccent(newAccent: Color): Palette = copy(accent = newAccent)
}

/** Android surfaces use the Material 3 tonal hierarchy in both appearances. */
object DesignTokens {
    val dark = fromScheme(androidx.compose.material3.darkColorScheme(), true)
    val light = fromScheme(androidx.compose.material3.lightColorScheme(), false)

    private fun fromScheme(c: androidx.compose.material3.ColorScheme, dark: Boolean) = Palette(
        appBg = c.background, windowBg = c.background,
        sidebarBg = c.surfaceContainerLow, surface = c.surfaceContainerLow,
        surfaceHover = c.surfaceContainer, surfaceActive = c.surfaceContainerHigh,
        border = c.outlineVariant, borderStrong = c.outline,
        text = c.onSurface, text2 = c.onSurfaceVariant,
        text3 = c.onSurfaceVariant, text4 = c.onSurfaceVariant,
        accent = c.primary, accent2 = c.secondary, accent3 = c.tertiary,
        ok = if (dark) Color(0xFF8CDAA2) else Color(0xFF276A45),
        composerBg = c.surfaceContainerHigh,
        ambient = AmbientStops(Color.Transparent, Color.Transparent),
        statusConnected = if (dark) Color(0xFF8CDAA2) else Color(0xFF276A45),
        statusIdle = c.onSurfaceVariant,
        statusTesting = if (dark) Color(0xFFFFB95C) else Color(0xFF815500),
        statusError = c.error, danger = c.error,
        diffSurface = c.surfaceContainerLowest,
        diffAddBg = if (dark) Color(0x3326A641) else Color(0x2E2DA44E),
        diffRemoveBg = if (dark) Color(0x33F85149) else Color(0x2ECF222E),
        diffAddEmphBg = if (dark) Color(0x6626A641) else Color(0x5C2DA44E),
        diffRemoveEmphBg = if (dark) Color(0x66F85149) else Color(0x5CCF222E),
        diffGutter = c.onSurfaceVariant, diffRule = c.outlineVariant, isDark = dark,
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

    fun color(forId: String, dark: Boolean = true): Color {
        val index = all.indexOfFirst { it.id == forId }.takeIf { it >= 0 } ?: 0
        return (if (dark) darkTones else lightTones)[index]
    }

    private val darkTones = listOf(0xFFD0BCFF, 0xFFFFB1C8, 0xFF80D5D9, 0xFF8CDAA2, 0xFFFFB95C, 0xFFFFB4AB).map { Color(it) }
    private val lightTones = listOf(0xFF6750A4, 0xFF9A405D, 0xFF00696E, 0xFF276A45, 0xFF815500, 0xFFBA1A1A).map { Color(it) }
}
