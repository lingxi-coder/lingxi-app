import Foundation

enum ConversationDeepLink {
    static func makeURL(
        sessionID: String,
        turnID: UInt64?,
        workspaceKey: String? = nil,
        sessionMode: String? = nil,
        messageUUID: String? = nil
    ) -> URL? {
        var parameters: [String: String] = [:]
        if let workspaceKey, !workspaceKey.isEmpty { parameters["workspace"] = workspaceKey }
        if let sessionMode, !sessionMode.isEmpty {
            guard ["code", "chat"].contains(sessionMode) else { return nil }
            parameters["mode"] = sessionMode
        }
        if let messageUUID {
            guard UUID(uuidString: messageUUID) != nil else { return nil }
            parameters["message"] = messageUUID.lowercased()
        }
        // Turn counters are process-local; public links use persisted message UUIDs.
        return makeURL(route: ["session", sessionID.trimmingCharacters(in: .whitespacesAndNewlines)], parameters: parameters)
    }

    /// Shared by notifications, Live Activities, Widgets and copy-link actions.
    static func makeURL(route: [String], parameters: [String: String] = [:]) -> URL? {
        let allowed = CharacterSet.urlPathAllowed.subtracting(CharacterSet(charactersIn: "/%?#\\"))
        guard !route.isEmpty, route.allSatisfy({ value in
            !value.isEmpty && value.count <= 512 && value != "." && value != ".." && !value.contains("/") && !value.contains("\\") &&
                !value.unicodeScalars.contains(where: { $0.value < 32 || $0.value == 127 })
        }), parameters.allSatisfy({ _, value in
            !value.isEmpty && value.count <= 512 && !value.unicodeScalars.contains(where: { $0.value < 32 || $0.value == 127 })
        }) else { return nil }
        let encoded = route.compactMap { $0.addingPercentEncoding(withAllowedCharacters: allowed) }
        guard encoded.count == route.count else { return nil }
        var components = URLComponents()
        components.scheme = "https"
        components.host = "lingxios.net"
        components.percentEncodedPath = "/open/" + encoded.joined(separator: "/")
        components.queryItems = parameters.isEmpty ? nil : parameters.sorted { $0.key < $1.key }.map { URLQueryItem(name: $0.key, value: $0.value) }
        return components.url
    }

}

struct ConversationLiveActivitySnapshot: Equatable, Sendable {
    enum Status: String, Codable, Hashable, Sendable {
        case running
        case waiting
        case paused
        case completed
        case failed

        var isTerminal: Bool {
            switch self {
            case .completed, .failed:
                return true
            case .running, .waiting, .paused:
                return false
            }
        }
    }

    let sessionID: String
    let turnID: UInt64?
    let title: String
    let subtitle: String
    let status: Status
    let updatedAt: Date
    let workspaceKey: String
    let sessionMode: String

    init(
        sessionID: String,
        turnID: UInt64?,
        title: String,
        subtitle: String,
        status: Status,
        updatedAt: Date,
        workspaceKey: String = "global",
        sessionMode: String = "code"
    ) {
        self.sessionID = sessionID
        self.turnID = turnID
        self.title = title
        self.subtitle = subtitle
        self.status = status
        self.updatedAt = updatedAt
        self.workspaceKey = workspaceKey
        self.sessionMode = sessionMode
    }

    var deepLinkURL: URL? {
        ConversationDeepLink.makeURL(
            sessionID: sessionID,
            turnID: turnID,
            workspaceKey: workspaceKey,
            sessionMode: sessionMode
        )
    }
}

#if canImport(ActivityKit)
    import ActivityKit

    @available(iOS 18.0, *)
    struct ConversationLiveActivityAttributes: ActivityAttributes {
        public struct ContentState: Codable, Hashable {
            let title: String
            let subtitle: String
            let status: ConversationLiveActivitySnapshot.Status
            let turnID: UInt64?
            let updatedAt: Date
            let workspaceKey: String
            let sessionMode: String
        }

        let sessionID: String
    }
#endif
