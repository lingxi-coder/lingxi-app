package com.lingxi.code.conversation

/**
 * A tiny, dependency-free Markdown model + parser for assistant replies.
 *
 * The prototype's `AIText` only handled `**bold**`, which left real coding
 * replies (fenced code, inline code, lists) unreadable. This parser covers the
 * subset that matters for those replies — fenced code blocks, inline code,
 * `**bold**`, and bullet / numbered lists — and is deliberately a PURE function
 * with NO Compose / Android dependency so it is exhaustively unit-testable on the
 * plain JVM (see `MarkdownParserTest`). The Compose renderer lives in
 * `MessageBubble.kt` and consumes [MdBlock] / [MdInline].
 *
 * Nested list indentation currently retains the flat list layout. Anything unrecognized degrades to literal paragraph text, so
 * the renderer is never wrong — only less rich.
 */

/** A block-level element. The document is a flat list of these. */
sealed interface MdBlock {
    data class Table(val header: List<List<MdInline>>, val rows: List<List<List<MdInline>>>,
        val alignment: List<TableAlignment>) : MdBlock
    data class Heading(val level: Int, val spans: List<MdInline>) : MdBlock
    data class Quote(val spans: List<MdInline>) : MdBlock

    /** A run of inline content (one logical paragraph; may span source lines). */
    data class Paragraph(val spans: List<MdInline>) : MdBlock

    /** A fenced code block (``` … ```). [language] is the info string (may be empty). */
    data class CodeBlock(val language: String, val code: String) : MdBlock

    /** A bullet list (`-` / `*` / `+`). Each item is its own inline run. */
    data class BulletList(val items: List<List<MdInline>>) : MdBlock

    /** A numbered list (`1.` / `2)` …). Each item carries its rendered [marker]. */
    data class NumberedList(val items: List<NumberedItem>) : MdBlock
}

/** One numbered-list row: the displayed marker (e.g. "1.") + its inline content. */
data class NumberedItem(val marker: String, val spans: List<MdInline>)

/** An inline span within a paragraph or list item. */
enum class TableAlignment { Left, Center, Right }

sealed interface MdInline {
    data class Italic(val text: String) : MdInline
    data class BoldItalic(val text: String) : MdInline
    data class Strike(val text: String) : MdInline
    /** Plain text. */
    data class Text(val text: String) : MdInline

    /** `**bold**` text. */
    data class Bold(val text: String) : MdInline

    /** `` `inline code` ``. */
    data class Code(val text: String) : MdInline

    /** `[label](url)`; the renderer decides which schemes are actionable. */
    data class Link(val label: String, val url: String) : MdInline
}

private val HEADING_RE = Regex("""^ {0,3}(#{1,6})\s+(.+?)(?:\s+#+)?\s*$""")
private val QUOTE_RE = Regex("""^ {0,3}> ?(.*)$""")
private val BULLET_RE = Regex("""^\s*[-*+]\s+(.*)$""")
private val NUMBERED_RE = Regex("""^\s*(\d+)[.)]\s+(.*)$""")
private val FENCE_RE = Regex("""^\s*```(.*)$""")

/**
 * Parse [src] into a flat list of [MdBlock]s. Total over any input: unrecognized
 * structure falls back to [MdBlock.Paragraph]. Consecutive non-blank,
 * non-list, non-fence lines coalesce into one paragraph (joined with newlines);
 * a blank line separates paragraphs; runs of bullet / numbered lines coalesce
 * into a single list block.
 */
fun parseMarkdownBlocks(src: String): List<MdBlock> {
    val lines = src.split("\n")
    val blocks = mutableListOf<MdBlock>()

    val paragraph = StringBuilder()
    fun flushParagraph() {
        if (paragraph.isNotEmpty()) {
            blocks += MdBlock.Paragraph(parseInline(paragraph.toString()))
            paragraph.setLength(0)
        }
    }

    var i = 0
    while (i < lines.size) {
        val line = lines[i]
        val fence = FENCE_RE.matchEntire(line)

        when {
            // --- fenced code block --------------------------------------
            fence != null -> {
                flushParagraph()
                val language = fence.groupValues[1].trim()
                val code = StringBuilder()
                i++
                // Consume until the closing fence (or EOF — unterminated is fine).
                while (i < lines.size && FENCE_RE.matchEntire(lines[i]) == null) {
                    if (code.isNotEmpty()) code.append("\n")
                    code.append(lines[i])
                    i++
                }
                if (i < lines.size) i++ // skip the closing ```
                blocks += MdBlock.CodeBlock(language = language, code = code.toString())
            }

            i + 1 < lines.size && tableAlignment(lines[i + 1]) != null &&
                line.contains('|') && tableCells(line).size == tableAlignment(lines[i + 1])!!.size -> {
                flushParagraph()
                val header = tableCells(line).map(::parseInline)
                val alignment = tableAlignment(lines[i + 1])!!
                val rows = mutableListOf<List<List<MdInline>>>()
                i += 2
                while (i < lines.size && lines[i].isNotBlank() && lines[i].contains('|')) {
                    val cells = tableCells(lines[i])
                    rows += List(header.size) { column -> parseInline(cells.getOrElse(column) { "" }) }
                    i++
                }
                blocks += MdBlock.Table(header, rows, alignment)
            }
            HEADING_RE.matchEntire(line) != null -> {
                flushParagraph()
                val heading = HEADING_RE.matchEntire(line)!!
                blocks += MdBlock.Heading(heading.groupValues[1].length, parseInline(heading.groupValues[2]))
                i++
            }
            QUOTE_RE.matchEntire(line) != null -> {
                flushParagraph()
                val quote = mutableListOf<String>()
                while (i < lines.size) {
                    val match = QUOTE_RE.matchEntire(lines[i]) ?: break
                    quote += match.groupValues[1]
                    i++
                }
                blocks += MdBlock.Quote(parseInline(quote.joinToString("\n")))
            }

            // --- bullet list --------------------------------------------
            BULLET_RE.matchEntire(line) != null -> {
                flushParagraph()
                val items = mutableListOf<List<MdInline>>()
                while (i < lines.size) {
                    val m = BULLET_RE.matchEntire(lines[i]) ?: break
                    items += parseInline(m.groupValues[1])
                    i++
                }
                blocks += MdBlock.BulletList(items)
            }

            // --- numbered list ------------------------------------------
            NUMBERED_RE.matchEntire(line) != null -> {
                flushParagraph()
                val items = mutableListOf<NumberedItem>()
                while (i < lines.size) {
                    val m = NUMBERED_RE.matchEntire(lines[i]) ?: break
                    items += NumberedItem(
                        marker = m.groupValues[1] + ".",
                        spans = parseInline(m.groupValues[2]),
                    )
                    i++
                }
                blocks += MdBlock.NumberedList(items)
            }

            // --- blank line: paragraph separator ------------------------
            line.isBlank() -> {
                flushParagraph()
                i++
            }

            // --- paragraph text -----------------------------------------
            else -> {
                if (paragraph.isNotEmpty()) paragraph.append("\n")
                paragraph.append(line)
                i++
            }
        }
    }
    flushParagraph()
    return blocks
}

/**
 * Parse one line/run of inline Markdown into [MdInline] spans. Handles
 * `` `code` `` and `**bold**`; inline code takes precedence (its content is
 * literal, so `**` inside backticks is NOT bold). Unterminated markers degrade
 * to literal text.
 */
fun parseInline(text: String): List<MdInline> {
    val out = mutableListOf<MdInline>()
    val plain = StringBuilder()
    fun flushPlain() {
        if (plain.isNotEmpty()) {
            out += MdInline.Text(plain.toString())
            plain.setLength(0)
        }
    }

    var i = 0
    while (i < text.length) {
        val c = text[i]
        when {
            // markdown link — keep parsing deliberately shallow and total
            c == '[' -> {
                val labelEnd = text.indexOf(']', i + 1)
                val urlStart = labelEnd.takeIf { it >= 0 }
                    ?.takeIf { it + 1 < text.length && text[it + 1] == '(' }
                    ?.plus(2)
                val urlEnd = urlStart?.let { text.indexOf(')', it) } ?: -1
                if (labelEnd < 0 || urlStart == null || urlEnd < 0) {
                    plain.append(c)
                    i++
                } else {
                    flushPlain()
                    out += MdInline.Link(
                        label = text.substring(i + 1, labelEnd),
                        url = text.substring(urlStart, urlEnd),
                    )
                    i = urlEnd + 1
                }
            }

            // inline code — literal until the next backtick
            c == '`' -> {
                val close = text.indexOf('`', i + 1)
                if (close < 0) {
                    plain.append(text.substring(i)); break
                }
                flushPlain()
                out += MdInline.Code(text.substring(i + 1, close))
                i = close + 1
            }

            c == '\\' && i + 1 < text.length -> { plain.append(text[i + 1]); i += 2 }
            c == '*' || c == '_' || (c == '~' && text.getOrNull(i + 1) == '~') -> {
                val count = text.substring(i).takeWhile { it == c }.length
                val width = if (c == '~') 2 else count.coerceAtMost(3)
                val intraword = c == '_' && i > 0 && text[i - 1].isLetterOrDigit()
                val close = if (intraword || text.getOrNull(i + width)?.isWhitespace() != false) -1
                    else closingEmphasis(text, i + width, c, width)
                if (close < 0) { plain.append(text.substring(i, i + width)); i += width }
                else {
                    flushPlain()
                    val body = text.substring(i + width, close)
                    out += when {
                        c == '~' -> MdInline.Strike(body)
                        width == 3 -> MdInline.BoldItalic(body)
                        width == 2 -> MdInline.Bold(body)
                        else -> MdInline.Italic(body)
                    }
                    i = close + width
                }
            }

            else -> {
                plain.append(c); i++
            }
        }
    }
    flushPlain()
    // An empty run still yields one empty Text so callers never face an empty list.
    return if (out.isEmpty()) listOf(MdInline.Text("")) else out
}

/** Skip escaped delimiters and code spans, preserving nested marker runs. */
private fun closingEmphasis(text: String, start: Int, marker: Char, width: Int): Int {
    var index = start
    while (index < text.length) {
        if (text[index] == '\\') { index += 2; continue }
        if (text[index] == '`') {
            val end = text.indexOf('`', index + 1)
            if (end >= 0) { index = end + 1; continue }
        }
        if (text[index] == marker) {
            var end = index
            while (end < text.length && text[end] == marker) end++
            val length = end - index
            if (length >= width && (width != 1 || length % 2 == 1) &&
                index > start && !text[index - 1].isWhitespace()) return end - width
            index = end
        } else index++
    }
    return -1
}

internal fun tableCells(line: String): List<String> {
    val trimmed = line.trim().removePrefix("|").removeSuffix("|")
    val cells = mutableListOf<String>()
    val cell = StringBuilder()
    var code = false
    var index = 0
    while (index < trimmed.length) {
        val char = trimmed[index]
        if (char == '\\' && trimmed.getOrNull(index + 1) == '|') {
            cell.append('|'); index += 2; continue
        }
        if (char == '`') code = !code
        if (char == '|' && !code) { cells += cell.toString().trim(); cell.clear() }
        else cell.append(char)
        index++
    }
    cells += cell.toString().trim()
    return cells
}

private fun tableAlignment(line: String): List<TableAlignment>? {
    val cells = tableCells(line)
    if (cells.isEmpty() || cells.any { !it.matches(Regex(":?-{3,}:?")) }) return null
    return cells.map { when {
        it.startsWith(':') && it.endsWith(':') -> TableAlignment.Center
        it.endsWith(':') -> TableAlignment.Right
        else -> TableAlignment.Left
    } }
}
