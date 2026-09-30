/*
 * Copyright (C) 2025 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 */
package com.lingxi.code.terminal.emulator

import androidx.compose.runtime.State
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf

class TerminalEmulator(columns: Int = 80, rows: Int = 24) {
    private val redraw = mutableLongStateOf(0)
    val version: State<Long> = redraw
    val primaryBuffer = TerminalBuffer(columns, rows)
    val alternateBuffer = TerminalBuffer(columns, rows, 0).also { it.scrollbackEnabled = false }
    private val parser = AnsiParser()
    private val style = CursorStyle()
    private var alternate = false
    private val selection = mutableStateOf<IntArray?>(null)

    val selectionRect: State<IntArray?> = selection
    val activeBuffer get() = if (alternate) alternateBuffer else primaryBuffer
    var columns = columns; private set
    var rows = rows; private set
    var applicationCursorKeys = false; private set
    var autoWrap = true; private set
    var cursorVisible = true; private set
    var bracketedPaste = false; private set
    var cursorShape = CursorShape.BLOCK; private set
    var title = ""; private set
    var scrollOffset = 0
        set(value) { field = value.coerceIn(0, activeBuffer.scrollback.size); redraw() }
    var onResponse: ((ByteArray) -> Unit)? = null
    var onOpenUrl: ((String) -> Unit)? = null

    fun feed(bytes: ByteArray) {
        parser.feed(bytes, ::handle)
        redraw()
    }

    fun resize(newColumns: Int, newRows: Int) {
        if (newColumns <= 0 || newRows <= 0) return
        columns = newColumns; rows = newRows
        primaryBuffer.resize(newColumns, newRows)
        alternateBuffer.resize(newColumns, newRows)
        redraw()
    }

    fun cursorPosition() = activeBuffer.cursorColumn to activeBuffer.cursorRow

    fun visibleLines(): List<Array<TerminalCell>> {
        val buffer = activeBuffer
        if (scrollOffset == 0) return buffer.grid.toList()
        val all = buffer.scrollback.toList() + buffer.grid.toList()
        val start = (all.size - rows - scrollOffset).coerceAtLeast(0)
        return (start until start + rows).map { all.getOrElse(it) { Array(columns) { TerminalCell.BLANK } } }
    }

    fun setSelection(startColumn: Int, startRow: Int, endColumn: Int, endRow: Int) {
        selection.value = intArrayOf(startColumn, startRow, endColumn, endRow)
        redraw()
    }

    fun clearSelection() { selection.value = null; redraw() }

    fun selectedText(): String {
        val rect = selection.value ?: return ""
        val (start, end) = normalize(rect)
        val lines = visibleLines()
        return (start.second..end.second).joinToString("\n") { rowIndex ->
            val line = lines.getOrNull(rowIndex) ?: return@joinToString ""
            val from = if (rowIndex == start.second) start.first else 0
            val to = if (rowIndex == end.second) end.first else line.lastIndex
            buildString {
                for (index in from.coerceAtLeast(0)..to.coerceAtMost(line.lastIndex)) {
                    if (!line[index].wideTrailer) appendCodePoint(line[index].codePoint)
                }
            }.trimEnd()
        }
    }

    fun reset() {
        parser.reset()
        primaryBuffer.eraseDisplay(3); primaryBuffer.moveTo(0, 0)
        alternateBuffer.eraseDisplay(3); alternateBuffer.moveTo(0, 0)
        alternate = false
        applicationCursorKeys = false
        autoWrap = true
        cursorVisible = true
        bracketedPaste = false
        scrollOffset = 0
        style.foreground = TerminalColor.Default
        style.background = TerminalColor.Default
        style.attributes = TextAttributes()
        redraw()
    }

    private fun handle(action: ParsedAction) = when (action) {
        is ParsedAction.Printable -> activeBuffer.write(action.codePoint, style, autoWrap)
        is ParsedAction.Control -> when (action.value) {
            8 -> activeBuffer.backward(1)
            9 -> activeBuffer.tab()
            10, 11, 12 -> activeBuffer.lineFeed()
            13 -> activeBuffer.carriageReturn()
            else -> Unit
        }
        is ParsedAction.Esc -> when (action.final) {
            '7' -> activeBuffer.save(style)
            '8' -> copyStyle(activeBuffer.restore())
            'D' -> activeBuffer.lineFeed()
            'M' -> activeBuffer.reverseIndex()
            'E' -> { activeBuffer.carriageReturn(); activeBuffer.lineFeed() }
            'c' -> reset()
            else -> Unit
        }
        is ParsedAction.Csi -> handleCsi(action)
        is ParsedAction.Osc -> when (action.command) {
            0, 2 -> title = action.payload
            8 -> action.payload.substringAfter(';', "").takeIf(String::isNotBlank)?.let { onOpenUrl?.invoke(it) }
            1337 -> action.payload.removePrefix("MinisOpenURL=").takeIf { it != action.payload }?.let { onOpenUrl?.invoke(it) }
            else -> Unit
        }
    }

    private fun handleCsi(csi: ParsedAction.Csi) {
        val p = csi.params
        fun param(index: Int, default: Int = 1) = p.getOrNull(index)?.takeIf { it != 0 } ?: default
        if (csi.privateMarker == '?') {
            p.forEach {
                when (it) {
                    1 -> applicationCursorKeys = csi.final == 'h'
                    7 -> autoWrap = csi.final == 'h'
                    25 -> cursorVisible = csi.final == 'h'
                    47, 1047, 1049 -> alternate = csi.final == 'h'
                    2004 -> bracketedPaste = csi.final == 'h'
                }
            }
            return
        }
        when (csi.final) {
            'A' -> activeBuffer.up(param(0)); 'B' -> activeBuffer.down(param(0))
            'C' -> activeBuffer.forward(param(0)); 'D' -> activeBuffer.backward(param(0))
            'E' -> { activeBuffer.down(param(0)); activeBuffer.carriageReturn() }
            'F' -> { activeBuffer.up(param(0)); activeBuffer.carriageReturn() }
            'G' -> activeBuffer.moveColumn(param(0) - 1)
            'H', 'f' -> activeBuffer.moveTo(param(1) - 1, param(0) - 1)
            'J' -> activeBuffer.eraseDisplay(p.getOrElse(0) { 0 })
            'K' -> activeBuffer.eraseLine(p.getOrElse(0) { 0 })
            'X' -> activeBuffer.eraseCharacters(param(0))
            'L' -> activeBuffer.insertLines(param(0)); 'M' -> activeBuffer.deleteLines(param(0))
            '@' -> activeBuffer.insertCharacters(param(0)); 'P' -> activeBuffer.deleteCharacters(param(0))
            'S' -> activeBuffer.scrollUp(param(0)); 'T' -> activeBuffer.scrollDown(param(0))
            'r' -> activeBuffer.setScrollRegion(param(0), param(1, rows))
            'm' -> handleSgr(if (p.isEmpty()) intArrayOf(0) else p)
            's' -> activeBuffer.save(style); 'u' -> copyStyle(activeBuffer.restore())
            'n' -> if (p.firstOrNull() == 6) respond("\u001b[${activeBuffer.cursorRow + 1};${activeBuffer.cursorColumn + 1}R")
            'c' -> respond("\u001b[?62;22c")
            'q' -> cursorShape = when (p.firstOrNull()) { 3, 4 -> CursorShape.UNDERLINE; 5, 6 -> CursorShape.BAR; else -> CursorShape.BLOCK }
            else -> Unit
        }
    }

    private fun handleSgr(params: IntArray) {
        var index = 0
        while (index < params.size) {
            when (val value = params[index]) {
                0 -> { style.foreground = TerminalColor.Default; style.background = TerminalColor.Default; style.attributes = TextAttributes() }
                1 -> style.attributes = style.attributes.with(TextAttributes.BOLD)
                2 -> style.attributes = style.attributes.with(TextAttributes.DIM)
                3 -> style.attributes = style.attributes.with(TextAttributes.ITALIC)
                4 -> style.attributes = style.attributes.with(TextAttributes.UNDERLINE)
                5 -> style.attributes = style.attributes.with(TextAttributes.BLINK)
                7 -> style.attributes = style.attributes.with(TextAttributes.INVERSE)
                8 -> style.attributes = style.attributes.with(TextAttributes.HIDDEN)
                9 -> style.attributes = style.attributes.with(TextAttributes.STRIKETHROUGH)
                22 -> style.attributes = style.attributes.without(TextAttributes.BOLD).without(TextAttributes.DIM)
                23 -> style.attributes = style.attributes.without(TextAttributes.ITALIC)
                24 -> style.attributes = style.attributes.without(TextAttributes.UNDERLINE)
                27 -> style.attributes = style.attributes.without(TextAttributes.INVERSE)
                30,31,32,33,34,35,36,37 -> style.foreground = TerminalColor.Indexed(value - 30)
                39 -> style.foreground = TerminalColor.Default
                40,41,42,43,44,45,46,47 -> style.background = TerminalColor.Indexed(value - 40)
                49 -> style.background = TerminalColor.Default
                in 90..97 -> style.foreground = TerminalColor.Indexed(value - 82)
                in 100..107 -> style.background = TerminalColor.Indexed(value - 92)
                38, 48 -> {
                    val foreground = value == 38
                    if (params.getOrNull(index + 1) == 5 && params.getOrNull(index + 2) != null) {
                        val color = TerminalColor.Indexed(params[index + 2])
                        if (foreground) style.foreground = color else style.background = color
                        index += 2
                    } else if (params.getOrNull(index + 1) == 2 && params.getOrNull(index + 4) != null) {
                        val color = TerminalColor.Rgb(params[index + 2], params[index + 3], params[index + 4])
                        if (foreground) style.foreground = color else style.background = color
                        index += 4
                    }
                }
            }
            index++
        }
    }

    private fun copyStyle(other: CursorStyle) {
        style.foreground = other.foreground; style.background = other.background; style.attributes = other.attributes
    }
    private fun respond(text: String) = onResponse?.invoke(text.toByteArray())
    private fun redraw() { redraw.longValue++ }
    private fun normalize(rect: IntArray): Pair<Pair<Int, Int>, Pair<Int, Int>> {
        val a = rect[0] to rect[1]; val b = rect[2] to rect[3]
        return if (a.second < b.second || (a.second == b.second && a.first <= b.first)) a to b else b to a
    }
}
