/*
 * Copyright (C) 2025 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 */
package com.lingxi.code.terminal.emulator

sealed interface ParsedAction {
    data class Printable(val codePoint: Int) : ParsedAction
    data class Control(val value: Int) : ParsedAction
    data class Csi(val params: IntArray, val privateMarker: Char?, val final: Char) : ParsedAction
    data class Esc(val final: Char) : ParsedAction
    data class Osc(val command: Int, val payload: String) : ParsedAction
}

/** Incremental VT100/xterm parser. Escape and UTF-8 sequences may span feeds. */
class AnsiParser {
    private enum class State {
        GROUND,
        ESCAPE,
        CSI,
        CSI_DISCARD,
        OSC_COMMAND,
        OSC_TEXT,
        OSC_ESC,
        OSC_DISCARD,
        OSC_DISCARD_ESC,
    }
    private var state = State.GROUND
    private val csi = StringBuilder()
    private val osc = StringBuilder()
    private var oscCommand = 0
    private val utf8 = ByteArray(4)
    private var utf8Length = 0
    private var utf8Remaining = 0

    fun feed(bytes: ByteArray, emit: (ParsedAction) -> Unit) {
        bytes.forEach { process(it.toInt() and 255, emit) }
    }

    fun reset() {
        clearEscapeState()
        utf8Length = 0
        utf8Remaining = 0
    }

    private fun process(value: Int, emit: (ParsedAction) -> Unit) {
        if (state == State.GROUND && utf8Remaining > 0) {
            if (value and 0xc0 == 0x80) {
                utf8[utf8Length++] = value.toByte()
                if (--utf8Remaining == 0) emit(ParsedAction.Printable(decodeUtf8()))
                return
            }
            utf8Length = 0
            utf8Remaining = 0
            emit(ParsedAction.Printable(0xfffd))
        }
        when (state) {
            State.GROUND -> when {
                value == 0x1b -> state = State.ESCAPE
                value < 0x20 -> emit(ParsedAction.Control(value))
                value in 0x20..0x7e -> emit(ParsedAction.Printable(value))
                value in 0xc2..0xdf -> beginUtf8(value, 1)
                value in 0xe0..0xef -> beginUtf8(value, 2)
                value in 0xf0..0xf4 -> beginUtf8(value, 3)
                value != 0x7f -> emit(ParsedAction.Printable(0xfffd))
            }
            State.ESCAPE -> when (value) {
                '['.code -> { csi.clear(); state = State.CSI }
                ']'.code -> { oscCommand = 0; osc.clear(); state = State.OSC_COMMAND }
                0x1b -> Unit
                in 0x30..0x7e -> { emit(ParsedAction.Esc(value.toChar())); state = State.GROUND }
                else -> state = State.GROUND
            }
            State.CSI -> when {
                value == 0x1b -> state = State.ESCAPE
                value in 0x40..0x7e -> {
                    val raw = csi.toString()
                    val marker = raw.firstOrNull()?.takeIf { it in "?><!" }
                    val paramsRaw = if (marker == null) raw else raw.drop(1)
                    val params = parseCsiParams(paramsRaw)
                    emit(ParsedAction.Csi(params, marker, value.toChar()))
                    clearEscapeState()
                }
                value in 0x20..0x3f -> appendBounded(csi, value) {
                    csi.clear()
                    state = State.CSI_DISCARD
                }
                else -> clearEscapeState()
            }
            State.CSI_DISCARD -> when {
                value == 0x1b -> state = State.ESCAPE
                value in 0x40..0x7e -> clearEscapeState()
                value !in 0x20..0x3f -> clearEscapeState()
            }
            State.OSC_COMMAND -> when {
                value in '0'.code..'9'.code -> {
                    val digit = value - '0'.code
                    if (oscCommand > MAX_OSC_COMMAND || oscCommand > (MAX_OSC_COMMAND - digit) / 10) {
                        clearEscapeState()
                    } else {
                        oscCommand = oscCommand * 10 + digit
                    }
                }
                value == ';'.code -> state = State.OSC_TEXT
                value == 7 -> finishOsc(emit)
                value == 0x1b -> state = State.OSC_ESC
                else -> clearEscapeState()
            }
            State.OSC_TEXT -> when (value) {
                7 -> finishOsc(emit)
                0x1b -> state = State.OSC_ESC
                else -> appendBounded(osc, value) {
                    osc.clear()
                    state = State.OSC_DISCARD
                }
            }
            State.OSC_ESC -> if (value == '\\'.code) finishOsc(emit) else {
                if (osc.length + 2 > MAX_OSC_CHARS) {
                    clearEscapeState()
                } else {
                    osc.append('\u001b').append(value.toChar())
                    state = State.OSC_TEXT
                }
            }
            State.OSC_DISCARD -> when (value) {
                7 -> clearEscapeState()
                0x1b -> state = State.OSC_DISCARD_ESC
            }
            State.OSC_DISCARD_ESC -> when (value) {
                '\\'.code -> clearEscapeState()
                0x1b -> Unit
                else -> state = State.OSC_DISCARD
            }
        }
    }

    private fun beginUtf8(value: Int, remaining: Int) {
        utf8[0] = value.toByte()
        utf8Length = 1
        utf8Remaining = remaining
    }

    private fun decodeUtf8(): Int {
        val text = utf8.copyOf(utf8Length).toString(Charsets.UTF_8)
        utf8Length = 0
        return text.codePointAt(0)
    }

    private fun finishOsc(emit: (ParsedAction) -> Unit) {
        emit(ParsedAction.Osc(oscCommand, osc.toString()))
        clearEscapeState()
    }

    private fun parseCsiParams(raw: String): IntArray {
        if (raw.isBlank()) return intArrayOf()
        val params = ArrayList<Int>(raw.count { it == ';' } + 1)
        var index = 0
        while (index <= raw.length) {
            val end = raw.indexOf(';', index).let { if (it >= 0) it else raw.length }
            params += parseCsiParam(raw, index, end)
            if (end == raw.length) break
            index = end + 1
        }
        return params.toIntArray()
    }

    private fun parseCsiParam(raw: String, start: Int, end: Int): Int {
        if (start >= end) return 0
        var value = 0
        for (index in start until end) {
            val char = raw[index]
            if (!char.isDigit()) return 0
            val digit = char.code - '0'.code
            value = if (value > (MAX_CSI_PARAM - digit) / 10) {
                MAX_CSI_PARAM
            } else {
                value * 10 + digit
            }
        }
        return value
    }

    private fun appendBounded(
        buffer: StringBuilder,
        value: Int,
        onOverflow: () -> Unit,
    ) {
        if (buffer.length >= bufferMax(buffer)) {
            onOverflow()
            return
        }
        buffer.append(value.toChar())
    }

    private fun bufferMax(buffer: StringBuilder): Int =
        if (buffer === csi) MAX_CSI_CHARS else MAX_OSC_CHARS

    private fun clearEscapeState() {
        state = State.GROUND
        csi.clear()
        osc.clear()
        oscCommand = 0
    }

    private companion object {
        const val MAX_CSI_CHARS = 128
        const val MAX_CSI_PARAM = 65_535
        const val MAX_OSC_CHARS = 8_192
        const val MAX_OSC_COMMAND = 999_999
    }
}
