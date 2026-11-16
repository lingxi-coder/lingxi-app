import Foundation

enum TeamsBotText {
    /// How one state of a Bot task reads and looks on the run card (docs/TEAMS-UI-POLISH.md U4): the words, the glyph and the tone.
    /// The glyph is decorative and the label is the accessible state, so the state is never colour alone.
    struct RunState: Equatable {
        /// Only Completed is `ok` (green) and only Failed `danger` (red); every other state is `muted`.
        enum Tone: Equatable { case ok, danger, muted }
        /// The state as a word: `teams_run_*`, `teams_task_*` (waiting, cancelled, needs checking), `chat_status_completed`,
        /// `chat_status_failed` or `chat_status_unknown`.
        let labelKey: String
        /// What to do or what is going on, one sentence. `nil` for Completed (the card shows the usage line instead), for a state this
        /// app does not know and for the three states whose lines the card writes itself (Waiting, Cancelled, Needs checking).
        let detailKey: String?
        /// An SF Symbol. Processing shows a spinner instead (`spins`), and this symbol under Reduce Motion.
        let symbol: String
        let tone: Tone
        /// A task that is still going or whose outcome is still open (Queued, Processing, Authorization required, Waiting, Needs
        /// checking) offers "Refresh status". The others have no action of their own: their detail says what to do.
        let active: Bool
        /// Only Processing: a spinner moves where the glyph would be.
        var spins = false
    }

    static func runState(_ state: String) -> RunState {
        switch state {
        case "queued": RunState(labelKey: "teams_run_queued", detailKey: "teams_run_queued_detail", symbol: "clock", tone: .muted, active: true)
        case "running": RunState(labelKey: "teams_run_processing", detailKey: "teams_run_processing_detail", symbol: "ellipsis.circle", tone: .muted, active: true, spins: true)
        case "completed": RunState(labelKey: "chat_status_completed", detailKey: nil, symbol: "checkmark.circle.fill", tone: .ok, active: false)
        case "failed": RunState(labelKey: "chat_status_failed", detailKey: "teams_run_failed_detail", symbol: "exclamationmark.triangle.fill", tone: .danger, active: false)
        case "expired": RunState(labelKey: "teams_run_expired", detailKey: "teams_run_expired_detail", symbol: "clock.badge.xmark", tone: .muted, active: false)
        case "interrupted": RunState(labelKey: "teams_run_interrupted", detailKey: "teams_run_interrupted_detail", symbol: "pause.circle", tone: .muted, active: false)
        case "authorization_required": RunState(labelKey: "teams_run_authorization_required", detailKey: "teams_run_authorization_required_detail", symbol: "lock.circle", tone: .muted, active: true)
        case "revoked": RunState(labelKey: "teams_run_revoked", detailKey: "teams_run_revoked_detail", symbol: "nosign", tone: .muted, active: false)
        // The three states of Teams M2/M3. They have no detail of their own here: the card says what it waits for, or why the outcome
        // is unknown, in the lines that know the person (`teams_task_waiting_you`, `teams_task_needs_checking_*`).
        case "waiting": RunState(labelKey: "teams_task_waiting", detailKey: nil, symbol: "ellipsis.bubble", tone: .muted, active: true)
        case "cancelled": RunState(labelKey: "teams_task_cancelled", detailKey: nil, symbol: "xmark.circle", tone: .muted, active: false)
        case "needs_reconciliation": RunState(labelKey: "teams_task_needs_checking", detailKey: nil, symbol: "questionmark.circle", tone: .muted, active: true)
        default: RunState(labelKey: "chat_status_unknown", detailKey: nil, symbol: "questionmark.circle", tone: .muted, active: false)
        }
    }

    static func stateKey(_ state: String) -> String { runState(state).labelKey }

    /// Only a shared Bot writes Markdown; what a person types is shown exactly as typed, and a recalled message
    /// is replaced by its notice.
    static func rendersMarkdown(authorKind: String, recalled: Bool) -> Bool {
        authorKind == "shared_bot" && !recalled
    }

    /// A Bot's Markdown as one plain line, for the conversation list (the stored preview is a truncated copy, so it
    /// may stop inside a marker). Line by line: block markers (heading, quote, list with its task box), code fences,
    /// table separators and rules go; the pipes of a table row (one that starts or ends with a pipe) become spaces; links and images keep their text; `**`, `__`, `~~`
    /// and backticks go; a single `*` or `_` goes only as a pair around text, never inside a word or around spaces.
    /// The lines join with one space. Android's `TeamsBotText.plainText` and Desktop's share these rules and vectors.
    static func plainText(_ markdown: String) -> String {
        func matches(_ line: String, _ pattern: String) -> Bool { line.range(of: pattern, options: .regularExpression) != nil }
        func replacing(_ text: String, _ pattern: String, with template: String) -> String {
            text.replacingOccurrences(of: pattern, with: template, options: .regularExpression)
        }
        let lines = markdown.components(separatedBy: .newlines).filter {
            !matches($0, #"^\s*(```|~~~)"#) && !matches($0, #"^\s*([-*_])(\s*\1){2,}\s*$"#)
                && !matches($0, #"^(?=.*\|)\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$"#)
        }
        let plain = lines.map { line -> String in
            var text = replacing(line, #"^(\s*>)+\s?"#, with: "")
            text = replacing(text, #"^\s*#{1,6}(\s+|$)"#, with: "")
            text = replacing(text, #"^\s*([-*+]|\d+[.)])(\s+|$)(\[[ xX]\](\s+|$))?"#, with: "")
            text = replacing(text, #"!?\[([^\]]*)\]\([^)]*\)"#, with: "$1")
            if matches(text, #"^\s*\||\|\s*$"#) { text = text.replacingOccurrences(of: "|", with: " ") }
            for mark in ["**", "__", "~~", "`"] { text = text.replacingOccurrences(of: mark, with: "") }
            text = replacing(text, #"(?<![A-Za-z0-9_*])\*(?=\S)([^*]*?\S)\*(?![A-Za-z0-9_*])"#, with: "$1")
            return replacing(text, #"(?<![A-Za-z0-9_])_(?=\S)([^_]*?\S)_(?![A-Za-z0-9_])"#, with: "$1")
        }
        return replacing(plain.joined(separator: " "), #"\s+"#, with: " ").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// The words and the number format of the usage line (U5). `english` is the fixed set the vectors use; `current` (in
    /// `TeamsStrings.swift`) is the UI language's.
    struct UsageStrings {
        var missing: String, partial: String, costUnknown: String, currencyUnknown: String
        /// The total of input and output tokens in the compact form of `locale` (`1.2K`), as "1.2K tokens".
        var tokensTotal: (_ total: String) -> String
        /// The split, "123 / 456 tokens": the accessibility value of the line.
        var tokensSplit: (_ input: Int, _ output: Int) -> String
        var locale: Locale
        static let english = UsageStrings(
            missing: "Usage unavailable", partial: "Partial usage", costUnknown: "cost unknown", currencyUnknown: "currency unknown",
            tokensTotal: { "\($0) tokens" }, tokensSplit: { "\($0) / \($1) tokens" }, locale: Locale(identifier: "en_US"))
    }

    /// The smallest cost the line shows; a positive cost below it reads "<" and this amount.
    static let smallestCost = Decimal(string: "0.0001")!

    /// "1.2K tokens · $0.0012". Tokens are the total of input and output in the locale's compact number; the cost is in the task's
    /// currency with two to four decimals. A missing or absent usage reads "Usage unavailable"; a partial one puts "Partial usage" in
    /// front; a complete one has no prefix. A cost is never invented: no `costMicros` says "cost unknown", and a cost without a
    /// currency says "currency unknown" after the amount. Android's `TeamsBotText.usageLine` and Desktop's `usageText` share the rules.
    static func usageLine(_ usage: TeamTaskUsage?, strings: UsageStrings) -> String {
        guard let usage, usage.status != "missing" else { return strings.missing }
        let total = (usage.inputTokens ?? 0) + (usage.outputTokens ?? 0)
        var parts: [String] = []
        if usage.status == "partial" { parts.append(strings.partial) }
        parts.append(strings.tokensTotal(total.formatted(.number.notation(.compactName).locale(strings.locale))))
        parts.append(cost(usage, strings: strings))
        return parts.joined(separator: " · ")
    }

    /// "123 / 456 tokens", or `nil` when the usage carries no token counts (missing or absent).
    static func tokenSplit(_ usage: TeamTaskUsage?, strings: UsageStrings) -> String? {
        guard let usage, usage.status != "missing" else { return nil }
        return strings.tokensSplit(usage.inputTokens ?? 0, usage.outputTokens ?? 0)
    }

    private static func cost(_ usage: TeamTaskUsage, strings: UsageStrings) -> String {
        guard let micros = usage.costMicros else { return strings.costUnknown }
        // Whole millionths are exact in a Decimal, so no binary noise reaches the digits.
        let amount = Decimal(micros) / 1_000_000
        let digits = 2...4
        func format(_ value: Decimal) -> String {
            if let code = usage.currency, !code.isEmpty {
                return value.formatted(.currency(code: code.uppercased()).precision(.fractionLength(digits)).locale(strings.locale))
            }
            return value.formatted(.number.precision(.fractionLength(digits)).locale(strings.locale)) + " " + strings.currencyUnknown
        }
        if amount > 0 && amount < smallestCost { return "<" + format(smallestCost) }
        return format(amount)
    }

    static let allKeys = ["teams_run_queued", "teams_run_processing", "chat_status_completed", "chat_status_failed", "teams_run_expired", "teams_run_interrupted", "teams_run_authorization_required", "teams_run_revoked", "teams_task_waiting", "teams_task_cancelled", "teams_task_needs_checking", "chat_status_unknown"]
    /// The one-sentence details of the states that have one.
    static let allDetailKeys = ["teams_run_queued_detail", "teams_run_processing_detail", "teams_run_failed_detail", "teams_run_expired_detail", "teams_run_interrupted_detail", "teams_run_authorization_required_detail", "teams_run_revoked_detail"]
}
