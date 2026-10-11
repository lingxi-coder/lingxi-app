import Foundation

enum ConversationDeepLink {
    static func makeURL(
        sessionID: String,
        turnID: UInt64?,
        workspaceKey: String? = nil,
        sessionMode: String? = nil
    ) -> URL? {
        let trimmed = sessionID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        var components = URLComponents()
        components.scheme = "lingxi"
        components.host = "open_conversation"
        var queryItems = [URLQueryItem(name: "sessionId", value: trimmed)]
        if let turnID {
            queryItems.append(URLQueryItem(name: "turnId", value: String(turnID)))
        }
        if let workspaceKey, !workspaceKey.isEmpty {
            queryItems.append(URLQueryItem(name: "workspaceKey", value: workspaceKey))
        }
        if let sessionMode, !sessionMode.isEmpty {
            queryItems.append(URLQueryItem(name: "sessionMode", value: sessionMode))
        }
        components.queryItems = queryItems
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
