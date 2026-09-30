package com.lingxi.code.theme

import androidx.compose.material3.Typography
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/**
 * Typography.
 *
 * The prototype + iOS use the platform system font for UI text and a monospaced
 * family for code / API-key / keyboard-key fields. On Android that maps to
 * [FontFamily.Default] (the device system font) and [FontFamily.Monospace].
 *
 * Compose's [Typography] roles are kept close to the prototype's literal sizes
 * so screens can reach for a named role instead of hand-rolling `fontSize`
 * everywhere; surfaces that need an exact prototype size (e.g. 11.5sp pills)
 * still override locally. [LXFont.mono] is exposed for the code/key fields.
 */
object LXFont {
    /** System default UI font. */
    val ui: FontFamily = FontFamily.Default

    /** Monospaced family for code blocks, API keys, MCP endpoints, key caps. */
    val mono: FontFamily = FontFamily.Monospace
}

/**
 * Material 3 typography scale mapped onto the prototype's sizes. These feed
 * `MaterialTheme.typography`; brand surfaces that need a precise size keep
 * overriding inline (the prototype is pixel-specific in places).
 */
val LXTypography: Typography = Typography(
    // Large title / sheet titles
    titleLarge = TextStyle(
        fontFamily = LXFont.ui,
        fontWeight = FontWeight.SemiBold,
        fontSize = 20.sp,
        lineHeight = 26.sp,
    ),
    // Top-bar / section titles
    titleMedium = TextStyle(
        fontFamily = LXFont.ui,
        fontWeight = FontWeight.SemiBold,
        fontSize = 16.sp,
        lineHeight = 22.sp,
    ),
    titleSmall = TextStyle(
        fontFamily = LXFont.ui,
        fontWeight = FontWeight.Medium,
        fontSize = 14.sp,
        lineHeight = 20.sp,
    ),
    // Default body / message text
    bodyLarge = TextStyle(
        fontFamily = LXFont.ui,
        fontWeight = FontWeight.Normal,
        fontSize = 15.sp,
        lineHeight = 22.sp,
    ),
    bodyMedium = TextStyle(
        fontFamily = LXFont.ui,
        fontWeight = FontWeight.Normal,
        fontSize = 13.5f.sp,
        lineHeight = 20.sp,
    ),
    bodySmall = TextStyle(
        fontFamily = LXFont.ui,
        fontWeight = FontWeight.Normal,
        fontSize = 11.5f.sp,
        lineHeight = 16.sp,
    ),
    // Row labels
    labelLarge = TextStyle(
        fontFamily = LXFont.ui,
        fontWeight = FontWeight.Medium,
        fontSize = 14.sp,
        lineHeight = 18.sp,
    ),
    labelMedium = TextStyle(
        fontFamily = LXFont.ui,
        fontWeight = FontWeight.Medium,
        fontSize = 12.sp,
        lineHeight = 16.sp,
    ),
    // Uppercased section labels / pills (0.6 tracking applied at use site)
    labelSmall = TextStyle(
        fontFamily = LXFont.ui,
        fontWeight = FontWeight.SemiBold,
        fontSize = 11.sp,
        lineHeight = 14.sp,
    ),
)
