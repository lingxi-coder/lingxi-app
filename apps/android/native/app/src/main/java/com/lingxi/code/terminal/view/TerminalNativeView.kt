/*
 * Copyright (C) 2025 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 *
 * Adapted from OpenMinis TerminalNativeView. Selection architecture was
 * inspired by Termux terminal-view (Apache-2.0).
 */
package com.lingxi.code.terminal.view

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Rect
import android.graphics.Typeface
import android.net.Uri
import android.view.ActionMode
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.Menu
import android.view.MenuItem
import android.view.MotionEvent
import android.view.View
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView
import com.lingxi.code.terminal.emulator.CursorShape
import com.lingxi.code.terminal.emulator.TerminalCell
import com.lingxi.code.terminal.emulator.TerminalEmulator
import com.lingxi.code.terminal.emulator.TerminalPalette
import com.lingxi.code.terminal.emulator.TextAttributes

@Composable
fun TerminalNativeView(
    emulator: TerminalEmulator,
    onResize: (Int, Int) -> Unit,
    onTap: () -> Unit,
    onOpenUrl: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    AndroidView(
        modifier = modifier,
        factory = { TerminalCanvasView(it) },
        update = { it.attach(emulator, onResize, onTap, onOpenUrl) },
    )
}

class TerminalCanvasView(context: Context) : View(context) {
    private var emulator: TerminalEmulator? = null
    private var onResize: (Int, Int) -> Unit = { _, _ -> }
    private var onTap: () -> Unit = {}
    private var onOpenUrl: (String) -> Unit = {}
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG or Paint.SUBPIXEL_TEXT_FLAG).apply {
        typeface = Typeface.MONOSPACE
        textSize = 13f * resources.displayMetrics.density * resources.configuration.fontScale
    }
    private var cellWidth = paint.measureText("M")
    private var cellHeight = paint.fontMetrics.run { bottom - top }
    private var baseline = -paint.fontMetrics.top
    private var columns = 80
    private var rows = 24
    private var actionMode: ActionMode? = null
    private var blinkOn = true
    private var lastVersion = -1L

    init {
        isFocusable = true
        isClickable = true
    }

    fun attach(
        emulator: TerminalEmulator,
        onResize: (Int, Int) -> Unit,
        onTap: () -> Unit,
        onOpenUrl: (String) -> Unit,
    ) {
        this.emulator = emulator
        this.onResize = onResize
        this.onTap = onTap
        this.onOpenUrl = onOpenUrl
        invalidate()
    }

    private val ticker = object : Runnable {
        override fun run() {
            val current = emulator?.version?.value ?: -1
            if (current != lastVersion) { lastVersion = current; invalidate() }
            postDelayed(this, 16)
        }
    }
    private val blinker = object : Runnable {
        override fun run() { blinkOn = !blinkOn; invalidate(); postDelayed(this, 500) }
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        post(ticker); post(blinker)
    }
    override fun onDetachedFromWindow() {
        removeCallbacks(ticker); removeCallbacks(blinker)
        actionMode?.finish()
        super.onDetachedFromWindow()
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        columns = (w / cellWidth).toInt().coerceAtLeast(1)
        rows = (h / cellHeight).toInt().coerceAtLeast(1)
        onResize(columns, rows)
    }

    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(e: MotionEvent) = true
        override fun onSingleTapUp(e: MotionEvent): Boolean {
            if (emulator?.selectionRect?.value != null) {
                actionMode?.finish()
                emulator?.clearSelection()
            } else onTap()
            return true
        }
        override fun onLongPress(e: MotionEvent) {
            val row = (e.y / cellHeight).toInt().coerceIn(0, rows - 1)
            val column = (e.x / cellWidth).toInt().coerceIn(0, columns - 1)
            val range = wordRange(column, row)
            emulator?.setSelection(range.first, row, range.last, row)
            performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
            showSelectionMenu()
        }
        override fun onScroll(e1: MotionEvent?, e2: MotionEvent, dx: Float, dy: Float): Boolean {
            val delta = (-dy / cellHeight).toInt()
            if (delta != 0) emulator?.let { it.scrollOffset += delta }
            return true
        }
    })

    override fun onTouchEvent(event: MotionEvent) = gestures.onTouchEvent(event)

    private fun wordRange(column: Int, row: Int): IntRange {
        val line = emulator?.visibleLines()?.getOrNull(row) ?: return column..column
        fun word(cp: Int): Boolean {
            val c = cp.toChar()
            return c.isLetterOrDigit() || c in "_-./:?&=+%#~@"
        }
        val safeColumn = column.coerceAtMost(line.lastIndex)
        if (!word(line[safeColumn].codePoint)) return safeColumn..safeColumn
        var start = safeColumn; var end = safeColumn
        while (start > 0 && word(line[start - 1].codePoint)) start--
        while (end < line.lastIndex && word(line[end + 1].codePoint)) end++
        return start..end
    }

    private fun showSelectionMenu() {
        if (actionMode != null) return
        actionMode = startActionMode(object : ActionMode.Callback2() {
            override fun onCreateActionMode(mode: ActionMode, menu: Menu): Boolean {
                menu.add(Menu.NONE, COPY, 0, android.R.string.copy)
                val selected = emulator?.selectedText().orEmpty()
                if (selected.toUriOrNull() != null) menu.add(Menu.NONE, OPEN, 1, "Open")
                menu.add(Menu.NONE, SELECT_ALL, 2, android.R.string.selectAll)
                return true
            }
            override fun onPrepareActionMode(mode: ActionMode, menu: Menu) = false
            override fun onActionItemClicked(mode: ActionMode, item: MenuItem): Boolean = when (item.itemId) {
                COPY -> {
                    val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                    clipboard.setPrimaryClip(ClipData.newPlainText("LingXi Terminal", emulator?.selectedText().orEmpty()))
                    mode.finish(); true
                }
                OPEN -> {
                    emulator?.selectedText()?.toUriOrNull()?.toString()?.let(onOpenUrl)
                    mode.finish(); true
                }
                SELECT_ALL -> {
                    emulator?.setSelection(0, 0, columns - 1, rows - 1)
                    mode.invalidate(); true
                }
                else -> false
            }
            override fun onDestroyActionMode(mode: ActionMode) {
                actionMode = null
                emulator?.clearSelection()
            }
            override fun onGetContentRect(mode: ActionMode, view: View?, outRect: Rect) {
                val rect = emulator?.selectionRect?.value ?: return
                outRect.set(
                    (rect[0] * cellWidth).toInt(), (rect[1] * cellHeight).toInt(),
                    ((rect[2] + 1) * cellWidth).toInt(), ((rect[3] + 1) * cellHeight).toInt(),
                )
            }
        }, ActionMode.TYPE_FLOATING)
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        canvas.drawColor(TerminalPalette.DEFAULT_BACKGROUND)
        val emulator = emulator ?: return
        val lines = emulator.visibleLines()
        lines.forEachIndexed { rowIndex, line ->
            line.forEachIndexed { columnIndex, cell ->
                if (!cell.wideTrailer) drawCell(canvas, cell, columnIndex * cellWidth, rowIndex * cellHeight)
            }
        }
        drawSelection(canvas, emulator.selectionRect.value)
        if (blinkOn && emulator.cursorVisible && emulator.scrollOffset == 0) drawCursor(canvas, emulator, lines)
    }

    private fun drawCell(canvas: Canvas, cell: TerminalCell, x: Float, y: Float) {
        val inverse = cell.attributes.has(TextAttributes.INVERSE)
        val bold = cell.attributes.has(TextAttributes.BOLD)
        val foreground = TerminalPalette.resolve(if (inverse) cell.background else cell.foreground, true, bold)
        val background = TerminalPalette.resolve(if (inverse) cell.foreground else cell.background, false)
        if (background != TerminalPalette.DEFAULT_BACKGROUND) {
            paint.color = background
            canvas.drawRect(x, y, x + cellWidth * cell.width, y + cellHeight, paint)
        }
        paint.color = foreground
        paint.isFakeBoldText = bold
        paint.isUnderlineText = cell.attributes.has(TextAttributes.UNDERLINE)
        paint.isStrikeThruText = cell.attributes.has(TextAttributes.STRIKETHROUGH)
        paint.alpha = when {
            cell.attributes.has(TextAttributes.HIDDEN) -> 0
            cell.attributes.has(TextAttributes.DIM) -> 128
            else -> 255
        }
        canvas.drawText(String(Character.toChars(cell.codePoint)), x, y + baseline, paint)
        paint.alpha = 255
    }

    private fun drawSelection(canvas: Canvas, rect: IntArray?) {
        rect ?: return
        val (start, end) = if (rect[1] < rect[3] || (rect[1] == rect[3] && rect[0] <= rect[2])) {
            (rect[0] to rect[1]) to (rect[2] to rect[3])
        } else (rect[2] to rect[3]) to (rect[0] to rect[1])
        paint.color = 0x663399ff
        for (row in start.second..end.second) {
            val left = if (row == start.second) start.first else 0
            val right = if (row == end.second) end.first + 1 else columns
            canvas.drawRect(left * cellWidth, row * cellHeight, right * cellWidth, (row + 1) * cellHeight, paint)
        }
    }

    private fun drawCursor(canvas: Canvas, emulator: TerminalEmulator, lines: List<Array<TerminalCell>>) {
        val (column, row) = emulator.cursorPosition()
        val left = column * cellWidth; val top = row * cellHeight
        paint.color = TerminalPalette.DEFAULT_FOREGROUND
        when (emulator.cursorShape) {
            CursorShape.BLOCK -> canvas.drawRect(left, top, left + cellWidth, top + cellHeight, paint)
            CursorShape.UNDERLINE -> canvas.drawRect(left, top + cellHeight - 2, left + cellWidth, top + cellHeight, paint)
            CursorShape.BAR -> canvas.drawRect(left, top, left + 2, top + cellHeight, paint)
        }
        if (emulator.cursorShape == CursorShape.BLOCK) {
            val cell = lines.getOrNull(row)?.getOrNull(column) ?: return
            paint.color = TerminalPalette.DEFAULT_BACKGROUND
            canvas.drawText(String(Character.toChars(cell.codePoint)), left, top + baseline, paint)
        }
    }

    private fun String.toUriOrNull(): Uri? = runCatching { Uri.parse(trim()) }.getOrNull()
        ?.takeIf { it.scheme == "http" || it.scheme == "https" || it.scheme == "lingxi" }

    companion object { private const val COPY = 1; private const val OPEN = 2; private const val SELECT_ALL = 3 }
}
