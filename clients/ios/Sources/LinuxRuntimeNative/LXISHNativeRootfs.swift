//
//  LXISHNativeRootfs.swift
//  LingxiCode
//
//  Rootfs lifecycle and zip extraction logic adapted from OpenMinis
//  `src/ios/iSH/RootfsManager.swift`, trimmed to LingXi's mobile-linux bridge.
//
//  SPDX-License-Identifier: GPL-3.0-only
//

import Foundation

struct LXISHRootfsStatus: Codable {
    var state: String
    var backend: String
    var mode: String
    var platform: String
    var abi: String
    var version: String?
    var managedRoot: String?
    var activeRoot: String?
    var stagedRoot: String?
    var archiveSha256: String?
    var installedSizeBytes: UInt64?
    var writableGuestPaths: [String]
    var lastError: String?
}

struct LXISHNativeConfig: Codable, Hashable {
    var managedRoot: String
    var workspaceHostPath: String
    var stableWorkspaceId: String
    var abi: String
    var rootfsVersion: String
    var archiveSha256: String?
    var authorizationFile: String?

    var normalizedManagedRoot: URL {
        URL(fileURLWithPath: managedRoot, isDirectory: true).standardizedFileURL
    }

    var rootfsURL: URL {
        normalizedManagedRoot.appendingPathComponent("alpine-rootfs", isDirectory: true)
    }

    var rootfsDataURL: URL {
        rootfsURL.appendingPathComponent("data", isDirectory: true)
    }

    var rootfsMetadataURL: URL {
        normalizedManagedRoot.appendingPathComponent("bridge-state.json")
    }

    var mountsCacheURL: URL {
        normalizedManagedRoot.appendingPathComponent("mounts.json")
    }
}

enum LXISHRootfsError: LocalizedError {
    case archiveMissing
    case corrupt(String)

    var errorDescription: String? {
        switch self {
        case .archiveMissing:
            return "bundled alpine-rootfs.zip not found"
        case let .corrupt(message):
            return message
        }
    }
}

final class LXISHNativeRootfsManager {
    private let fileManager = FileManager.default
    private let currentArch = "aarch64"

    func status(for config: LXISHNativeConfig, lastError: String? = nil) -> LXISHRootfsStatus {
        let rootfsURL = config.rootfsURL
        let dataURL = config.rootfsDataURL
        let metaDB = rootfsURL.appendingPathComponent("meta.db")
        let installed = fileManager.fileExists(atPath: dataURL.path) && fileManager.fileExists(atPath: metaDB.path)
        let archMatches = readArchTag(at: rootfsURL) == currentArch
        let state: String
        if !fileManager.fileExists(atPath: rootfsURL.path) {
            state = "missing"
        } else if installed && archMatches {
            state = "ready"
        } else {
            state = "corrupt"
        }

        return LXISHRootfsStatus(
            state: state,
            backend: "ios-ish",
            mode: "mobileLinux",
            platform: "ios",
            abi: config.abi,
            version: config.rootfsVersion,
            managedRoot: config.normalizedManagedRoot.path,
            activeRoot: installed ? rootfsURL.path : nil,
            stagedRoot: nil,
            archiveSha256: config.archiveSha256,
            installedSizeBytes: installed ? directorySize(at: rootfsURL) : nil,
            writableGuestPaths: ["/workspace/\(config.stableWorkspaceId)", "/tmp", "/var/tmp", "/root"],
            lastError: lastError
        )
    }

    @discardableResult
    func installIfNeeded(for config: LXISHNativeConfig) throws -> LXISHRootfsStatus {
        try fileManager.createDirectory(at: config.normalizedManagedRoot, withIntermediateDirectories: true)
        let rootfsURL = config.rootfsURL
        let existing = status(for: config)
        if existing.state == "ready" {
            try ensureGuestDirectories(for: config)
            try persistBridgeMetadata(for: config)
            return status(for: config)
        }
        if fileManager.fileExists(atPath: rootfsURL.path) {
            try fileManager.removeItem(at: rootfsURL)
        }
        guard let zipURL = resolveBundledArchive(for: config) else {
            throw LXISHRootfsError.archiveMissing
        }
        try fileManager.createDirectory(at: rootfsURL, withIntermediateDirectories: true)
        try LXISHZipArchive(url: zipURL).extractAll(to: rootfsURL, strippingPrefix: "alpine-rootfs/")
        try currentArch.write(to: rootfsURL.appendingPathComponent(".arch"), atomically: true, encoding: .utf8)
        try ensureGuestDirectories(for: config)
        try applyDefaultMountOverlay(for: config, into: config.rootfsDataURL)
        try persistBridgeMetadata(for: config)
        return status(for: config)
    }

    func repair(for config: LXISHNativeConfig) throws -> LXISHRootfsStatus {
        let current = status(for: config)
        if current.state == "ready" {
            try ensureGuestDirectories(for: config)
            try persistBridgeMetadata(for: config)
            return status(for: config)
        }
        _ = try reset(for: config)
        return try installIfNeeded(for: config)
    }

    @discardableResult
    func reset(for config: LXISHNativeConfig) throws -> LXISHRootfsStatus {
        if fileManager.fileExists(atPath: config.rootfsURL.path) {
            try fileManager.removeItem(at: config.rootfsURL)
        }
        try? fileManager.removeItem(at: config.mountsCacheURL)
        try? fileManager.removeItem(at: config.rootfsMetadataURL)
        return status(for: config)
    }

    func cacheMounts(_ mounts: [LXISHMountSpec], for config: LXISHNativeConfig) throws {
        try fileManager.createDirectory(at: config.normalizedManagedRoot, withIntermediateDirectories: true)
        let data = try JSONEncoder().encode(mounts)
        try data.write(to: config.mountsCacheURL, options: .atomic)
    }

    func cachedMounts(for config: LXISHNativeConfig) -> [LXISHMountSpec] {
        guard let data = try? Data(contentsOf: config.mountsCacheURL) else {
            return []
        }
        return (try? JSONDecoder().decode([LXISHMountSpec].self, from: data)) ?? []
    }

    private func readArchTag(at rootfsURL: URL) -> String? {
        let tagURL = rootfsURL.appendingPathComponent(".arch")
        return try? String(contentsOf: tagURL, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func resolveBundledArchive(for config: LXISHNativeConfig) -> URL? {
        let env = ProcessInfo.processInfo.environment
        let candidates: [URL?] = [
            env["LINGXI_IOS_ISH_ROOTFS_ZIP"].map { URL(fileURLWithPath: $0) },
            Bundle.main.url(forResource: "alpine-rootfs", withExtension: "zip"),
            Bundle.main.url(forResource: "alpine-rootfs", withExtension: "zip", subdirectory: "LinuxRuntimeNative"),
            Bundle.main.resourceURL?.appendingPathComponent("alpine-rootfs.zip"),
            config.normalizedManagedRoot.appendingPathComponent("resources/alpine-rootfs.zip")
        ]
        return candidates.compactMap { $0 }.first(where: { fileManager.fileExists(atPath: $0.path) })
    }

    private func resolveDefaultMountDirectory(for config: LXISHNativeConfig) -> URL? {
        let env = ProcessInfo.processInfo.environment
        let candidates: [URL?] = [
            env["LINGXI_IOS_ISH_DEFAULT_MOUNT_DIR"].map { URL(fileURLWithPath: $0, isDirectory: true) },
            Bundle.main.url(forResource: "default_mount", withExtension: nil),
            Bundle.main.resourceURL?.appendingPathComponent("default_mount", isDirectory: true),
            config.normalizedManagedRoot.appendingPathComponent("resources/default_mount", isDirectory: true)
        ]
        return candidates.compactMap { $0 }.first(where: { fileManager.fileExists(atPath: $0.path) })
    }

    private func ensureGuestDirectories(for config: LXISHNativeConfig) throws {
        let dataRoot = config.rootfsDataURL
        let directories = [
            "var/lingxi",
            "var/lingxi/shared",
            "var/lingxi/memory",
            "var/lingxi/skills",
            "var/lingxi/tmp",
            "workspace/\(config.stableWorkspaceId)",
            "root",
            "tmp",
            "var/tmp"
        ]
        for relative in directories {
            try fileManager.createDirectory(
                at: dataRoot.appendingPathComponent(relative, isDirectory: true),
                withIntermediateDirectories: true
            )
        }
    }

    private func applyDefaultMountOverlay(for config: LXISHNativeConfig, into rootfsDataURL: URL) throws {
        guard let overlayURL = resolveDefaultMountDirectory(for: config) else {
            return
        }
        guard let enumerator = fileManager.enumerator(at: overlayURL, includingPropertiesForKeys: [.isDirectoryKey]) else {
            return
        }
        for case let sourceURL as URL in enumerator {
            let values = try sourceURL.resourceValues(forKeys: [.isDirectoryKey])
            let relativePath = sourceURL.path.replacingOccurrences(of: overlayURL.path, with: "")
            guard !relativePath.isEmpty else { continue }
            let destinationURL = rootfsDataURL.appendingPathComponent(relativePath)
            if values.isDirectory == true {
                try fileManager.createDirectory(at: destinationURL, withIntermediateDirectories: true)
                continue
            }
            try fileManager.createDirectory(at: destinationURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            if fileManager.fileExists(atPath: destinationURL.path) {
                try fileManager.removeItem(at: destinationURL)
            }
            try fileManager.copyItem(at: sourceURL, to: destinationURL)
        }
    }

    private func persistBridgeMetadata(for config: LXISHNativeConfig) throws {
        let payload: [String: String] = [
            "abi": config.abi,
            "rootfs_version": config.rootfsVersion,
            "arch": currentArch,
            "updated_at": ISO8601DateFormatter().string(from: Date())
        ]
        let data = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
        try data.write(to: config.rootfsMetadataURL, options: .atomic)
    }

    private func directorySize(at url: URL) -> UInt64 {
        guard let enumerator = fileManager.enumerator(at: url, includingPropertiesForKeys: [.fileSizeKey]) else {
            return 0
        }
        var total: UInt64 = 0
        for case let fileURL as URL in enumerator {
            let values = try? fileURL.resourceValues(forKeys: [.fileSizeKey])
            let size = values?.fileSize ?? 0
            total += UInt64(max(size, 0))
        }
        return total
    }
}

private final class LXISHZipArchive {
    struct Entry {
        let path: String
        let compressedSize: UInt32
        let uncompressedSize: UInt32
        let compressionMethod: UInt16
        let localHeaderOffset: UInt32
        let isDirectory: Bool
    }

    private let fileHandle: FileHandle
    private(set) var entries: [Entry] = []

    init(url: URL) throws {
        fileHandle = try FileHandle(forReadingFrom: url)
        try parse()
    }

    deinit {
        try? fileHandle.close()
    }

    func extractAll(to destinationURL: URL, strippingPrefix prefix: String) throws {
        let fileManager = FileManager.default
        let destinationRoot = destinationURL.standardizedFileURL
        for entry in entries {
            var relativePath = entry.path
            if relativePath.hasPrefix(prefix) {
                relativePath = String(relativePath.dropFirst(prefix.count))
            }
            guard !relativePath.isEmpty else { continue }
            let components = relativePath.split(separator: "/", omittingEmptySubsequences: true)
            guard !relativePath.hasPrefix("/"),
                  !components.isEmpty,
                  !components.contains(where: { $0 == ".." || $0 == "." })
            else {
                throw LXISHRootfsError.corrupt("unsafe zip entry path: \(entry.path)")
            }
            let outputURL = destinationRoot.appendingPathComponent(relativePath).standardizedFileURL
            guard outputURL.path.hasPrefix(destinationRoot.path + "/") else {
                throw LXISHRootfsError.corrupt("zip entry escapes managed root: \(entry.path)")
            }
            if entry.isDirectory {
                try fileManager.createDirectory(at: outputURL, withIntermediateDirectories: true)
                continue
            }
            try fileManager.createDirectory(at: outputURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            let data = try extractData(for: entry)
            try data.write(to: outputURL, options: .atomic)
        }
    }

    private func parse() throws {
        fileHandle.seekToEndOfFile()
        let fileSize = fileHandle.offsetInFile
        let searchSize = min(fileSize, 65_557)
        try fileHandle.seek(toOffset: fileSize - searchSize)
        let searchData = try fileHandle.read(upToCount: Int(searchSize)) ?? Data()
        guard let eocdOffset = findEOCD(in: searchData) else {
            throw LXISHRootfsError.corrupt("invalid zip archive")
        }
        let eocdStart = Int(fileSize - searchSize) + eocdOffset
        try fileHandle.seek(toOffset: UInt64(eocdStart))
        let eocdData = try require(count: 22)
        let entryCount = readUInt16(eocdData, at: 10)
        let centralDirOffset = readUInt32(eocdData, at: 16)
        try fileHandle.seek(toOffset: UInt64(centralDirOffset))
        for _ in 0..<entryCount {
            guard let entry = try readCentralDirectoryEntry() else { break }
            entries.append(entry)
        }
    }

    private func extractData(for entry: Entry) throws -> Data {
        try fileHandle.seek(toOffset: UInt64(entry.localHeaderOffset))
        let localHeader = try require(count: 30)
        let fileNameLength = readUInt16(localHeader, at: 26)
        let extraLength = readUInt16(localHeader, at: 28)
        try fileHandle.seek(
            toOffset: UInt64(entry.localHeaderOffset) + 30 + UInt64(fileNameLength) + UInt64(extraLength)
        )
        let payload = try require(count: Int(entry.compressedSize))
        let data: Data
        switch entry.compressionMethod {
        case 0:
            data = payload
        case 8:
            data = try (payload as NSData).decompressed(using: .zlib) as Data
        default:
            throw LXISHRootfsError.corrupt("unsupported zip compression method \(entry.compressionMethod)")
        }
        guard data.count == Int(entry.uncompressedSize) else {
            throw LXISHRootfsError.corrupt("zip entry size mismatch: \(entry.path)")
        }
        return data
    }

    private func readCentralDirectoryEntry() throws -> Entry? {
        let header = try require(count: 46)
        guard readUInt32(header, at: 0) == 0x02014b50 else { return nil }
        let compressionMethod = readUInt16(header, at: 10)
        let compressedSize = readUInt32(header, at: 20)
        let uncompressedSize = readUInt32(header, at: 24)
        let fileNameLength = readUInt16(header, at: 28)
        let extraLength = readUInt16(header, at: 30)
        let commentLength = readUInt16(header, at: 32)
        let localHeaderOffset = readUInt32(header, at: 42)
        let fileNameData = try require(count: Int(fileNameLength))
        let fileName = String(data: fileNameData, encoding: .utf8) ?? ""
        try fileHandle.seek(
            toOffset: fileHandle.offsetInFile + UInt64(extraLength) + UInt64(commentLength)
        )
        return Entry(
            path: fileName,
            compressedSize: compressedSize,
            uncompressedSize: uncompressedSize,
            compressionMethod: compressionMethod,
            localHeaderOffset: localHeaderOffset,
            isDirectory: fileName.hasSuffix("/")
        )
    }

    private func findEOCD(in data: Data) -> Int? {
        guard data.count >= 22 else { return nil }
        for index in stride(from: data.count - 22, through: 0, by: -1) {
            if readUInt32(data, at: index) == 0x06054b50 {
                return index
            }
        }
        return nil
    }

    private func require(count: Int) throws -> Data {
        let data = try fileHandle.read(upToCount: count) ?? Data()
        guard data.count == count else {
            throw LXISHRootfsError.corrupt("unexpected end of archive")
        }
        return data
    }

    private func readUInt16(_ data: Data, at offset: Int) -> UInt16 {
        UInt16(data[offset]) | (UInt16(data[offset + 1]) << 8)
    }

    private func readUInt32(_ data: Data, at offset: Int) -> UInt32 {
        UInt32(data[offset])
            | (UInt32(data[offset + 1]) << 8)
            | (UInt32(data[offset + 2]) << 16)
            | (UInt32(data[offset + 3]) << 24)
    }
}
