import Foundation

/// The 最近 list's mode filter chip. A pure list filter: choosing a chip never
/// changes the live conversation's mode — the mode follows the session the user
/// opens, and a new chat picks its own mode on the new-chat page.
enum RecentsFilter: String, CaseIterable, Hashable, Sendable {
    case all
    case chat
    case code

    func accepts(_ mode: SessionMode) -> Bool {
        switch self {
        case .all: true
        case .chat: mode == .chat
        case .code: mode == .code
        }
    }

    /// The mode this chip stands for, or nil for `.all`.
    var mode: SessionMode? {
        switch self {
        case .all: nil
        case .chat: .chat
        case .code: .code
        }
    }
}

/// The bucket a recent session is listed under. Declaration order is display order.
enum RecentBucket: Int, CaseIterable, Comparable, Sendable {
    case pinned
    case today
    case yesterday
    case lastWeek
    case earlier

    static func < (lhs: RecentBucket, rhs: RecentBucket) -> Bool { lhs.rawValue < rhs.rawValue }
}

/// What the drawer can honestly say about a session's live state. The engine is
/// bound to a single session at a time, so only the visible session can be
/// running or waiting for the user.
enum RecentStatus: Equatable, Sendable {
    case idle
    case running
    case awaitingConfirmation
}

/// One row of the unified recent-sessions list.
struct RecentSession: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let mode: SessionMode
    let scope: ConversationScope
    let workspaceName: String
    let messageCount: Int
    let relativeTime: String
    let modifiedAt: Date?
    let pinnedAt: Date?
    let status: RecentStatus

    var workspaceKey: String { scope.workspaceKey }
}

struct RecentGroup: Identifiable, Equatable, Sendable {
    let bucket: RecentBucket
    let sessions: [RecentSession]

    var id: RecentBucket { bucket }
}

enum RecentSessionsBuilder {
    /// Flatten the per-workspace session catalogs into the single, mode-agnostic
    /// recent list: pinned first, then today / yesterday / within seven days /
    /// earlier, newest first inside each bucket.
    ///
    /// Pure — the clock and calendar are parameters — so the bucket boundaries
    /// are unit-testable.
    ///
    /// - Parameter query: matches title and workspace name (localized,
    ///   case-insensitive); blank means no filtering.
    /// - Parameter pinnedAt: pin time per session id.
    /// - Parameter statusFor: live state per session id.
    static func groups(
        seeds: [WorkspaceGroupSeed],
        filter: RecentsFilter,
        query: String,
        pinnedAt: [String: Date],
        statusFor: (String) -> RecentStatus = { _ in .idle },
        now: Date = Date(),
        calendar: Calendar = .current
    ) -> [RecentGroup] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        var seen = Set<String>()
        var rows: [RecentSession] = []
        for seed in seeds {
            for row in seed.sessions {
                guard seen.insert(row.id).inserted, filter.accepts(row.mode) else { continue }
                if !needle.isEmpty,
                   !row.title.localizedStandardContains(needle),
                   !seed.title.localizedStandardContains(needle) {
                    continue
                }
                rows.append(
                    RecentSession(
                        id: row.id,
                        title: row.title,
                        mode: row.mode,
                        scope: seed.scope,
                        workspaceName: seed.title,
                        messageCount: row.messageCount,
                        relativeTime: row.relativeTime,
                        modifiedAt: row.modifiedAt,
                        pinnedAt: pinnedAt[row.id],
                        status: statusFor(row.id)
                    )
                )
            }
        }
        rows.sort(by: isOrderedBefore)
        let today = calendar.startOfDay(for: now)
        let grouped = Dictionary(grouping: rows) { bucket(for: $0, today: today, calendar: calendar) }
        return RecentBucket.allCases.compactMap { bucket in
            guard let sessions = grouped[bucket], !sessions.isEmpty else { return nil }
            return RecentGroup(bucket: bucket, sessions: sessions)
        }
    }

    /// One workspace's sessions, narrowed by the chip, newest first.
    static func sessions(in rows: [WorkspaceSessionRow], filter: RecentsFilter) -> [WorkspaceSessionRow] {
        rows.filter { filter.accepts($0.mode) }.sorted { lhs, rhs in
            switch (lhs.modifiedAt, rhs.modifiedAt) {
            case let (l?, r?) where l != r: return l > r
            case (.some, .none): return true
            case (.none, .some): return false
            default: return lhs.title.localizedStandardCompare(rhs.title) == .orderedAscending
            }
        }
    }

    /// Pinned projects lead (latest pin first); the rest order by the newest
    /// session or the project's own update time, then name.
    static func sortedProjects(_ projects: [ProjectSnapshot], pinnedAt: [String: Date]) -> [ProjectSnapshot] {
        func activity(_ project: ProjectSnapshot) -> Date? {
            let newest = project.sessions.filter { !$0.isArchived }.map(\.updatedAt).max()
            return [newest, project.record.updatedAt].compactMap { $0 }.max()
        }
        return projects.sorted { lhs, rhs in
            let l = pinnedAt[ConversationScope.project(lhs.id).workspaceKey]
            let r = pinnedAt[ConversationScope.project(rhs.id).workspaceKey]
            switch (l, r) {
            case let (l?, r?) where l != r: return l > r
            case (.some, .none): return true
            case (.none, .some): return false
            default: break
            }
            switch (activity(lhs), activity(rhs)) {
            case let (l?, r?) where l != r: return l > r
            case (.some, .none): return true
            case (.none, .some): return false
            default: break
            }
            let byName = lhs.record.name.localizedStandardCompare(rhs.record.name)
            if byName != .orderedSame { return byName == .orderedAscending }
            return lhs.id < rhs.id
        }
    }

    private static func isOrderedBefore(_ lhs: RecentSession, _ rhs: RecentSession) -> Bool {
        switch (lhs.pinnedAt, rhs.pinnedAt) {
        case let (l?, r?) where l != r: return l > r
        case (.some, .none): return true
        case (.none, .some): return false
        default: break
        }
        switch (lhs.modifiedAt, rhs.modifiedAt) {
        case let (l?, r?) where l != r: return l > r
        case (.some, .none): return true
        case (.none, .some): return false
        default: break
        }
        let byTitle = lhs.title.localizedStandardCompare(rhs.title)
        if byTitle != .orderedSame { return byTitle == .orderedAscending }
        return lhs.id < rhs.id
    }

    private static func bucket(for session: RecentSession, today: Date, calendar: Calendar) -> RecentBucket {
        if session.pinnedAt != nil { return .pinned }
        guard let modified = session.modifiedAt else { return .earlier }
        let day = calendar.startOfDay(for: modified)
        let age = calendar.dateComponents([.day], from: day, to: today).day ?? 0
        switch age {
        // A stamp from the future (clock skew) reads as today.
        case ...0: return .today
        case 1: return .yesterday
        case 2...6: return .lastWeek
        default: return .earlier
        }
    }
}
