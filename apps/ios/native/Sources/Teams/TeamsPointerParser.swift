import Foundation

// The strict reader of a conversation document (spec §8.4; mirrors `firestoreLastMessage` and the
// directory mapping in `packages/teams/firestore.ts`). Foundation only: the Firestore `Timestamp` is
// recognised by the caller's `millis` closure, so this file runs in the macOS XCTest harness against
// `mobile-team-bot-vectors.json` › `lastMessage`. Android has the identical contract in
// `teams/TeamsPointerParser.kt`.
//
// By project decision there is no compatibility code for older data: the backend and the Firestore rules
// write the whole `lastMessage` pointer, production conversations were repaired on 2026-11-03, and a
// document that breaks the contract stops the team's listeners with `INVALID_DATA`. A half-parsed list is
// never shown.
enum TeamsPointerParser {
    /// The failure code of a stored document that breaks the contract (`teams_error_invalid_data`).
    static let invalidDataCode = "INVALID_DATA"

    /// JavaScript's safe integer range, the range `seq` values are checked against.
    static let maxSafeInteger: Int64 = 9_007_199_254_740_991

    /// The `lastMessage` field of a conversation document.
    ///
    /// - Parameters:
    ///   - present: the field exists in the document. Absent is `INVALID_DATA`; an explicit `null`
    ///     (`NSNull`, or `nil` with `present: true`) is "no message yet". `[String: Any]` cannot tell the two
    ///     apart from a missing key's `nil`, so the caller passes `data.keys.contains("lastMessage")`.
    ///   - value: the stored value.
    ///   - millis: epoch milliseconds of a Firestore timestamp object, or `nil` when the object is not one.
    /// - Returns: `nil` for "no message yet", otherwise the whole pointer.
    /// - Throws: `TeamMessageFailure("INVALID_DATA")`.
    static func parseLastMessage(present: Bool, value: Any?, millis: (Any) -> Int64?) throws -> TeamLastMessage? {
        guard present else { throw invalid() }
        guard let value, !(value is NSNull) else { return nil }
        guard let map = value as? [String: Any] else { throw invalid() }
        guard let id = nonEmptyString(map["id"]), let seq = safeInteger(map["seq"]), let preview = map["preview"] as? String,
              let author = map["author"] as? [String: Any] else { throw invalid() }
        // `at` is `null` only while the server has not stamped a write made on this device; a plain string
        // or a missing field is not a timestamp.
        let atMs: Int64?
        switch map["at"] {
        case nil: throw invalid()
        case is NSNull: atMs = nil
        case let stamp?:
            guard let ms = millis(stamp) else { throw invalid() }
            atMs = ms
        }
        // `recalled` is either absent or exactly `true`.
        var recalled = false
        if let flag = map["recalled"] {
            guard isBool(flag), (flag as? NSNumber)?.boolValue == true else { throw invalid() }
            recalled = true
        }
        let kind: String
        let authorUid: String
        var botKey: String?
        switch author["kind"] as? String {
        case "user"?:
            kind = "user"
            guard let uid = nonEmptyString(author["uid"]) else { throw invalid() }
            authorUid = uid
        case "bot"?:
            kind = "bot"
            guard let uid = nonEmptyString(author["uid"]), let key = nonEmptyString(author["botKey"]) else { throw invalid() }
            authorUid = uid
            botKey = key
        default:
            throw invalid()
        }
        return TeamLastMessage(id: id, seq: seq, atMs: atMs, authorKind: kind, authorUid: authorUid, authorBotKey: botKey, preview: preview, recalled: recalled)
    }

    /// A document of the `conversations` query (the directory) as the app model. `data` is the document's
    /// fields; `lastReadSeq` comes from the viewer's `reads/{uid}` document, not from this one.
    /// - Throws: `TeamMessageFailure("INVALID_DATA")` for any field that breaks the contract, including a
    ///   missing `lastMessage`.
    static func parseConversation(id: String, data: [String: Any], lastReadSeq: Int, millis: (Any) -> Int64?) throws -> TeamMessageConversation {
        guard let kind = data["kind"] as? String, let title = data["title"] as? String,
              let participants = data["participants"] as? [String],
              let lastSeq = safeInteger(data["lastSeq"]), let lastChangeSeq = safeInteger(data["lastChangeSeq"]) else { throw invalid() }
        var botKey: String?
        switch data["botKey"] {
        case nil, is NSNull: break
        case let key as String: botKey = key.isEmpty ? nil : key
        default: throw invalid()
        }
        let last = try parseLastMessage(present: data.keys.contains("lastMessage"), value: data["lastMessage"], millis: millis)
        return TeamMessageConversation(id: id, kind: kind, title: title, participants: participants, lastSeq: lastSeq, lastChangeSeq: lastChangeSeq,
                                       lastReadSeq: lastReadSeq, botKey: botKey, lastMessage: last)
    }

    /// An integer in the JavaScript safe range. A Firestore double with an integral value is accepted; a
    /// fractional number, a boolean or a string is not.
    static func safeInteger(_ value: Any?) -> Int? {
        guard let value, let number = value as? NSNumber, !isBool(number) else { return nil }
        if CFNumberIsFloatType(number as CFNumber) {
            let double = number.doubleValue
            guard double.isFinite, double == double.rounded(.towardZero), abs(double) <= Double(maxSafeInteger) else { return nil }
            return Int(double)
        }
        // Compare as numbers, so an unsigned value beyond Int64 does not wrap into the range.
        guard number.compare(NSNumber(value: maxSafeInteger)) != .orderedDescending,
              number.compare(NSNumber(value: -maxSafeInteger)) != .orderedAscending else { return nil }
        return Int(number.int64Value)
    }

    private static func nonEmptyString(_ value: Any?) -> String? {
        guard let text = value as? String, !text.isEmpty else { return nil }
        return text
    }

    /// `NSNumber` also carries booleans; they must not pass for numbers (and `1` must not pass for `true`).
    private static func isBool(_ value: Any) -> Bool {
        guard let number = value as? NSNumber else { return false }
        return CFGetTypeID(number) == CFBooleanGetTypeID()
    }

    private static func invalid() -> TeamMessageFailure { TeamMessageFailure(code: invalidDataCode) }
}
