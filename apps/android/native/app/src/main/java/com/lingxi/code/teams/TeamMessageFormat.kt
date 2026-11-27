package com.lingxi.code.teams

import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import org.json.JSONArray
import org.json.JSONObject

internal enum class TeamTextStyle(val bit: Int) {
    Bold(1), Italic(2), Underline(4), Strike(8),
}

internal data class TeamMessageFormat(val offset: Int, val length: Int, val style: Int) {
    val end get() = offset + length
    fun json() = JSONObject().put("offset", offset).put("length", length).put("style", style)

    fun span() = SpanStyle(
        fontWeight = FontWeight.Bold.takeIf { style and 1 != 0 },
        fontStyle = FontStyle.Italic.takeIf { style and 2 != 0 },
        textDecoration = TextDecoration.combine(buildList {
            if (style and 4 != 0) add(TextDecoration.Underline)
            if (style and 8 != 0) add(TextDecoration.LineThrough)
        }).takeIf { style and 12 != 0 },
    )

    companion object {
        const val LIMIT = 256

        fun valid(runs: List<TeamMessageFormat>, text: String): Boolean {
            var end = 0
            return runs.size <= LIMIT && runs.all {
                val valid = it.style in 1..15 && it.offset >= end && it.offset <= text.length &&
                    it.length in 1..text.length - it.offset &&
                    !splitsSurrogate(text, it.offset) && !splitsSurrogate(text, it.end)
                if (valid) end = it.end
                valid
            }
        }

        private fun splitsSurrogate(text: String, index: Int) =
            index in 1 until text.length && text[index - 1].isHighSurrogate() && text[index].isLowSurrogate()

        fun parse(value: JSONArray?, text: String): List<TeamMessageFormat> = runCatching {
            value?.objects()?.map { TeamMessageFormat(it.getInt("offset"), it.getInt("length"), it.getInt("style")) }
                .orEmpty().takeIf { valid(it, text) }.orEmpty()
        }.getOrDefault(emptyList())

        private fun merged(runs: List<TeamMessageFormat>): List<TeamMessageFormat> = buildList {
            for (run in runs.sortedBy { it.offset }) {
                val last = lastOrNull()
                if (last != null && last.end == run.offset && last.style == run.style) {
                    set(lastIndex, last.copy(length = last.length + run.length))
                } else add(run)
            }
        }

        fun replace(runs: List<TeamMessageFormat>, start: Int, end: Int, count: Int, insertedStyle: Int = 0): List<TeamMessageFormat> {
            val shift = count - (end - start)
            return merged(buildList {
                for (run in runs) {
                    val leftEnd = minOf(start, run.end)
                    if (leftEnd > run.offset) add(run.copy(length = leftEnd - run.offset))
                    val rightStart = maxOf(end, run.offset)
                    if (run.end > rightStart) add(TeamMessageFormat(rightStart + shift, run.end - rightStart, run.style))
                }
                if (count > 0 && insertedStyle != 0) add(TeamMessageFormat(start, count, insertedStyle))
            })
        }

        fun styleAt(runs: List<TeamMessageFormat>, caret: Int): Int =
            runs.firstOrNull { caret > it.offset && caret <= it.end }?.style ?: 0

        fun toggle(runs: List<TeamMessageFormat>, start: Int, end: Int, bit: Int): List<TeamMessageFormat> {
            val boundaries = (runs.flatMap { listOf(it.offset, it.end) } + listOf(start, end)).distinct().sorted()
            val parts = boundaries.zipWithNext().map { (a, b) ->
                TeamMessageFormat(a, b - a, styleAt(runs, b))
            }
            val remove = parts.filter { it.offset >= start && it.end <= end }.all { it.style and bit != 0 }
            return merged(parts.map {
                if (it.offset < start || it.end > end) it
                else it.copy(style = if (remove) it.style and bit.inv() else it.style or bit)
            }.filter { it.style != 0 })
        }

        fun shift(runs: List<TeamMessageFormat>, before: String, after: String, typing: Int? = null): List<TeamMessageFormat> {
            if (before == after) return runs
            var start = before.commonPrefixWith(after).length
            var suffix = before.substring(start).commonSuffixWith(after.substring(start)).length
            if (splitsSurrogate(before, start) || splitsSurrogate(after, start)) start--
            if (splitsSurrogate(before, before.length - suffix) || splitsSurrogate(after, after.length - suffix)) suffix--
            return replace(runs, start, before.length - suffix, after.length - start - suffix, typing ?: styleAt(runs, start))
        }
    }
}
