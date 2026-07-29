/*
 * Copyright (C) 2025 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 *
 * Adapted from OpenMinis Android terminal emulator.
 */
package com.lingxi.code.terminal.emulator

import android.graphics.Color

sealed interface TerminalColor {
    data object Default : TerminalColor
    data class Indexed(val index: Int) : TerminalColor
    data class Rgb(val red: Int, val green: Int, val blue: Int) : TerminalColor
}

object TerminalPalette {
    const val DEFAULT_FOREGROUND: Int = 0xffd4d4d4.toInt()
    const val DEFAULT_BACKGROUND: Int = Color.BLACK
    private val colors = IntArray(256).also { palette ->
        val base = intArrayOf(
            0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5,
            0x7f7f7f, 0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
        )
        base.forEachIndexed { i, rgb -> palette[i] = Color.rgb(rgb shr 16, rgb shr 8 and 255, rgb and 255) }
        var index = 16
        for (r in 0..5) for (g in 0..5) for (b in 0..5) {
            fun channel(v: Int) = if (v == 0) 0 else 55 + 40 * v
            palette[index++] = Color.rgb(channel(r), channel(g), channel(b))
        }
        repeat(24) {
            val value = 8 + it * 10
            palette[index++] = Color.rgb(value, value, value)
        }
    }

    fun resolve(color: TerminalColor, foreground: Boolean, bold: Boolean = false): Int = when (color) {
        TerminalColor.Default -> if (foreground) DEFAULT_FOREGROUND else DEFAULT_BACKGROUND
        is TerminalColor.Indexed -> colors[
            if (foreground && bold && color.index in 0..7) color.index + 8
            else color.index.coerceIn(0, 255)
        ]
        is TerminalColor.Rgb -> Color.rgb(color.red, color.green, color.blue)
    }
}

@JvmInline
value class TextAttributes(val bits: Int = 0) {
    fun has(flag: Int) = bits and flag != 0
    fun with(flag: Int) = TextAttributes(bits or flag)
    fun without(flag: Int) = TextAttributes(bits and flag.inv())

    companion object {
        const val BOLD = 1 shl 0
        const val DIM = 1 shl 1
        const val ITALIC = 1 shl 2
        const val UNDERLINE = 1 shl 3
        const val BLINK = 1 shl 4
        const val INVERSE = 1 shl 5
        const val HIDDEN = 1 shl 6
        const val STRIKETHROUGH = 1 shl 7
    }
}

data class TerminalCell(
    val codePoint: Int = ' '.code,
    val foreground: TerminalColor = TerminalColor.Default,
    val background: TerminalColor = TerminalColor.Default,
    val attributes: TextAttributes = TextAttributes(),
    val width: Int = 1,
    val wideTrailer: Boolean = false,
) {
    companion object { val BLANK = TerminalCell() }
}

data class CursorStyle(
    var foreground: TerminalColor = TerminalColor.Default,
    var background: TerminalColor = TerminalColor.Default,
    var attributes: TextAttributes = TextAttributes(),
) {
    fun cell(codePoint: Int, width: Int) =
        TerminalCell(codePoint, foreground, background, attributes, width)
}

enum class CursorShape { BLOCK, UNDERLINE, BAR }
