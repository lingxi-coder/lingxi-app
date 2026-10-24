import Foundation

/// Persist the mutation identity before any server write. A failed disk write
/// stops publication; a retry can recover a server result with the same key.
@MainActor
final class LocalAppPublicationReceipts {
    private let file: URL
    init(directory: URL) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        file = directory.appendingPathComponent("receipts.json")
    }
    private func load() throws -> [String: String] {
        guard FileManager.default.fileExists(atPath: file.path) else { return [:] }
        return try JSONDecoder().decode([String: String].self, from: Data(contentsOf: file))
    }
    func get(_ name: String) throws -> String? { try load()[name] }
    func set(_ name: String, _ value: String) throws {
        var values = try load()
        values[name] = value
        try JSONEncoder().encode(values).write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        let handle = try FileHandle(forWritingTo: file)
        defer { try? handle.close() }
        try handle.synchronize()
    }
    func mutationKey(_ name: String) throws -> String {
        if let previous = try get(name) { return previous }
        let value = UUID().uuidString
        try set(name, value)
        return value
    }
    func fixedValue(_ name: String, proposed: String) throws -> String {
        if let value = try get(name) { return value }
        try set(name, proposed)
        return proposed
    }
    func preparedArchive(_ archive: URL, releaseKey: String, proposed: Data) throws -> Data {
        let hashKey = releaseKey + ":archiveHash"
        if FileManager.default.fileExists(atPath: archive.path), let expected = try get(hashKey) {
            let bytes = try Data(contentsOf: archive)
            guard UnifiedAppMarket.digest(bytes) == expected else { throw MarketFailure("PUBLISH_ARCHIVE_CHANGED") }
            return bytes
        }
        guard try get(releaseKey + ":createVersion") == nil,
              try get(releaseKey + ":versionID") == nil else { throw MarketFailure("PUBLISH_ARCHIVE_MISSING", "Preserve the pending publication for recovery.") }
        // No version request can precede its durable key. An unsubmitted orphan
        // archive can be recreated after a failed receipt write.
        try FileManager.default.createDirectory(at: archive.deletingLastPathComponent(), withIntermediateDirectories: true)
        try proposed.write(to: archive, options: .atomic)
        let handle = try FileHandle(forWritingTo: archive)
        defer { try? handle.close() }
        try handle.synchronize()
        try set(hashKey, UnifiedAppMarket.digest(proposed))
        return proposed
    }
}
