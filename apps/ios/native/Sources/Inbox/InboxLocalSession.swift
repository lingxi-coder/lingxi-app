import Foundation

struct InboxLocalSession: Equatable, Sendable {
    let session: ProjectSessionSummary
    let scope: ConversationScope
    let context: String?
    var ref: InboxRef?

    var sessionID: String { canonicalSessionID(session.sessionId) }

    struct Projection: Sendable {
        var rows: [InboxRow] = []
        var activity: [String: InboxAssistantActivity] = [:]
        var sessions: [String: InboxLocalSession] = [:]
    }

    private static var activityDirectory: URL {
        URL(fileURLWithPath: MobileUserStorage.directories().configHome, isDirectory: true)
            .appending(path: "inbox-activity", directoryHint: .isDirectory)
    }

    static func readActivity(_ ref: InboxRef, after: Int, through: Int) -> [InboxAssistantReadEntry] {
        let root = activityDirectory
        guard ref.kind == .assistant, after >= 0, through > after,
              let id = UUID(uuidString: ref.conversationId)?.uuidString.lowercased(),
              let currentSource = try? String(contentsOf: root.appending(path: "source-id"), encoding: .utf8),
              currentSource.trimmingCharacters(in: .whitespacesAndNewlines) == ref.sourceId else { return [] }
        let directory = root.appending(path: id + "/entries", directoryHint: .isDirectory)
        let decoder = JSONDecoder()
        var page: [InboxAssistantReadEntry] = []
        for cursor in (after + 1)...(after + min(50, through - after)) {
            let path = directory.appending(path: "\(cursor).json")
            guard !Task.isCancelled,
                  let info = try? path.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey]),
                  info.isRegularFile == true, let bytes = info.fileSize, (1...4_096).contains(bytes),
                  let data = try? Data(contentsOf: path),
                  let entry = try? decoder.decode(InboxAssistantReadEntry.self, from: data),
                  entry.cursor == cursor else { break }
            page.append(entry)
        }
        return page
    }

    static func load(_ catalog: [InboxLocalSession]) -> Projection {
        let root = activityDirectory
        guard let raw = try? String(contentsOf: root.appending(path: "source-id"), encoding: .utf8),
              let sourceID = UUID(uuidString: raw.trimmingCharacters(in: .whitespacesAndNewlines))?.uuidString.lowercased() else {
            return Projection()
        }

        let decoder = JSONDecoder()
        var result = Projection()
        for var entry in catalog {
            if Task.isCancelled { return result }
            guard let id = UUID(uuidString: entry.sessionID)?.uuidString.lowercased() else { continue }
            let ref = InboxRef(kind: .assistant, sourceId: sourceID, conversationId: id, generation: 1)
            entry.ref = ref
            var row = InboxRow(key: ref.key, ref: ref, category: .assistant,
                               title: entry.session.title, context: entry.context,
                               lastActivityAt: Int64(entry.session.updatedAt.timeIntervalSince1970 * 1_000),
                               archived: entry.session.isArchived)
            let path = root.appending(path: id + "/summary.json")
            if let data = try? Data(contentsOf: path), data.count <= 4_096,
               let activity = try? decoder.decode(InboxAssistantActivity.self, from: data),
               activity.sourceId == sourceID, (0...activity.cursor).contains(activity.baselineCursor) {
                result.activity[ref.key] = activity
                row = InboxAssistantSummary(activity: activity).updating(row)
                switch activity.previewKind {
                case "reply": row.preview = String(localized: "inbox_preview_reply")
                case "taskResult": row.preview = String(localized: "inbox_preview_task_completed")
                case "failure": row.preview = String(localized: "inbox_preview_task_failed")
                default: break
                }
            }
            result.rows.append(row)
            result.sessions[ref.key] = entry
        }
        return result
    }
}
