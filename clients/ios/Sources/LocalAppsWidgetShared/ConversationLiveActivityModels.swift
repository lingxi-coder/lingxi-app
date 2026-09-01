import Foundation

enum ConversationDeepLink {
    static func makeURL(sessionID: String, turnID: UInt64?) -> URL? {
        let trimmed = sessionID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        var components = URLComponents()
        components.scheme = "lingxi"
        components.host = "open_conversation"
        var queryItems = [URLQueryItem(name: "sessionId", value: trimmed)]
        if let turnID {
            queryItems.append(URLQueryItem(name: "turnId", value: String(turnID)))
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

    var deepLinkURL: URL? {
        ConversationDeepLink.makeURL(sessionID: sessionID, turnID: turnID)
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
        }

        let sessionID: String
    }
#endif
