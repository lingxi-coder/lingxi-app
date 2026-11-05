package com.lingxi.code.theme
import androidx.compose.runtime.staticCompositionLocalOf
// Avatar appearance boundary only; the avatar art and hash use actual product sources.
data class ProbePalette(val isDark: Boolean = false)
val LocalPalette = staticCompositionLocalOf { ProbePalette() }
