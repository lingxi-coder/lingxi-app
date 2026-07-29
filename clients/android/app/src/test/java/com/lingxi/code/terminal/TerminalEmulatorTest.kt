/*
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 */
package com.lingxi.code.terminal

import com.lingxi.code.terminal.emulator.AnsiParser
import com.lingxi.code.terminal.emulator.ParsedAction
import com.lingxi.code.terminal.emulator.TerminalEmulator
import com.lingxi.code.terminal.emulator.TerminalPalette
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TerminalEmulatorTest {
    @Test
    fun utf8AndEscapeSequencesCanSpanFeedChunks() {
        val parser = AnsiParser()
        val actions = mutableListOf<ParsedAction>()
        val utf8 = "你".toByteArray()

        parser.feed(byteArrayOf(utf8[0]), actions::add)
        parser.feed(byteArrayOf(utf8[1], utf8[2], 0x1b, '['.code.toByte(), '3'.code.toByte()), actions::add)
        parser.feed(byteArrayOf('1'.code.toByte(), 'm'.code.toByte()), actions::add)

        assertEquals("你".codePointAt(0), (actions[0] as ParsedAction.Printable).codePoint)
        val csi = actions[1] as ParsedAction.Csi
        assertEquals(31, csi.params.single())
        assertEquals('m', csi.final)
    }

    @Test
    fun oscStringTerminatorCanSpanChunks() {
        val parser = AnsiParser()
        val actions = mutableListOf<ParsedAction>()
        parser.feed("\u001b]8;;https://example.com\u001b".toByteArray(), actions::add)
        parser.feed("\\".toByteArray(), actions::add)

        val osc = actions.single() as ParsedAction.Osc
        assertEquals(8, osc.command)
        assertEquals(";https://example.com", osc.payload)
    }

    @Test
    fun overlongOscSequenceIsDroppedAndParserRecovers() {
        val parser = AnsiParser()
        val actions = mutableListOf<ParsedAction>()

        parser.feed(("\u001b]8;;" + "x".repeat(9_000) + "\u0007").toByteArray(), actions::add)
        parser.feed("A".toByteArray(), actions::add)

        val printable = actions.single() as ParsedAction.Printable
        assertEquals('A'.code, printable.codePoint)
    }

    @Test
    fun hugeCsiParameterIsSaturatedWithoutBreakingFollowingText() {
        val parser = AnsiParser()
        val actions = mutableListOf<ParsedAction>()

        parser.feed("\u001b[999999999999C".toByteArray(), actions::add)
        parser.feed("B".toByteArray(), actions::add)

        val csi = actions[0] as ParsedAction.Csi
        assertEquals(65_535, csi.params.single())
        val printable = actions[1] as ParsedAction.Printable
        assertEquals('B'.code, printable.codePoint)
    }

    @Test
    fun sgrAndCursorMovementUpdateCells() {
        val terminal = TerminalEmulator(columns = 8, rows = 2)
        terminal.feed("\u001b[31mA\u001b[2CB".toByteArray())

        val line = terminal.visibleLines().first()
        assertEquals('A'.code, line[0].codePoint)
        assertEquals(TerminalPalette.resolve(line[0].foreground, true), TerminalPalette.resolve(line[3].foreground, true))
        assertEquals('B'.code, line[3].codePoint)
    }

    @Test
    fun alternateScreenRestoresPrimaryContent() {
        val terminal = TerminalEmulator(columns = 6, rows = 2)
        terminal.feed("main".toByteArray())
        terminal.feed("\u001b[?1049hother".toByteArray())
        assertTrue(terminal.visibleLines().first().asString().startsWith("other"))

        terminal.feed("\u001b[?1049l".toByteArray())
        assertTrue(terminal.visibleLines().first().asString().startsWith("main"))
    }

    @Test
    fun selectionNormalizesReverseCoordinatesAndSkipsWideTrailers() {
        val terminal = TerminalEmulator(columns = 6, rows = 2)
        terminal.feed("abc\r\n你x".toByteArray())
        terminal.setSelection(1, 1, 0, 0)

        assertEquals("abc\n你", terminal.selectedText())
        terminal.clearSelection()
        assertFalse(terminal.selectionRect.value != null)
    }

    private fun Array<com.lingxi.code.terminal.emulator.TerminalCell>.asString() =
        buildString {
            this@asString.forEach { cell ->
                if (!cell.wideTrailer) appendCodePoint(cell.codePoint)
            }
        }
}
