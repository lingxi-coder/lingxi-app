import Foundation

struct PushInstallationIdentity: Codable, Equatable {
    let id: String
    let secret: String
    var revision: Int64 = 0
    var mayBeBound = false

    static func create() -> Self {
        var generator = SystemRandomNumberGenerator()
        let secret = (0..<32).map { _ in String(format: "%02x", UInt8.random(in: .min ... .max, using: &generator)) }.joined()
        return Self(id: UUID().uuidString.lowercased(), secret: secret)
    }
}

enum PushMessagePolicy {
    /// Remote notifications open only validated RC, Teams inbox or Friends routes.
    static func remoteControlAction(_ info: [AnyHashable: Any], uid: String?, enabled: Bool, now: Date = Date()) -> LingxiAppAction? {
        guard accepts(info, uid: uid, enabled: enabled, now: now),
              let route = info["route"] as? String,
              let target = link(forRoute: route) else { return nil }
        return .canonical(target)
    }

    /// The link a notification route opens, or nil when a push may not open it. Besides the Teams inbox and a
    /// Remote Control session, a push may open the Friends requests and one friend's chat
    /// (`friend/chat?uid=<sender>`, the uid never percent-encoded); it never carries a friend code (`friend/add`).
    static func link(forRoute route: String) -> CanonicalDeepLink? {
        let remote = "^remote/[A-Za-z0-9_-]{22}/[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$"
        let friendChat = #"^friend/chat\?uid=[A-Za-z0-9_-]{1,128}$"#
        guard route == "team/inbox" || route == "friend/requests"
                || route.range(of: friendChat, options: .regularExpression) != nil
                || route.range(of: remote, options: .regularExpression) != nil else { return nil }
        return CanonicalDeepLink.parse("lingxi://" + route)
    }

    static func isRemote(_ info: [AnyHashable: Any]) -> Bool {
        info["gcm.message_id"] != nil || info["recipientUid"] != nil
    }
    static func accepts(_ info: [AnyHashable: Any], uid: String?, enabled: Bool, now: Date = Date()) -> Bool {
        guard enabled, let uid, info["recipientUid"] as? String == uid,
              let raw = info["expiresAt"] as? String, let expires = Double(raw), expires.isFinite else { return false }
        return expires > now.timeIntervalSince1970 * 1000
    }
}
