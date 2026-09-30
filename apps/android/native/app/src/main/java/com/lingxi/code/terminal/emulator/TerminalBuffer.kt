/*
 * Copyright (C) 2025 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 */
package com.lingxi.code.terminal.emulator

class TerminalBuffer(
    var columns: Int,
    var rows: Int,
    private val maxScrollback: Int = 2_000,
) {
    var grid = Array(rows) { blankLine() }
        private set
    val scrollback = ArrayDeque<Array<TerminalCell>>()
    var cursorColumn = 0
    var cursorRow = 0
    var scrollTop = 0
    var scrollBottom = rows - 1
    var wrapPending = false
    var scrollbackEnabled = true
    private var savedColumn = 0
    private var savedRow = 0
    private var savedStyle = CursorStyle()

    private fun blankLine() = Array(columns) { TerminalCell.BLANK }

    fun moveTo(column: Int, row: Int) {
        cursorColumn = column.coerceIn(0, columns - 1)
        cursorRow = row.coerceIn(0, rows - 1)
        wrapPending = false
    }

    fun moveColumn(column: Int) = moveTo(column, cursorRow)
    fun up(count: Int) = moveTo(cursorColumn, (cursorRow - count).coerceAtLeast(scrollTop))
    fun down(count: Int) = moveTo(cursorColumn, (cursorRow + count).coerceAtMost(scrollBottom))
    fun forward(count: Int) = moveTo((cursorColumn + count).coerceAtMost(columns - 1), cursorRow)
    fun backward(count: Int) = moveTo((cursorColumn - count).coerceAtLeast(0), cursorRow)

    fun carriageReturn() { cursorColumn = 0; wrapPending = false }
    fun lineFeed() {
        if (cursorRow == scrollBottom) scrollUp(1) else cursorRow++
        wrapPending = false
    }
    fun reverseIndex() {
        if (cursorRow == scrollTop) scrollDown(1) else cursorRow--
        wrapPending = false
    }

    fun write(codePoint: Int, style: CursorStyle, autoWrap: Boolean) {
        val width = characterWidth(codePoint)
        if (wrapPending) {
            if (autoWrap) { carriageReturn(); lineFeed() }
            wrapPending = false
        }
        if (width == 2 && cursorColumn == columns - 1 && autoWrap) {
            carriageReturn()
            lineFeed()
        }
        grid[cursorRow][cursorColumn] = style.cell(codePoint, width)
        if (width == 2 && cursorColumn + 1 < columns) {
            grid[cursorRow][cursorColumn + 1] = style.cell(' '.code, 1).copy(wideTrailer = true)
        }
        cursorColumn += width
        if (cursorColumn >= columns) {
            cursorColumn = columns - 1
            wrapPending = true
        }
    }

    fun tab() { moveColumn(((cursorColumn / 8 + 1) * 8).coerceAtMost(columns - 1)) }

    fun scrollUp(count: Int) = repeat(count.coerceAtMost(scrollBottom - scrollTop + 1)) {
        if (scrollbackEnabled && scrollTop == 0) {
            scrollback.addLast(grid[0])
            while (scrollback.size > maxScrollback) scrollback.removeFirst()
        }
        for (row in scrollTop until scrollBottom) grid[row] = grid[row + 1]
        grid[scrollBottom] = blankLine()
    }

    fun scrollDown(count: Int) = repeat(count.coerceAtMost(scrollBottom - scrollTop + 1)) {
        for (row in scrollBottom downTo scrollTop + 1) grid[row] = grid[row - 1]
        grid[scrollTop] = blankLine()
    }

    fun eraseDisplay(mode: Int) {
        when (mode) {
            0 -> { eraseLine(0); for (r in cursorRow + 1 until rows) grid[r] = blankLine() }
            1 -> { eraseLine(1); for (r in 0 until cursorRow) grid[r] = blankLine() }
            2 -> for (r in grid.indices) grid[r] = blankLine()
            3 -> { for (r in grid.indices) grid[r] = blankLine(); scrollback.clear() }
        }
    }

    fun eraseLine(mode: Int) {
        val range = when (mode) {
            0 -> cursorColumn until columns
            1 -> 0..cursorColumn
            else -> 0 until columns
        }
        range.forEach { grid[cursorRow][it] = TerminalCell.BLANK }
    }

    fun eraseCharacters(count: Int) {
        for (column in cursorColumn until (cursorColumn + count).coerceAtMost(columns)) {
            grid[cursorRow][column] = TerminalCell.BLANK
        }
    }

    fun insertLines(count: Int) = repeat(count.coerceAtMost(scrollBottom - cursorRow + 1)) {
        for (r in scrollBottom downTo cursorRow + 1) grid[r] = grid[r - 1]
        grid[cursorRow] = blankLine()
    }

    fun deleteLines(count: Int) = repeat(count.coerceAtMost(scrollBottom - cursorRow + 1)) {
        for (r in cursorRow until scrollBottom) grid[r] = grid[r + 1]
        grid[scrollBottom] = blankLine()
    }

    fun insertCharacters(count: Int) {
        val row = grid[cursorRow]
        repeat(count.coerceAtMost(columns - cursorColumn)) {
            for (c in columns - 1 downTo cursorColumn + 1) row[c] = row[c - 1]
            row[cursorColumn] = TerminalCell.BLANK
        }
    }

    fun deleteCharacters(count: Int) {
        val row = grid[cursorRow]
        repeat(count.coerceAtMost(columns - cursorColumn)) {
            for (c in cursorColumn until columns - 1) row[c] = row[c + 1]
            row[columns - 1] = TerminalCell.BLANK
        }
    }

    fun setScrollRegion(topOneBased: Int, bottomOneBased: Int) {
        val top = (topOneBased - 1).coerceAtLeast(0)
        val bottom = (bottomOneBased - 1).coerceAtMost(rows - 1)
        if (top < bottom) { scrollTop = top; scrollBottom = bottom }
    }

    fun save(style: CursorStyle) {
        savedColumn = cursorColumn
        savedRow = cursorRow
        savedStyle = style.copy()
    }

    fun restore(): CursorStyle {
        moveTo(savedColumn, savedRow)
        return savedStyle.copy()
    }

    fun resize(newColumns: Int, newRows: Int) {
        if (newColumns <= 0 || newRows <= 0 || (newColumns == columns && newRows == rows)) return
        val old = grid
        val oldColumns = columns
        val oldRows = rows
        columns = newColumns
        rows = newRows
        grid = Array(newRows) { r ->
            Array(newColumns) { c ->
                if (r < oldRows && c < oldColumns) old[r][c] else TerminalCell.BLANK
            }
        }
        scrollTop = 0
        scrollBottom = rows - 1
        moveTo(cursorColumn, cursorRow)
    }

    companion object {
        fun characterWidth(cp: Int): Int = if (
            cp in 0x1100..0x115f || cp in 0x2e80..0xa4cf ||
            cp in 0xac00..0xd7af || cp in 0xf900..0xfaff ||
            cp in 0xfe10..0xfe6f || cp in 0xff01..0xff60 ||
            cp in 0x1f000..0x1faff || cp in 0x20000..0x3ffff
        ) 2 else 1
    }
}
