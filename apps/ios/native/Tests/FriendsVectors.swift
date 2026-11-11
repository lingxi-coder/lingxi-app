import Foundation
import XCTest

@testable import LingxiCode

/// The shared Friends test vectors (`packages/teams/fixtures/friends-vectors.json`, docs/FRIENDS-CLIENTS.md section 5)
/// and the adapters from their row shapes to the app types. A failing row here and not on another client is a parity
/// bug in this platform, never a reason to edit the file.
///
/// The file is found in the test bundle (`project.yml` ships it), else by walking up from the calling test file, so this
/// works from the repository and from the macOS runner's scratch package (which links `packages`).
enum FriendsVectors {
    private final class ResourceBundle {}

    static func document(file: String = #filePath) throws -> [String: Any] {
        let name = "friends-vectors"
        var url = Bundle(for: ResourceBundle.self).url(forResource: name, withExtension: "json")
        var directory = URL(fileURLWithPath: file).deletingLastPathComponent()
        while url == nil, directory.path != "/" {
            let candidate = directory.appendingPathComponent("packages/teams/fixtures/\(name).json")
            if FileManager.default.fileExists(atPath: candidate.path) { url = candidate }
            directory.deleteLastPathComponent()
        }
        guard let url, let document = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any] else {
            throw VectorError.missing("packages/teams/fixtures/\(name).json above \(file)")
        }
        return document
    }

    static func section(_ path: String..., file: String = #filePath) throws -> Any {
        var current: Any = try document(file: file)
        for key in path {
            guard let next = (current as? [String: Any])?[key] else { throw VectorError.missing("\(path.joined(separator: ".")) (no '\(key)')") }
            current = next
        }
        return current
    }

    static func rows(_ path: String..., file: String = #filePath) throws -> [[String: Any]] {
        var current: Any = try document(file: file)
        for key in path {
            guard let next = (current as? [String: Any])?[key] else { throw VectorError.missing("\(path.joined(separator: ".")) (no '\(key)')") }
            current = next
        }
        guard let rows = current as? [[String: Any]] else { throw VectorError.missing("\(path.joined(separator: ".")) is not an array of objects") }
        return rows
    }

    /// The Foundation locales the vectors were checked with (the app's five UI languages).
    static let locales = ["zh-Hans", "zh-Hant", "en", "ja", "ko"].map(Locale.init(identifier:))

    /// A card from a sort, filter or badge row. `since` and `lastAtMs` are epoch milliseconds; `lastAtMs` makes a pointer.
    static func card(_ row: [String: Any]) throws -> FriendCard {
        let uid = row["uid"] as? String ?? "u"
        let state = try XCTUnwrap((row["state"] as? String).flatMap(FriendState.init(rawValue:)), "state of \(row)")
        let at = (row["lastAtMs"] as? NSNumber)?.int64Value
        let pointer = at.map {
            TeamLastMessage(id: "m", seq: 1, atMs: $0, authorKind: "user", authorUid: "x", authorSharedBotId: nil, preview: "hi", recalled: false)
        }
        return FriendCard(
            uid: uid, state: state, displayName: row["displayName"] as? String ?? "", handle: row["handle"] as? String, note: nil,
            sinceMs: (row["since"] as? NSNumber)?.int64Value ?? 0, teamId: state == .friends ? "f_\(uid)" : nil, generation: state == .friends ? 1 : nil,
            lastSeq: row["lastSeq"] as? Int ?? 0, lastReadSeq: row["lastReadSeq"] as? Int ?? 0, lastMessage: pointer)
    }

    enum VectorError: Error, CustomStringConvertible {
        case missing(String)
        var description: String {
            switch self {
            case let .missing(what): "friends vectors: \(what)"
            }
        }
    }
}
