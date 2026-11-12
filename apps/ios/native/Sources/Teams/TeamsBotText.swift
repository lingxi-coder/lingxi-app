import Foundation

enum TeamsBotText {
    static func stateKey(_ state: String) -> String {
        switch state {
        case "queued": "cron_status_queued"
        case "running": "teams_run_processing"
        case "completed": "chat_status_completed"
        case "failed": "chat_status_failed"
        case "expired": "teams_task_expired"
        case "interrupted": "teams_task_interrupted"
        case "authorization_required": "teams_task_authorization_required"
        case "revoked": "teams_run_revoked"
        case "waiting": "teams_task_waiting"
        case "cancelled": "teams_task_cancelled"
        case "needs_reconciliation": "teams_task_needs_checking"
        default: "chat_status_unknown"
        }
    }

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

    /// The words of the usage line (Desktop `usageText`).
    struct UsageStrings {
        var missing: String, partial: String, complete: String, costUnknown: String, currencyUnknown: String
        var tokens: (_ input: Int, _ output: Int) -> String
        static let english = UsageStrings(
            missing: "Missing usage · cost unknown", partial: "Partial usage", complete: "Complete usage",
            costUnknown: "cost unknown", currencyUnknown: "currency unknown",
            tokens: { "\($0) / \($1) tokens" })
    }

    /// "Complete usage · 123 / 456 tokens · 0.001322 USD". A missing or absent usage never invents a cost:
    /// no `costMicros` says "cost unknown", and a cost without a currency says "currency unknown".
    static func usageLine(_ usage: TeamTaskUsage?, strings: UsageStrings) -> String {
        guard let usage, usage.status != "missing" else { return strings.missing }
        let prefix = usage.status == "partial" ? strings.partial : strings.complete
        let cost = usage.costMicros.map { String(format: "%.6f", $0 / 1_000_000) + " " + (usage.currency ?? strings.currencyUnknown) } ?? strings.costUnknown
        return [prefix, strings.tokens(usage.inputTokens ?? 0, usage.outputTokens ?? 0), cost].joined(separator: " · ")
    }

    static let allKeys = ["cron_status_queued", "teams_run_processing", "chat_status_completed", "chat_status_failed", "teams_task_expired", "teams_task_interrupted", "teams_task_authorization_required", "teams_run_revoked", "teams_task_waiting", "teams_task_cancelled", "teams_task_needs_checking", "chat_status_unknown"]
}
