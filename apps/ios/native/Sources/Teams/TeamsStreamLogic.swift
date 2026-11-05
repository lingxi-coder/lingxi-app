import Foundation

// The message stream rules of a conversation (spec §8.2; mirrors `packages/teams/stream.ts`).
// Pure: Foundation only. Time is epoch milliseconds; `now` and the time zone are parameters, so nothing here
// reads the clock or the locale on its own. Runs in the macOS XCTest harness against
// `mobile-team-bot-vectors.json` (`stream`, `visibleStart`, `listTimeKinds`, `dayLabelKinds`). Android has
// the identical contract in `teams/TeamsStreamLogic.kt`.

/// The part of a message the stream rules look at.
struct StreamMessage: Equatable, Sendable {
    let id: String
    let seq: Int
    /// `user` or `bot`.
    let authorKind: String
    let authorUid: String
    let authorBotKey: String?
    /// `nil` = no usable time (a write the server has not stamped yet, or an unparsable one).
    let createdAtMs: Int64?
    let recalled: Bool

    init(id: String, seq: Int, authorKind: String, authorUid: String, authorBotKey: String? = nil, createdAtMs: Int64? = nil, recalled: Bool = false) {
        self.id = id; self.seq = seq; self.authorKind = authorKind; self.authorUid = authorUid
        self.authorBotKey = authorBotKey; self.createdAtMs = createdAtMs; self.recalled = recalled
    }

    init(_ message: TeamMessage) {
        self.init(id: message.id, seq: message.seq, authorKind: message.author.kind, authorUid: message.author.uid,
                  authorBotKey: message.author.kind == "bot" ? message.author.botKey : nil,
                  createdAtMs: message.createdAtMs, recalled: message.recalled == true)
    }
}

extension TeamMessage {
    /// `createdAt` (ISO-8601, as the listener writes it) as epoch milliseconds; `nil` when absent or unparsable.
    var createdAtMs: Int64? { TeamsStreamLogic.parseInstant(createdAt) }
}

/// How a date separator is worded; the UI turns it into text (§8.2).
enum DayKind: Equatable, Sendable { case today, yesterday, dateThisYear, dateOtherYear }

/// How the time in a list row is worded (§8.2).
enum ListTimeKind: Equatable, Sendable { case none, clock, yesterday, weekday, dateThisYear, dateOtherYear }

enum StreamRow: Equatable, Sendable {
    /// A message with a valid time on a different local day than the previous timed message. `key` is `date:YYYY-MM-DD`.
    case date(key: String, atMs: Int64, kind: DayKind)
    /// The "new messages" divider, before the first unread message written by someone else.
    case unread
    /// `grouped` continues the previous message's run: no repeated avatar and name.
    case message(StreamMessage, grouped: Bool)

    var key: String {
        switch self {
        case let .date(key, _, _): key
        case .unread: "unread"
        case let .message(message, _): "message:\(message.id)"
        }
    }
}

enum TeamsStreamLogic {
    /// Messages from one author this close (inclusive) read as one run.
    static let groupWindowMs: Int64 = 5 * 60_000
    /// Messages rendered at once: the newest `windowSize`, more on request.
    static let windowSize = 100
    /// The most the window grows by itself (to include unread messages). A reader can always ask for more.
    static let windowMax = 400
    /// Context shown above the first unread message when the window is widened to include it.
    static let unreadContext = 3

    /// The first unread message written by someone else: where the "new messages" divider goes.
    static func firstUnreadMessage(_ messages: [StreamMessage], lastReadSeq: Int, uid: String) -> StreamMessage? {
        messages.first { $0.seq > lastReadSeq && !($0.authorKind == "user" && $0.authorUid == uid) }
    }

    /// Rows of the message stream: day dividers, one "new messages" divider, and messages flagged `grouped`
    /// when they continue the previous message's run (same author, at most five minutes apart, same day,
    /// no divider between, previous not recalled). Messages without a time never start a day and never group.
    static func buildStreamRows(_ messages: [StreamMessage], lastReadSeq: Int, uid: String, nowMs: Int64, zone: TimeZone) -> [StreamRow] {
        let anchor = firstUnreadMessage(messages, lastReadSeq: lastReadSeq, uid: uid)
        var rows: [StreamRow] = []
        var previous: StreamMessage?
        var lastDay: Int?
        for message in messages {
            let time = message.createdAtMs
            let day = time.map { localDay($0, zone) }
            let startsDay = day != nil && day != lastDay
            if startsDay, let time, let day {
                rows.append(.date(key: "date:" + dayKey(day), atMs: time, kind: dayLabelKind(atMs: time, nowMs: nowMs, zone: zone)))
            }
            let unread = anchor?.id == message.id
            if unread { rows.append(.unread) }
            var grouped = false
            if let previous, !startsDay, !unread, !previous.recalled, sameAuthor(previous, message),
               let time, let before = previous.createdAtMs, abs(time - before) <= groupWindowMs {
                grouped = true
            }
            rows.append(.message(message, grouped: grouped))
            previous = message
            if let day { lastDay = day }
        }
        return rows
    }

    /// Where the rendered window of a conversation starts. The window follows the newest message, so arrivals
    /// never push older ones out of view. By itself it is the newest `size` messages, widened back to the first
    /// unread message (with a little context) but never past `max`; `startId`, the oldest message the reader
    /// asked to see, extends it back without that limit.
    static func visibleStart(_ messages: [StreamMessage], startId: String?, lastReadSeq: Int, uid: String, size: Int = windowSize, max ceiling: Int = windowMax) -> Int {
        var start = Swift.max(0, messages.count - size)
        if let unread = firstUnreadMessage(messages, lastReadSeq: lastReadSeq, uid: uid), let index = messages.firstIndex(where: { $0.id == unread.id }) {
            start = Swift.min(start, Swift.max(0, index - unreadContext))
        }
        start = Swift.max(start, messages.count - ceiling, 0)
        if let startId, let asked = messages.firstIndex(where: { $0.id == startId }) { return Swift.min(start, asked) }
        return start
    }

    /// `today`, `yesterday`, or a date (with the year when it is not this one), by local calendar day in `zone`.
    static func dayLabelKind(atMs: Int64, nowMs: Int64, zone: TimeZone) -> DayKind {
        let day = localDay(atMs, zone), today = localDay(nowMs, zone)
        if day == today { return .today }
        if day == today - 1 { return .yesterday }
        return sameYear(day, today) ? .dateThisYear : .dateOtherYear
    }

    /// The compact time of a list row: clock today, yesterday, a weekday for days 2 to 6 back, then the date.
    static func listTimeKind(atMs: Int64?, nowMs: Int64, zone: TimeZone) -> ListTimeKind {
        guard let atMs else { return .none }
        let day = localDay(atMs, zone), today = localDay(nowMs, zone)
        if day == today { return .clock }
        if day == today - 1 { return .yesterday }
        if day >= today - 6, day < today { return .weekday }
        return sameYear(day, today) ? .dateThisYear : .dateOtherYear
    }

    /// Short time in the given locale and zone; empty when there is no time. Platform formatting, not
    /// vector-tested.
    static func formatClock(atMs: Int64?, zone: TimeZone = .current, locale: Locale = .current) -> String {
        guard let atMs else { return "" }
        let style = Date.FormatStyle(date: .omitted, time: .shortened, locale: locale, timeZone: zone)
        return Date(timeIntervalSince1970: Double(atMs) / 1000).formatted(style)
    }

    /// An ISO-8601 instant (`2026-11-02T10:00:00Z`, optionally with fractional seconds and with `Z` or a
    /// numeric offset) as epoch milliseconds; `nil` for anything else.
    static func parseInstant(_ value: String?) -> Int64? {
        guard let value else { return nil }
        let bytes = Array(value.utf8)
        var index = 0
        func number(_ count: Int) -> Int? {
            guard index + count <= bytes.count else { return nil }
            var result = 0
            for offset in 0..<count {
                let byte = bytes[index + offset]
                guard byte >= 48, byte <= 57 else { return nil }
                result = result * 10 + Int(byte - 48)
            }
            index += count
            return result
        }
        func expect(_ byte: UInt8) -> Bool {
            guard index < bytes.count, bytes[index] == byte else { return false }
            index += 1
            return true
        }
        guard let year = number(4), expect(0x2D), let month = number(2), expect(0x2D), let day = number(2), expect(0x54),
              let hour = number(2), expect(0x3A), let minute = number(2), expect(0x3A), let second = number(2) else { return nil }
        var millis = 0
        if expect(0x2E) {
            var count = 0
            while index < bytes.count, bytes[index] >= 48, bytes[index] <= 57 {
                if count < 3 { millis = millis * 10 + Int(bytes[index] - 48) }
                count += 1
                index += 1
            }
            guard count > 0 else { return nil }
            if count < 3 { for _ in count..<3 { millis *= 10 } }
        }
        var offsetSeconds = 0
        if expect(0x5A) {
            // UTC
        } else if index < bytes.count, bytes[index] == 0x2B || bytes[index] == 0x2D {
            let sign = bytes[index] == 0x2B ? 1 : -1
            index += 1
            guard let offsetHours = number(2) else { return nil }
            _ = expect(0x3A)
            guard let offsetMinutes = number(2), offsetHours <= 23, offsetMinutes <= 59 else { return nil }
            offsetSeconds = sign * (offsetHours * 3600 + offsetMinutes * 60)
        } else {
            return nil
        }
        guard index == bytes.count, (1...12).contains(month), day >= 1, day <= daysIn(month: month, year: year),
              hour <= 23, minute <= 59, second <= 59 else { return nil }
        let seconds = Int64(daysFromCivil(year: year, month: month, day: day)) * 86_400 + Int64(hour * 3600 + minute * 60 + second - offsetSeconds)
        return seconds * 1000 + Int64(millis)
    }

    // MARK: calendar arithmetic (days since 1970-01-01 in the viewer's zone; Howard Hinnant's civil algorithms)

    /// The local calendar day of an instant as a day number.
    private static func localDay(_ ms: Int64, _ zone: TimeZone) -> Int {
        let offsetMs = Int64(zone.secondsFromGMT(for: Date(timeIntervalSince1970: Double(ms) / 1000))) * 1000
        let local = ms + offsetMs
        let quotient = local / 86_400_000
        return Int(local % 86_400_000 < 0 ? quotient - 1 : quotient)
    }

    private static func dayKey(_ day: Int) -> String {
        let civil = civilFromDays(day)
        return String(format: "%04d-%02d-%02d", civil.year, civil.month, civil.day)
    }

    private static func sameYear(_ a: Int, _ b: Int) -> Bool { civilFromDays(a).year == civilFromDays(b).year }

    private static func civilFromDays(_ days: Int) -> (year: Int, month: Int, day: Int) {
        let z = days + 719_468
        let era = (z >= 0 ? z : z - 146_096) / 146_097
        let doe = z - era * 146_097
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100)
        let mp = (5 * doy + 2) / 153
        let day = doy - (153 * mp + 2) / 5 + 1
        let month = mp < 10 ? mp + 3 : mp - 9
        return (yoe + era * 400 + (month <= 2 ? 1 : 0), month, day)
    }

    private static func daysFromCivil(year: Int, month: Int, day: Int) -> Int {
        let y = month <= 2 ? year - 1 : year
        let era = (y >= 0 ? y : y - 399) / 400
        let yoe = y - era * 400
        let doy = (153 * (month > 2 ? month - 3 : month + 9) + 2) / 5 + day - 1
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
        return era * 146_097 + doe - 719_468
    }

    private static func daysIn(month: Int, year: Int) -> Int {
        switch month {
        case 2: (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 ? 29 : 28
        case 4, 6, 9, 11: 30
        default: 31
        }
    }

    private static func sameAuthor(_ a: StreamMessage, _ b: StreamMessage) -> Bool {
        a.authorKind == b.authorKind && a.authorUid == b.authorUid && (a.authorBotKey ?? "") == (b.authorBotKey ?? "")
    }
}
