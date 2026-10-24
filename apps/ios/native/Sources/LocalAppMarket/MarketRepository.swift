import Foundation
import CryptoKit
import ZIPFoundation

/// Validates staging only; never reads, deletes or rewrites an old installation registry.
enum MarketPackageFiles {
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
        guard entries.contains(where: { $0.path == "lingxi-app.json" && $0.type == .file }) else { throw MarketFailure("MANIFEST_REQUIRED") }
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
            let executable = entry.fileAttributes[.posixPermissions].flatMap { $0 as? NSNumber }.map { $0.intValue & 0o111 != 0 } ?? false
            try FileManager.default.setAttributes([.posixPermissions: executable ? 0o700 : 0o600], ofItemAtPath: target.path)
        }
    }
}
