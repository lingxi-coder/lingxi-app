import Foundation

typealias WorkspaceSessionMode = SessionMode

enum WorkspaceGroupKind: String, Hashable, Sendable {
    case global
    case project
}

struct WorkspaceSessionRow: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let mode: WorkspaceSessionMode
    let modifiedAt: Date?
    let relativeTime: String
    let messageCount: Int
}

struct WorkspaceGroupSeed: Equatable, Sendable {
    let scope: ConversationScope
    let kind: WorkspaceGroupKind
    let title: String
    let subtitle: String?
    let updatedAt: Date?
    let sessions: [WorkspaceSessionRow]
}

enum WorkspaceGroupBuilder {
    static func mergedSessionRows(live: [EngineSession], cached: [ProjectSessionSummary]) -> [WorkspaceSessionRow] {
        let archived = Set(cached.filter(\.isArchived).map(\.sessionId))
        let liveRows = live.filter { !archived.contains($0.id) }.map { row in
            if let submitted = cached.first(where: { $0.sessionId == row.id && $0.pendingCatalogConfirmation && $0.messageCount > row.messageCount }) {
                return projectRow(submitted)
            }
            return engineRow(row)
        }
        let liveIDs = Set(liveRows.map(\.id))
        let pending = cached.filter { !$0.isArchived && !liveIDs.contains($0.sessionId) }
        return pending.map(projectRow) + liveRows
    }

    private static func engineRow(_ session: EngineSession) -> WorkspaceSessionRow {
        WorkspaceSessionRow(
            id: session.id,
            title: session.title,
            mode: session.mode,
            modifiedAt: session.modifiedAt,
            relativeTime: session.relativeTime,
            messageCount: session.messageCount
        )
    }

    private static func projectRow(_ session: ProjectSessionSummary) -> WorkspaceSessionRow {
        WorkspaceSessionRow(
            id: session.sessionId,
            title: session.title,
            mode: session.mode,
            modifiedAt: session.modifiedAt,
            relativeTime: session.relativeTime,
            messageCount: session.messageCount
        )
    }
}
