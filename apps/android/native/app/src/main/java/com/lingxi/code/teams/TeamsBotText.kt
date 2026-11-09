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

    /** Links in Bot output may only leave the app as web pages. */
    fun allowsLink(link: String): Boolean = runCatching {
        val uri = java.net.URI(link.trim())
        uri.scheme?.lowercase() in setOf("http", "https") && !uri.host.isNullOrEmpty()
    }.getOrDefault(false)
}
