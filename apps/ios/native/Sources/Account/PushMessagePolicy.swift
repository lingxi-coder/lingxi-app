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
    /// Remote notifications may only open a validated Remote Control route.
    static func remoteControlAction(_ info: [AnyHashable: Any], uid: String?, enabled: Bool, now: Date = Date()) -> LingxiAppAction? {
        guard accepts(info, uid: uid, enabled: enabled, now: now),
              let route = info["route"] as? String,
              route.range(of: "^remote/[A-Za-z0-9_-]{22}/[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$", options: .regularExpression) != nil,
              let target = CanonicalDeepLink.parse("lingxi://" + route) else { return nil }
        return .canonical(target)
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
