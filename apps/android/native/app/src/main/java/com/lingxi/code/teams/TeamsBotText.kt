package com.lingxi.code.teams

import androidx.annotation.StringRes
import com.lingxi.code.R
import java.math.BigDecimal
import java.math.RoundingMode
import java.text.NumberFormat
import java.util.Currency
import java.util.Locale

/**
 * The words and the number formats of a run's usage line (docs/TEAMS-UI-POLISH.md U5). The screen builds the real ones from the UI language
 * (`rememberTeamUsageStrings`); [english] is the fixed set the test vectors use, because ICU is not available to a plain JVM test.
 */
internal data class TeamUsageStrings(
    val missing: String, val partial: String, val costUnknown: String, val currencyUnknown: String,
    /** The total of input and output tokens in the locale's compact form, as "1.2K tokens". */
    val tokensTotal: (total: Long) -> String,
    /** The split, "123 / 456 tokens": the spoken value of the line. */
    val tokensSplit: (input: Long, output: Long) -> String,
    /** [amount] in [currency] with two to four decimals ("$0.0012"); a null [currency] is the bare number. */
    val money: (amount: BigDecimal, currency: String?) -> String,
) {
    companion object {
        val english = TeamUsageStrings(
            "Usage unavailable", "Partial usage", "cost unknown", "currency unknown",
            tokensTotal = { total -> "${compact(total)} tokens" },
            tokensSplit = { input, output -> "$input / $output tokens" },
            money = { amount, currency ->
                val number = amount.setScale(maxOf(2, minOf(4, amount.stripTrailingZeros().scale())), RoundingMode.HALF_EVEN).toPlainString()
                when (currency) { null -> number; "USD" -> "$$number"; else -> "$number $currency" }
            },
        )

        /** "1.2K": thousands and millions to one decimal, the way the compact format of en reads. */
        private fun compact(total: Long): String = when {
            total < 1_000 -> total.toString()
            total < 1_000_000 -> oneDecimal(total / 1_000.0) + "K"
            else -> oneDecimal(total / 1_000_000.0) + "M"
        }
        private fun oneDecimal(value: Double): String = java.lang.String.format(Locale.ROOT, "%.1f", value).removeSuffix(".0")
    }
}

/** The glyph of a run state; the screen picks the icon (Material `Schedule`, `CheckCircle`, ...) and shows a spinner for [Spinner] unless motion is reduced. */
internal enum class RunGlyph { Clock, Spinner, Check, Alert, Expired, Paused, Locked, Blocked, Waiting, Cancelled, Checking, Unknown }

/** Only Completed is [Ok] and only Failed [Danger]; every other state is [Muted]. */
internal enum class RunTone { Ok, Danger, Muted }

/** How one state of a Bot task reads and looks on the run card (U4). The glyph is decorative; the [label] is the accessible state, so the state is never colour alone. */
internal data class TeamRunState(
    @StringRes val label: Int,
    /**
     * What to do or what is going on, one sentence; null for Completed (the card shows the usage line), for a state this app does not know, and for Waiting,
     * Cancelled and Needs checking, whose line the card writes itself (who the task waits for, what to check, what the run used).
     */
    @StringRes val detail: Int?,
    val glyph: RunGlyph,
    val tone: RunTone,
    /** A task that is still going or still waits for someone (Queued, Processing, Waiting, Authorization required, Needs checking) offers "Refresh status"; the others say what to do in their detail. */
    val active: Boolean,
)

internal object TeamsBotText {
    fun runState(state: String): TeamRunState = when (state) {
        "queued" -> TeamRunState(R.string.teams_run_queued, R.string.teams_run_queued_detail, RunGlyph.Clock, RunTone.Muted, active = true)
        "running" -> TeamRunState(R.string.teams_run_processing, R.string.teams_run_processing_detail, RunGlyph.Spinner, RunTone.Muted, active = true)
        "completed" -> TeamRunState(R.string.chat_status_completed, null, RunGlyph.Check, RunTone.Ok, active = false)
        "failed" -> TeamRunState(R.string.chat_status_failed, R.string.teams_run_failed_detail, RunGlyph.Alert, RunTone.Danger, active = false)
        "expired" -> TeamRunState(R.string.teams_run_expired, R.string.teams_run_expired_detail, RunGlyph.Expired, RunTone.Muted, active = false)
        "interrupted" -> TeamRunState(R.string.teams_run_interrupted, R.string.teams_run_interrupted_detail, RunGlyph.Paused, RunTone.Muted, active = false)
        "authorization_required" -> TeamRunState(R.string.teams_run_authorization_required, R.string.teams_run_authorization_required_detail, RunGlyph.Locked, RunTone.Muted, active = true)
        "revoked" -> TeamRunState(R.string.teams_run_revoked, R.string.teams_run_revoked_detail, RunGlyph.Blocked, RunTone.Muted, active = false)
        // The states Teams M2 added; they keep their own words, are muted like the other open states, and the card writes their detail line.
        "waiting" -> TeamRunState(R.string.teams_task_waiting, null, RunGlyph.Waiting, RunTone.Muted, active = true)
        "cancelled" -> TeamRunState(R.string.teams_task_cancelled, null, RunGlyph.Cancelled, RunTone.Muted, active = false)
        "needs_reconciliation" -> TeamRunState(R.string.teams_task_needs_checking, null, RunGlyph.Checking, RunTone.Muted, active = true)
        // A Bot nobody asked that chose not to answer: nothing to wait for or act on.
        "declined" -> TeamRunState(R.string.teams_task_declined, null, RunGlyph.Unknown, RunTone.Muted, active = false)
        else -> TeamRunState(R.string.chat_status_unknown, null, RunGlyph.Unknown, RunTone.Muted, active = false)
    }

    /** The word for [task]'s state: "Cancelling" while a cancel is on its way, "Waiting for approval" when it waits on a tool approval, else the state's own [TeamRunState.label]. */
    @StringRes
    fun stateLabel(task: TeamTask): Int = when {
        task.cancelling -> R.string.teams_task_cancelling
        task.state == "waiting" && task.wait?.kind == "tool_approval" -> R.string.teams_task_waiting_approval
        else -> runState(task.state).label
    }

    /**
     * [amount] with two to four decimals: in the currency [code] names when it is a real currency code (`$0.0012`), else the bare number, followed by the
     * code as sent when there is one. The real [TeamUsageStrings.money].
     */
    fun money(amount: BigDecimal, code: String?, locale: Locale): String {
        val currency = code?.let { runCatching { Currency.getInstance(it) }.getOrNull() }
        val format = if (currency != null) NumberFormat.getCurrencyInstance(locale).apply { this.currency = currency } else NumberFormat.getNumberInstance(locale)
        format.minimumFractionDigits = 2
        format.maximumFractionDigits = 4
        val text = format.format(amount)
        return if (code != null && currency == null) "$text $code" else text
    }

    /** The smallest cost the line shows; a positive cost below it reads "<" and this amount. */
    private val SMALLEST_COST = BigDecimal("0.0001")

    /**
     * "1.2K tokens \u00b7 $0.0012". Tokens are the total of input and output in the locale's compact number; the cost is in the task's currency with two to four
     * decimals. A missing or absent usage reads "Usage unavailable"; a partial one puts "Partial usage" in front; a complete one has no prefix. A cost is never
     * invented: no `costMicros` says "cost unknown", and a cost without a currency says "currency unknown" after the amount.
     */
    fun usageLine(usage: TeamTaskUsage?, strings: TeamUsageStrings): String {
        if (usage == null || usage.status == "missing") return strings.missing
        val parts = buildList {
            if (usage.status == "partial") add(strings.partial)
            add(strings.tokensTotal(usage.inputTokens + usage.outputTokens))
            add(cost(usage, strings))
        }
        return parts.joinToString(SEPARATOR)
    }

    /** "123 / 456 tokens", the spoken value of the usage line; null when there is no usage to speak of (missing or absent). */
    fun tokenSplit(usage: TeamTaskUsage?, strings: TeamUsageStrings): String? =
        if (usage == null || usage.status == "missing") null else strings.tokensSplit(usage.inputTokens, usage.outputTokens)

    private fun cost(usage: TeamTaskUsage, strings: TeamUsageStrings): String {
        val micros = usage.costMicros?.takeIf { it.isFinite() } ?: return strings.costUnknown
        val amount = BigDecimal.valueOf(micros).movePointLeft(6)
        val currency = usage.currency?.takeIf { it.isNotBlank() }
        fun format(value: BigDecimal) = strings.money(value, currency?.uppercase()) + if (currency == null) " " + strings.currencyUnknown else ""
        return if (amount.signum() > 0 && amount < SMALLEST_COST) "<" + format(SMALLEST_COST) else format(amount)
    }

    private const val SEPARATOR = " \u00b7 "

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
    // Unicode White_Space without Java's (?U) flag, which Android's ICU rejects.
    private val WHITESPACE = Regex("""[\p{Z}\u0009-\u000D\u0085]+""")

    /** Links in Bot output may only leave the app as web pages. */
    fun allowsLink(link: String): Boolean = runCatching {
        val uri = java.net.URI(link.trim())
        uri.scheme?.lowercase() in setOf("http", "https") && !uri.host.isNullOrEmpty()
    }.getOrDefault(false)
}
