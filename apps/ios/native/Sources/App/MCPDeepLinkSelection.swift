import Foundation

enum MCPDeepLinkSelection: Equatable {
    case selected(scope: String)
    case missing
    case ambiguous
    case invalidScope

    static func resolve(name: String, requestedScope: String?, snapshot: [String: Any]) -> Self {
        let scopes = snapshot["scopes"] as? [[String: Any]] ?? []
        if let requestedScope, !["user", "local", "project"].contains(requestedScope) { return .invalidScope }
        let matches = scopes.compactMap { row -> String? in
            guard let scope = row["scope"] as? String,
                  let raw = row["raw_json"] as? String, let data = raw.data(using: .utf8),
                  let root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
                  (root["mcpServers"] as? [String: Any] ?? root)[name] != nil else { return nil }
            return scope
        }
        if let requestedScope { return matches.contains(requestedScope) ? .selected(scope: requestedScope) : .missing }
        if matches.count > 1 { return .ambiguous }
        return matches.first.map { .selected(scope: $0) } ?? .missing
    }
}
