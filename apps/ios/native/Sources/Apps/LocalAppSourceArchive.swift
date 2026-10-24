import Foundation
import ZIPFoundation

struct LocalAppSourceArchive: Sendable {
    let appID: String
    let revision: UInt64
    let digest: String
    let files: [String]
    let bytes: Data
}

extension UnifiedLocalApps {
    func importSource(_ url: URL) async throws -> LocalAppPublishResult {
        let isDirectory = try url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true
        if isDirectory { return try await client.request(["op": .string("import"), "sourcePath": .string(url.path)]) }
        let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
        guard size > 0, size <= 25 * 1024 * 1024 else { throw MarketFailure("UPLOAD_TOO_LARGE") }
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("local-app-import-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        try await Task.detached(priority: .userInitiated) { try MarketPackageFiles.extract(url, to: folder) }.value
        try Task.checkCancellation()
        return try await client.request(["op": .string("import"), "sourcePath": .string(folder.path)])
    }

    /// The shared library selects the exact public inventory before native ZIP creation.
    func sourceArchive(_ id: String, revision: UInt64) async throws -> LocalAppSourceArchive {
        struct Inventory: Decodable, Sendable {
            struct File: Decodable, Sendable { let path: String }
            let digest: String
            let files: [File]
        }
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("local-app-export-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let source = folder.appendingPathComponent("source", isDirectory: true)
        let inventory: Inventory = try await client.request(["op": .string("export"), "appId": .string(id), "revision": .number(Double(revision)), "destination": .string(source.path)])
        let zip = folder.appendingPathComponent("source.zip")
        let bytes = try await Task.detached(priority: .userInitiated) {
            try Task.checkCancellation()
            try FileManager.default.zipItem(at: source, to: zip, shouldKeepParent: false)
            let size = try zip.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard size > 0, size <= 25 * 1024 * 1024 else { throw MarketFailure("UPLOAD_TOO_LARGE") }
            return try Data(contentsOf: zip)
        }.value
        try Task.checkCancellation()
        return .init(appID: id, revision: revision, digest: inventory.digest, files: inventory.files.map(\.path).sorted(), bytes: bytes)
    }

    static func guestDirectory(_ hostPath: String) throws -> String {
        let base = URL(fileURLWithPath: MobileUserStorage.directories().home, isDirectory: true).resolvingSymlinksInPath().path
        let path = URL(fileURLWithPath: hostPath, isDirectory: true).resolvingSymlinksInPath().path
        guard path == base || path.hasPrefix(base + "/") else { throw LocalAppServiceFailure("INVALID_SOURCE", "Directory is outside the Linux user home.") }
        return LXISHGuestPaths.home + String(path.dropFirst(base.count))
    }
}
