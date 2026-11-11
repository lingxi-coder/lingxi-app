package com.lingxi.code.teams

import java.util.Locale

/** The words of a task's usage line (Desktop `usageText`). */
internal data class TeamUsageStrings(
    val missing: String, val partial: String, val complete: String, val costUnknown: String, val currencyUnknown: String,
    val tokens: (input: Long, output: Long) -> String,
) {
    companion object {
        val english = TeamUsageStrings("Missing usage \u00b7 cost unknown", "Partial usage", "Complete usage", "cost unknown", "currency unknown") { i, o -> "$i / $o tokens" }
    }
}

internal object TeamsBotText {
    /** "Complete usage \u00b7 123 / 456 tokens \u00b7 0.001322 USD". Missing usage never invents a cost: no `costMicros` says "cost unknown", a cost without a currency says "currency unknown". */
    fun usageLine(usage: TeamTaskUsage?, strings: TeamUsageStrings): String {
        if (usage == null || usage.status == "missing") return strings.missing
        val prefix = if (usage.status == "partial") strings.partial else strings.complete
        val cost = usage.costMicros?.let { String.format(Locale.ROOT, "%.6f", it / 1_000_000) + " " + (usage.currency ?: strings.currencyUnknown) } ?: strings.costUnknown
        return listOf(prefix, strings.tokens(usage.inputTokens, usage.outputTokens), cost).joinToString(" \u00b7 ")
    }

    /** Only a shared Bot writes Markdown; what a person types is shown exactly as typed, and a recalled message is replaced by its notice. */
    fun rendersMarkdown(authorKind: String, recalled: Boolean): Boolean = authorKind == "shared_bot" && !recalled

    /**
     * A Bot's Markdown as one plain line, for the conversation list (the stored preview is a truncated copy, so it may stop inside a
     * marker). Line by line: block markers (heading, quote, list with its task box), code fences, table separators and rules go; the pipes of a table row
     * (one that starts or ends with a pipe) become spaces; links and images keep their text; `**`, `__`, `~~` and backticks go; a single `*` or `_` goes only as a pair
     * around text, never inside a word or around spaces. The lines join with one space.
     */
    fun plainText(markdown: String): String =
        markdown.lines().filterNot { FENCE.containsMatchIn(it) || RULE.matches(it) || TABLE_SEPARATOR.matches(it) }
            .joinToString(" ") { line ->
                var text = line.replace(QUOTE, "").replace(HEADING, "").replace(LIST_MARKER, "")
                text = text.replace(LINK_OR_IMAGE, "$1")
                if (TABLE_ROW.containsMatchIn(text)) text = text.replace('|', ' ')
                for (mark in listOf("**", "__", "~~", "`")) text = text.replace(mark, "")
                text.replace(SINGLE_STAR, "$1").replace(SINGLE_UNDERSCORE, "$1")
            }.replace(WHITESPACE, " ").trim()

    private val FENCE = Regex("""^\s*(```|~~~)""")
    private val RULE = Regex("""^\s*([-*_])(\s*\1){2,}\s*$""")
    private val TABLE_SEPARATOR = Regex("""^(?=.*\|)\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$""")
    private val TABLE_ROW = Regex("""^\s*\||\|\s*$""")
    private val QUOTE = Regex("""^(\s*>)+\s?""")
    private val HEADING = Regex("""^\s*#{1,6}(\s+|$)""")
    private val LIST_MARKER = Regex("""^\s*([-*+]|\d+[.)])(\s+|$)(\[[ xX]](\s+|$))?""")
    private val LINK_OR_IMAGE = Regex("""!?\[([^\]]*)]\([^)]*\)""")
    private val SINGLE_STAR = Regex("""(?<![A-Za-z0-9_*])\*(?=\S)([^*]*?\S)\*(?![A-Za-z0-9_*])""")
    private val SINGLE_UNDERSCORE = Regex("""(?<![A-Za-z0-9_])_(?=\S)([^_]*?\S)_(?![A-Za-z0-9_])""")
    private val WHITESPACE = Regex("""(?U)\s+""")

    /** Links in Bot output may only leave the app as web pages. */
    fun allowsLink(link: String): Boolean = runCatching {
        val uri = java.net.URI(link.trim())
        uri.scheme?.lowercase() in setOf("http", "https") && !uri.host.isNullOrEmpty()
    }.getOrDefault(false)
}
