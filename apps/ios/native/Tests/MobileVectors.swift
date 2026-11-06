import Foundation
import XCTest

/// The shared Android / iOS test vectors of the mobile Bot / Team UI spec §8
/// (`packages/bridge-client/fixtures/mobile-team-bot-vectors.json`) and the agent-avatar identities
/// (`agent-avatar-identities.json`). Every pure-logic suite loads its table from here; a failing row on
/// one platform and not the other is a parity bug in that platform, never a reason to edit the file.
///
/// The fixtures are found by walking up from the calling test file, so this works from the repository
/// (`apps/ios/native/Tests`) and from the macOS runner's scratch package (which links `packages`).
enum MobileVectors {
    static let vectorsFile = "mobile-team-bot-vectors.json"
    static let avatarIdentitiesFile = "agent-avatar-identities.json"

    /// The whole vectors document.
    static func document(file: String = #filePath) throws -> [String: Any] {
        try object(vectorsFile, file: file)
    }

    /// A section of the vectors document, e.g. `section("mobile", "tabForLink")`.
    static func section(_ path: String..., file: String = #filePath) throws -> Any {
        var current: Any = try document(file: file)
        for key in path {
            guard let next = (current as? [String: Any])?[key] else {
                throw VectorError.missing("\(path.joined(separator: ".")) (no '\(key)')")
            }
            current = next
        }
        return current
    }

    static func rows(_ path: String..., file: String = #filePath) throws -> [[String: Any]] {
        var current: Any = try document(file: file)
        for key in path {
            guard let next = (current as? [String: Any])?[key] else {
                throw VectorError.missing("\(path.joined(separator: ".")) (no '\(key)')")
            }
            current = next
        }
        guard let rows = current as? [[String: Any]] else {
            throw VectorError.missing("\(path.joined(separator: ".")) is not an array of objects")
        }
        return rows
    }

    static func object(_ name: String, file: String = #filePath) throws -> [String: Any] {
        guard let object = try json(name, file: file) as? [String: Any] else {
            throw VectorError.missing("\(name) is not a JSON object")
        }
        return object
    }

    static func json(_ name: String, file: String = #filePath) throws -> Any {
        try JSONSerialization.jsonObject(with: Data(contentsOf: try url(of: name, from: file)))
    }

    /// `packages/bridge-client/fixtures/<name>`, from the nearest ancestor of `file` that has it.
    private final class ResourceBundle {}

    static func url(of name: String, from file: String) throws -> URL {
        if let bundled = Bundle(for: ResourceBundle.self).url(forResource: name, withExtension: nil) { return bundled }
        var directory = URL(fileURLWithPath: file).deletingLastPathComponent()
        for _ in 0..<10 {
            let candidate = directory.appendingPathComponent("packages/bridge-client/fixtures/\(name)")
            if FileManager.default.fileExists(atPath: candidate.path) { return candidate }
            let parent = directory.deletingLastPathComponent()
            if parent.path == directory.path { break }
            directory = parent
        }
        throw VectorError.missing("packages/bridge-client/fixtures/\(name) above \(file)")
    }

    enum VectorError: Error, CustomStringConvertible {
        case missing(String)
        var description: String {
            switch self {
            case let .missing(what): "mobile vectors: \(what)"
            }
        }
    }
}
