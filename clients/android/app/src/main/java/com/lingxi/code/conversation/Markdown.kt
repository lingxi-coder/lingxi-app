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
 * Non-goals: headings, blockquotes, tables, links, nested lists, emphasis with
 * `_underscores_`. Anything unrecognized degrades to literal paragraph text, so
 * the renderer is never wrong — only less rich.
 */

/** A block-level element. The document is a flat list of these. */
sealed interface MdBlock {
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
sealed interface MdInline {
    /** Plain text. */
    data class Text(val text: String) : MdInline

    /** `**bold**` text. */
    data class Bold(val text: String) : MdInline

    /** `` `inline code` ``. */
    data class Code(val text: String) : MdInline
}

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

            // bold — **…**
            c == '*' && i + 1 < text.length && text[i + 1] == '*' -> {
                val close = text.indexOf("**", i + 2)
                if (close < 0) {
                    plain.append(text.substring(i)); break
                }
                flushPlain()
                out += MdInline.Bold(text.substring(i + 2, close))
                i = close + 2
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
