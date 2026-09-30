/*
 * Copyright (C) 2026 LingXi contributors
 * SPDX-License-Identifier: GPL-3.0-only
 */
package com.lingxi.code.shell

import java.util.regex.Pattern
import java.util.regex.PatternSyntaxException

/**
 * Small, platform-independent reader for the checked-in bashism rule asset.
 *
 * Keeping this independent of android.jar makes the exact production parser
 * runnable in local JVM tests without adding another JSON dependency.
 */
internal object BashismRuleJson {
    fun parse(json: String): List<BashismDetector.Rule> {
        val root = JsonReader(json).readValue() as? Map<*, *>
            ?: throw IllegalArgumentException("bashism rule document must be an object")
        val entries = root["rules"] as? List<*>
            ?: throw IllegalArgumentException("bashism rule document has no rules array")
        return entries.mapNotNull { raw ->
            val item = raw as? Map<*, *> ?: return@mapNotNull null
            val name = item["name"] as? String ?: return@mapNotNull null
            val tier = (item["tier"] as? String)
                ?.let { runCatching { BashismDetector.Tier.valueOf(it) }.getOrNull() }
                ?: return@mapNotNull null
            val expression = item["pattern"] as? String ?: return@mapNotNull null
            val pattern = try {
                Pattern.compile(expression)
            } catch (_: PatternSyntaxException) {
                return@mapNotNull null
            }
            BashismDetector.Rule(
                name = name,
                tier = tier,
                regex = pattern,
                behaviorNote = item["behaviorNote"] as? String ?: "",
                fixHint = item["fixHint"] as? String ?: "",
            )
        }
    }

    private class JsonReader(private val source: String) {
        private var offset = 0

        fun readValue(): Any? {
            skipWhitespace()
            val value = when (peek()) {
                '{' -> readObject()
                '[' -> readArray()
                '"' -> readString()
                't' -> readLiteral("true", true)
                'f' -> readLiteral("false", false)
                'n' -> readLiteral("null", null)
                '-', in '0'..'9' -> readNumber()
                else -> error("unexpected JSON token")
            }
            skipWhitespace()
            if (offset != source.length) error("trailing JSON content")
            return value
        }

        private fun readNestedValue(): Any? {
            skipWhitespace()
            return when (peek()) {
                '{' -> readObject()
                '[' -> readArray()
                '"' -> readString()
                't' -> readLiteral("true", true)
                'f' -> readLiteral("false", false)
                'n' -> readLiteral("null", null)
                '-', in '0'..'9' -> readNumber()
                else -> error("unexpected JSON token")
            }
        }

        private fun readObject(): Map<String, Any?> {
            expect('{')
            skipWhitespace()
            val result = linkedMapOf<String, Any?>()
            if (peek() == '}') {
                offset++
                return result
            }
            while (true) {
                skipWhitespace()
                val key = readString()
                skipWhitespace()
                expect(':')
                result[key] = readNestedValue()
                skipWhitespace()
                when (peek()) {
                    ',' -> offset++
                    '}' -> {
                        offset++
                        return result
                    }
                    else -> error("expected ',' or '}'")
                }
            }
        }

        private fun readArray(): List<Any?> {
            expect('[')
            skipWhitespace()
            val result = mutableListOf<Any?>()
            if (peek() == ']') {
                offset++
                return result
            }
            while (true) {
                result += readNestedValue()
                skipWhitespace()
                when (peek()) {
                    ',' -> offset++
                    ']' -> {
                        offset++
                        return result
                    }
                    else -> error("expected ',' or ']'")
                }
            }
        }

        private fun readString(): String {
            expect('"')
            val result = StringBuilder()
            while (offset < source.length) {
                when (val char = source[offset++]) {
                    '"' -> return result.toString()
                    '\\' -> {
                        if (offset >= source.length) error("unterminated escape")
                        result.append(
                            when (val escaped = source[offset++]) {
                                '"', '\\', '/' -> escaped
                                'b' -> '\b'
                                'f' -> '\u000c'
                                'n' -> '\n'
                                'r' -> '\r'
                                't' -> '\t'
                                'u' -> readUnicodeEscape()
                                else -> error("invalid escape")
                            },
                        )
                    }
                    else -> {
                        if (char.code < 0x20) error("control character in string")
                        result.append(char)
                    }
                }
            }
            error("unterminated string")
        }

        private fun readUnicodeEscape(): Char {
            if (offset + 4 > source.length) error("short unicode escape")
            val code = source.substring(offset, offset + 4).toIntOrNull(16)
                ?: error("invalid unicode escape")
            offset += 4
            return code.toChar()
        }

        private fun readNumber(): Number {
            val start = offset
            while (offset < source.length && source[offset] in "-+0123456789.eE") offset++
            val token = source.substring(start, offset)
            return token.toLongOrNull() ?: token.toDoubleOrNull() ?: error("invalid number")
        }

        private fun <T> readLiteral(expected: String, value: T): T {
            if (!source.startsWith(expected, offset)) error("invalid literal")
            offset += expected.length
            return value
        }

        private fun expect(expected: Char) {
            if (peek() != expected) error("expected '$expected'")
            offset++
        }

        private fun peek(): Char = source.getOrNull(offset) ?: error("unexpected end of JSON")

        private fun skipWhitespace() {
            while (offset < source.length && source[offset].isWhitespace()) offset++
        }

        private fun error(message: String): Nothing =
            throw IllegalArgumentException("$message at offset $offset")
    }
}
