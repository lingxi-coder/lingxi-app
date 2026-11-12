import SwiftUI

// The view layer's words for the pure Teams rules: the label / preview vocabulary in the UI language, and
// the dates of list rows and day separators. The rules themselves (`TeamsListLogic`, `TeamsStreamLogic`)
// stay Foundation-only and take everything language-dependent as a parameter.

extension TeamsBotText.UsageStrings {
    static var current: TeamsBotText.UsageStrings {
        TeamsBotText.UsageStrings(
            missing: String(localized: "teams_usage_missing"), partial: String(localized: "teams_usage_partial"),
            complete: String(localized: "teams_usage_complete"), costUnknown: String(localized: "teams_usage_cost_unknown"),
            currencyUnknown: String(localized: "teams_usage_currency_unknown"),
            tokens: { String(localized: "teams_usage_tokens \($0) \($1)") })
    }
}

extension TeamsBotText {
    /// "in 4 minutes" in the UI language, for a wait's expiry.
    static func expiryText(at: Double, now: Double) -> String {
        let expiry = TeamTaskActions.expiry(at: at, now: now)
        var components = DateComponents()
        switch expiry.unit {
        case .minutes: components.minute = expiry.value
        case .hours: components.hour = expiry.value
        case .days: components.day = expiry.value
        }
        let formatter = RelativeDateTimeFormatter()
        let language = Bundle._lxCurrentLanguageID.withLock { $0 }
        if !language.isEmpty { formatter.locale = Locale(identifier: language) }
        formatter.unitsStyle = .full
        return formatter.localizedString(from: components)
    }
}

extension TeamsListStrings {
    /// The vocabulary of labels and previews in the UI language. The separators are code, not resources:
    /// `、` and `：` when the UI language is Chinese, `, ` and `: ` otherwise (spec §6.1).
    static var current: TeamsListStrings {
        let chinese = isChinese(languageTag: Bundle._lxCurrentLanguageID.withLock { $0 })
        return TeamsListStrings(
            channel: String(localized: "teams_conversation_channel"),
            directMessage: String(localized: "teams_label_direct"),
            group: String(localized: "teams_conversation_group"),
            member: String(localized: "teams_member"),
            you: String(localized: "teams_preview_you"),
            bot: "Bot",
            noMessages: String(localized: "teams_preview_none"),
            recalled: String(localized: "teams_message_recalled"),
            nonText: String(localized: "teams_preview_non_text"),
            listSeparator: chinese ? "、" : ", ",
            authorSeparator: chinese ? "：" : ": ")
    }
}

enum TeamsTimeText {
    /// The compact time of a list row (`TeamsStreamLogic.listTimeKind`): the clock today, "Yesterday", a weekday,
    /// then the date. Blank when the conversation has no usable time.
    static func listTime(atMs: Int64?, now: Date = Date(), zone: TimeZone = .current, locale: Locale) -> String {
        let nowMs = Int64(now.timeIntervalSince1970 * 1000)
        switch TeamsStreamLogic.listTimeKind(atMs: atMs, nowMs: nowMs, zone: zone) {
        case .none: return ""
        case .clock: return TeamsStreamLogic.formatClock(atMs: atMs, zone: zone, locale: locale)
        case .yesterday: return String(localized: "session_relative_yesterday")
        case .weekday: return format(atMs, zone, locale) { $0.weekday(.abbreviated) }
        case .dateThisYear: return format(atMs, zone, locale) { $0.month(.abbreviated).day() }
        case .dateOtherYear: return format(atMs, zone, locale) { $0.year().month(.abbreviated).day() }
        }
    }

    /// The label of a day separator in the stream.
    static func dayLabel(_ kind: DayKind, atMs: Int64, zone: TimeZone = .current, locale: Locale) -> String {
        switch kind {
        case .today: String(localized: "drawer_recents_today")
        case .yesterday: String(localized: "session_relative_yesterday")
        case .dateThisYear: format(atMs, zone, locale) { $0.weekday(.abbreviated).month(.abbreviated).day() }
        case .dateOtherYear: format(atMs, zone, locale) { $0.year().month(.abbreviated).day() }
        }
    }

    private static func format(_ atMs: Int64?, _ zone: TimeZone, _ locale: Locale, _ build: (Date.FormatStyle) -> Date.FormatStyle) -> String {
        guard let atMs else { return "" }
        let style = build(Date.FormatStyle(date: .omitted, time: .omitted, locale: locale, timeZone: zone))
        return Date(timeIntervalSince1970: Double(atMs) / 1000).formatted(style)
    }
}

/// Small text helpers shared by the Teams views.
enum TeamsText {
    /// `99+` above 99, nothing for no unread messages (the row and section badges).
    static func badge(_ count: Int) -> String? { TabLogic.tabBadgeText(count) }

    static func membersCount(_ count: Int) -> String { String(localized: "teams_members_count \(count)") }

    static func botsCount(_ count: Int) -> String { String(localized: "teams_bots_count \(count)") }

    /// The role label of a team member: `teams_owner`, `teams_admin` or `teams_member`.
    static func role(_ role: String) -> LocalizedStringKey { LocalizedStringKey("teams_" + role) }
}
