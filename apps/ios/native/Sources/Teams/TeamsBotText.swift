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
        default: "chat_status_unknown"
        }
    }

    /// Only a shared Bot writes Markdown; what a person types is shown exactly as typed, and a recalled message
    /// is replaced by its notice.
    static func rendersMarkdown(authorKind: String, recalled: Bool) -> Bool {
        authorKind == "shared_bot" && !recalled
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

    static let allKeys = ["cron_status_queued", "teams_run_processing", "chat_status_completed", "chat_status_failed", "teams_task_expired", "teams_task_interrupted", "teams_task_authorization_required", "teams_run_revoked", "chat_status_unknown"]
}
