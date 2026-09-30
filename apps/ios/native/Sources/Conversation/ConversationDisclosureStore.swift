import Foundation

/// Bounded UI preferences only; transcript contents never enter this store.
struct ConversationDisclosureStore {
    static let maximumSessions = 32
    static let maximumExpandedRows = 512
    private static let key = "conversation.disclosures.v1"
    let defaults: UserDefaults

    private struct Entry: Codable {
        let sessionID: String
        let expandedRows: [String]
        let updatedAt: Date
    }

    func load(sessionID: String) -> Set<String> {
        guard !sessionID.isEmpty else { return [] }
        return Set(entries.first { $0.sessionID == sessionID }?.expandedRows ?? [])
    }

    func save(_ expandedRows: Set<String>, sessionID: String) {
        guard !sessionID.isEmpty else { return }
        var next = entries.filter { $0.sessionID != sessionID }
        if !expandedRows.isEmpty {
            next.append(Entry(sessionID: sessionID,
                              expandedRows: Array(expandedRows.sorted().prefix(Self.maximumExpandedRows)),
                              updatedAt: Date()))
        }
        next.sort { $0.updatedAt > $1.updatedAt }
        if let data = try? JSONEncoder().encode(Array(next.prefix(Self.maximumSessions))) {
            defaults.set(data, forKey: Self.key)
        }
    }

    private var entries: [Entry] {
        guard let data = defaults.data(forKey: Self.key),
              let value = try? JSONDecoder().decode([Entry].self, from: data) else { return [] }
        return value
    }
}
