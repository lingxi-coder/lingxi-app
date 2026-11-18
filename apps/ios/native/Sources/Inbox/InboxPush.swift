import Foundation

struct InboxPush: Equatable, Sendable {
    let ref: InboxRef
    let recipient: String
    let through: Int
    let eventKind: String
    let eventID: String
    let expiresAt: Int64

    static var now: Int64 { Int64(Date().timeIntervalSince1970 * 1_000) }

    var isAction: Bool {
        ["approval", "input", "pending_input", "pending_approval", "waiting_for_input", "waiting_for_approval"].contains(eventKind)
    }

    static func parse(_ payload: [AnyHashable: Any], currentUID: String?, enabled: Bool,
                      now: Int64 = Self.now) -> Self? {
        guard enabled, let currentUID, payload["kind"] as? String == "inbox",
              payload["recipientUid"] as? String == currentUID,
              let key = payload["key"] as? String,
              let generation = integer(payload["generation"]),
              let ref = InboxRef(key: key, generation: generation),
              let rawRoute = payload["route"] as? String, rawRoute.hasPrefix("inbox/"),
              String(rawRoute.dropFirst(6)).removingPercentEncoding == key,
              let through = integer(payload["through"]), through >= 0,
              let eventKind = payload["eventKind"] as? String, InboxRef.safe(eventKind),
              let expiration = integer(payload["expiresAt"]), expiration > now else { return nil }
        let eventID = (payload["notificationId"] as? String).flatMap { $0.utf8.count <= 512 ? $0 : nil }
            ?? "\(key):\(generation):\(eventKind):\(through)"
        return Self(ref: ref, recipient: currentUID, through: through, eventKind: eventKind,
                    eventID: eventID, expiresAt: Int64(expiration))
    }

    private static func integer(_ value: Any?) -> Int? {
        if let string = value as? String { return Int(string) }
        return value as? Int
    }
}
