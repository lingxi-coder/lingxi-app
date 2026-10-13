package com.lingxi.code.components

import androidx.compose.ui.graphics.Color
import com.lingxi.code.model.SessionMode
import com.lingxi.code.theme.Palette

/**
 * The accent a mode is drawn in — Chat indigo, Code teal, taken from the
 * existing palette so no new colors appear. Used by the new-chat mode bar, the
 * top bar's 「模式 · 工作区」 subtitle and the drawer's filter chips.
 */
fun Palette.modeColor(mode: SessionMode): Color = when (mode) {
    SessionMode.Chat -> accent
    SessionMode.Code -> accent3
}
