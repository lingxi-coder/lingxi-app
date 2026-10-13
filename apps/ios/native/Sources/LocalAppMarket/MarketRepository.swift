import Foundation
import CryptoKit
import ZIPFoundation

struct MarketInstallation: Codable, Equatable, Identifiable {
    var id: String { appId }
    let source: URL
    let appId: String
    let versionId: String
    let sequence: Int
    let installationID: UUID
    let directory: String
    var port: UInt16?
    var removing: Bool = false
}
/// Actor serializes registry commits. Downloads for distinct apps may run concurrently.
actor MarketRepository {
    private let root: URL
    private var records: [String: MarketInstallation]
    private var busy = Set<String>()
    private var blocked = Set<String>()
    private let fm = FileManager.default
    init(root: URL) throws {
        self.root = root
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let registry = root.appendingPathComponent("registry.json")
        records = FileManager.default.fileExists(atPath: registry.path) ? try JSONDecoder().decode([String: MarketInstallation].self, from: Data(contentsOf: registry)) : [:]
        let staging = root.appendingPathComponent("staging")
        if FileManager.default.fileExists(atPath: staging.path) { try FileManager.default.removeItem(at: staging) }
        try FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
        let versions = root.appendingPathComponent("versions")
        try FileManager.default.createDirectory(at: versions, withIntermediateDirectories: true)
        let referenced = Set(records.values.map(\.directory))
        for directory in try FileManager.default.contentsOfDirectory(at: versions, includingPropertiesForKeys: nil) where !referenced.contains(directory.lastPathComponent) { try FileManager.default.removeItem(at: directory) }
    }
    func installations() -> [MarketInstallation] { records.values.sorted { $0.appId < $1.appId } }
    func acquire(_ appId: String) throws {
        guard busy.insert(appId).inserted else { throw MarketFailure("OPERATION_IN_PROGRESS") }
    }
    func release(_ appId: String) { busy.remove(appId); blocked.remove(appId) }
    func installation(_ appId: String) throws -> MarketInstallation {
        guard !blocked.contains(appId), let record = records[appId], !record.removing else { throw MarketFailure("APP_NOT_INSTALLED_OR_BUSY") }
        return record
    }
    func resources(_ record: MarketInstallation) -> URL { root.appendingPathComponent("versions").appendingPathComponent(record.directory) }
    func blockOpen(_ appId: String) { blocked.insert(appId) }
    func savePort(_ appId: String, installationID: UUID, port: UInt16) throws {
        guard var record = records[appId], record.installationID == installationID, !record.removing, !blocked.contains(appId) else { throw MarketFailure("APP_NOT_INSTALLED_OR_BUSY") }
        record.port = port
        var next = records; next[appId] = record
        try persist(next)
    }
    private func persist(_ next: [String: MarketInstallation]) throws {
        try JSONEncoder().encode(next).write(to: root.appendingPathComponent("registry.json"), options: .atomic)
        records = next
    }
    func stage(_ distribution: MarketDistribution, api: MarketAPI, progress: @escaping @Sendable (Double) async -> Void, verifying: @escaping @Sendable () async -> Void = {}) async throws -> URL {
        let folder = root.appendingPathComponent("staging").appendingPathComponent(UUID().uuidString)
        try fm.createDirectory(at: folder, withIntermediateDirectories: true)
        do {
            let download = folder.appendingPathComponent("artifact")
            try await api.download(distribution.artifact, to: download, progress: progress)
            try Task.checkCancellation()
            await verifying()
            let digest = SHA256.hash(data: try Data(contentsOf: download)).map { String(format: "%02x", $0) }.joined()
            guard digest == distribution.artifact.sha256.lowercased() else { throw MarketFailure("HASH_MISMATCH") }
            let content = folder.appendingPathComponent("content")
            try fm.createDirectory(at: content, withIntermediateDirectories: true)
            switch distribution.artifact.format {
            case "html": try fm.moveItem(at: download, to: content.appendingPathComponent("index.html"))
            case "zip": try Self.extract(download, to: content)
            default: throw MarketFailure("UNSUPPORTED_FORMAT")
            }
            return folder
        } catch { try? fm.removeItem(at: folder); throw error }
    }
    func discard(_ staging: URL) { try? fm.removeItem(at: staging) }
    func commit(_ distribution: MarketDistribution, staged: URL, source: URL) throws -> MarketInstallation {
        let old = records[distribution.appId]
        guard old?.removing != true else { throw MarketFailure("UNINSTALL_PENDING") }
        let name = UUID().uuidString
        let target = root.appendingPathComponent("versions").appendingPathComponent(name)
        try fm.moveItem(at: staged.appendingPathComponent("content"), to: target)
        let record = MarketInstallation(source: source, appId: distribution.appId, versionId: distribution.versionId, sequence: distribution.sequence, installationID: old?.installationID ?? UUID(), directory: name, port: old?.port)
        var next = records; next[record.appId] = record
        do { try persist(next) } catch { try? fm.removeItem(at: target); throw error }
        try? fm.removeItem(at: staged)
        if let old { try? fm.removeItem(at: resources(old)) }
        return record
    }
    func beginRemoval(_ appId: String) throws -> MarketInstallation {
        guard var record = records[appId] else { throw MarketFailure("APP_NOT_INSTALLED") }
        record.removing = true
        var next = records; next[appId] = record; try persist(next)
        return record
    }
    func finishRemoval(_ record: MarketInstallation) throws {
        if fm.fileExists(atPath: resources(record).path) { try fm.removeItem(at: resources(record)) }
        var next = records; next.removeValue(forKey: record.appId); try persist(next)
    }
    static func safePath(_ input: String) throws -> String {
        let path = input.precomposedStringWithCanonicalMapping
        let stripped = path.hasSuffix("/") ? String(path.dropLast()) : path
        guard !stripped.isEmpty, stripped.utf16.count <= 512, !stripped.hasPrefix("/"), stripped.range(of: "[\\\\%?#:\\x00-\\x1f\\x7f]", options: .regularExpression) == nil else { throw MarketFailure("UNSAFE_PATH") }
        for part in stripped.split(separator: "/", omittingEmptySubsequences: false) {
            guard !part.isEmpty, !part.hasPrefix("."), !part.hasSuffix("."), !part.hasSuffix(" "), part.utf16.count <= 180 else { throw MarketFailure("UNSAFE_PATH") }
        }
        return stripped
    }
    // Validate central and local flags explicitly: ZIPFoundation's iterator can
    // terminate early for an encrypted/malformed entry rather than throwing.
    static func validateZIP(_ data: Data) throws -> Int {
        func number(_ offset: Int, _ length: Int) throws -> UInt64 {
            guard offset >= 0, offset + length <= data.count else { throw MarketFailure("INVALID_ZIP") }
            return (0..<length).reduce(0) { $0 | UInt64(data[offset + $1]) << ($1 * 8) }
        }
        guard data.count >= 22 else { throw MarketFailure("INVALID_ZIP") }
        var end: Int?
        for offset in stride(from: data.count - 22, through: max(0, data.count - 65557), by: -1) {
            if try number(offset, 4) == 0x06054b50, offset + 22 + Int(try number(offset + 20, 2)) == data.count { end = offset; break }
        }
        guard let end, try number(end + 4, 2) == 0, try number(end + 6, 2) == 0 else { throw MarketFailure("UNSUPPORTED_ZIP") }
        let count = Int(try number(end + 10, 2))
        guard count > 0, count <= 2000, try number(end + 8, 2) == UInt64(count) else { throw MarketFailure("FILE_COUNT_LIMIT") }
        var cursor = Int(try number(end + 16, 4))
        let centralEnd = cursor + Int(try number(end + 12, 4))
        guard centralEnd == end else { throw MarketFailure("INVALID_ZIP") }
        for _ in 0..<count {
            guard try number(cursor, 4) == 0x02014b50 else { throw MarketFailure("INVALID_ZIP") }
            let flags = try number(cursor + 8, 2), method = try number(cursor + 10, 2)
            guard flags & 0x41 == 0, method == 0 || method == 8 else { throw MarketFailure("UNSUPPORTED_ZIP") }
            let local = Int(try number(cursor + 42, 4))
            guard try number(local, 4) == 0x04034b50, try number(local + 6, 2) == flags, try number(local + 8, 2) == method else { throw MarketFailure("INVALID_ZIP") }
            cursor += 46 + Int(try number(cursor + 28, 2)) + Int(try number(cursor + 30, 2)) + Int(try number(cursor + 32, 2))
            guard cursor <= centralEnd else { throw MarketFailure("INVALID_ZIP") }
        }
        guard cursor == centralEnd else { throw MarketFailure("INVALID_ZIP") }
        return count
    }
    static func extract(_ source: URL, to destination: URL) throws {
        let expectedCount = try validateZIP(Data(contentsOf: source))
        let archive = try Archive(url: source, accessMode: .read)
        let entries = Array(archive)
        guard entries.count == expectedCount, entries.count <= 2000 else { throw MarketFailure("FILE_COUNT_LIMIT") }
        var paths = Set<String>(); var files = Set<String>(); var total: UInt64 = 0
        for entry in entries {
            try Task.checkCancellation()
            guard entry.type != .symlink else { throw MarketFailure("SYMLINK_REJECTED") }
            let path = try safePath(entry.path)
            let key = path.lowercased()
            guard paths.insert(key).inserted else { throw MarketFailure("DUPLICATE_PATH") }
            if entry.type == .file { files.insert(key) }
            guard entry.uncompressedSize <= 25 * 1024 * 1024 else { throw MarketFailure("ENTRY_SIZE_LIMIT") }
            total += UInt64(entry.uncompressedSize)
            guard total <= 100 * 1024 * 1024 else { throw MarketFailure("EXPANDED_SIZE_LIMIT") }
        }
        for path in paths {
            let components = path.split(separator: "/")
            for count in 1..<components.count where files.contains(components.prefix(count).joined(separator: "/")) { throw MarketFailure("PATH_CONFLICT") }
        }
        guard entries.contains(where: { $0.path == "index.html" && $0.type == .file }) else { throw MarketFailure("INDEX_REQUIRED") }
        var actual: UInt64 = 0
        for entry in entries {
            let target = destination.appendingPathComponent(try safePath(entry.path))
            if entry.type == .directory { try FileManager.default.createDirectory(at: target, withIntermediateDirectories: true); continue }
            try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
            FileManager.default.createFile(atPath: target.path, contents: nil)
            let file = try FileHandle(forWritingTo: target)
            defer { try? file.close() }
            var entryActual: UInt64 = 0
            let crc = try archive.extract(entry, skipCRC32: false) { data in
                try Task.checkCancellation()
                actual += UInt64(data.count); entryActual += UInt64(data.count)
                guard entryActual <= entry.uncompressedSize, entryActual <= 25 * 1024 * 1024 else { throw MarketFailure("ENTRY_SIZE_LIMIT") }
                guard actual <= 100 * 1024 * 1024 else { throw MarketFailure("EXPANDED_SIZE_LIMIT") }
                try file.write(contentsOf: data)
            }
            guard entryActual == entry.uncompressedSize else { throw MarketFailure("SIZE_MISMATCH") }
            guard crc == entry.checksum else { throw MarketFailure("CRC_MISMATCH") }
        }
    }
}
