import CryptoKit
import Foundation

private let syncTempMarker = ".__lingxi_temp__"
private let syncBackupMarker = ".__lingxi_backup__"
private let maxSyncEntries = 50_000
private let maxSyncFileBytes: Int64 = 2 * 1024 * 1024 * 1024
private let maxSyncTotalBytes: Int64 = 8 * 1024 * 1024 * 1024
private let minFreeSpaceBytes: Int64 = 64 * 1024 * 1024

protocol ProjectPathCoordinating: Sendable {
    func coordinateRead<T>(at url: URL, _ block: (URL) throws -> T) throws -> T
    func coordinateWrite<T>(at url: URL, _ block: (URL) throws -> T) throws -> T
}

struct NSFileProjectCoordinator: ProjectPathCoordinating {
    func coordinateRead<T>(at url: URL, _ block: (URL) throws -> T) throws -> T {
        try coordinate(url: url, writing: false, block)
    }

    func coordinateWrite<T>(at url: URL, _ block: (URL) throws -> T) throws -> T {
        try coordinate(url: url, writing: true, block)
    }

    private func coordinate<T>(url: URL, writing: Bool, _ block: (URL) throws -> T) throws -> T {
        let coordinator = NSFileCoordinator(filePresenter: nil)
        var coordinationError: NSError?
        var result: Result<T, Error>?
        if writing {
            coordinator.coordinate(writingItemAt: url, options: [], error: &coordinationError) { coordinatedURL in
                result = Result { try block(coordinatedURL) }
            }
        } else {
            coordinator.coordinate(readingItemAt: url, options: [], error: &coordinationError) { coordinatedURL in
                result = Result { try block(coordinatedURL) }
            }
        }
        if let coordinationError { throw coordinationError }
        guard let result else { throw CocoaError(.fileReadUnknown) }
        return try result.get()
    }
}

struct ProjectExternalTransactionHooks: Sendable {
    var beforePromoteTemp: @Sendable () throws -> Void = {}
}

struct ScannedFile: Equatable, Sendable {
    let url: URL
    let sha256: String
    let sizeBytes: Int64
}

private struct SyncBudget {
    private(set) var entries = 0
    private(set) var totalBytes: Int64 = 0

    mutating func includeEntry() throws {
        entries += 1
        guard entries <= maxSyncEntries else {
            throw ProjectSynchronizerError.tooManyEntries(maxSyncEntries)
        }
    }

    mutating func includeFile(_ sizeBytes: Int64) throws {
        guard sizeBytes <= maxSyncFileBytes else {
            throw ProjectSynchronizerError.fileTooLarge
        }
        totalBytes += sizeBytes
        guard totalBytes <= maxSyncTotalBytes else {
            throw ProjectSynchronizerError.totalSizeTooLarge
        }
    }
}

final class ProjectWorkspaceSynchronizer: @unchecked Sendable {
    private let repository: ProjectRepository
    private let bookmarkResolver: ProjectBookmarkResolving
    private let coordinator: ProjectPathCoordinating
    private let now: @Sendable () -> Date
    private let hooks: ProjectExternalTransactionHooks

    init(
        repository: ProjectRepository,
        bookmarkResolver: ProjectBookmarkResolving = SecurityScopedProjectBookmarkResolver(),
        coordinator: ProjectPathCoordinating = NSFileProjectCoordinator(),
        now: @escaping @Sendable () -> Date = { Date() },
        hooks: ProjectExternalTransactionHooks = .init()
    ) {
        self.repository = repository
        self.bookmarkResolver = bookmarkResolver
        self.coordinator = coordinator
        self.now = now
        self.hooks = hooks
    }

    func importInitial(projectId: String) throws -> ProjectSyncResult {
        try synchronize(projectId: projectId, direction: .externalToInternal, resolution: nil)
    }

    func reimport(projectId: String) throws -> ProjectSyncResult {
        try synchronize(projectId: projectId, direction: .externalToInternal, resolution: nil)
    }

    func export(projectId: String) throws -> ProjectSyncResult {
        try synchronize(projectId: projectId, direction: .internalToExternal, resolution: nil)
    }

    func resolve(projectId: String, resolution: ProjectConflictResolution) throws -> ProjectSyncResult {
        try synchronize(
            projectId: projectId,
            direction: resolution == .keepInternal ? .internalToExternal : .externalToInternal,
            resolution: resolution
        )
    }

    private func synchronize(
        projectId: String,
        direction: ProjectSyncDirection,
        resolution: ProjectConflictResolution?
    ) throws -> ProjectSyncResult {
        let snapshot = try repository.project(projectId: projectId)
        guard snapshot.record.storageKind == .externalBookmarkMirror else {
            throw ProjectSynchronizerError.internalProjectsHaveNoExternalDirectory
        }
        guard let bookmark = snapshot.record.sourceBookmark else {
            throw ProjectSynchronizerError.missingBookmark
        }

        let resolved: ResolvedProjectBookmark
        do {
            resolved = try bookmarkResolver.resolve(bookmark)
        } catch {
            let lost = try repository.updateProject(snapshot.record.with(syncState: .authorizationLost))
            return ProjectSyncResult(project: lost, conflicts: [], copiedFiles: 0, skippedFiles: 0)
        }

        var syncing = snapshot.record
        if resolved.isStale {
            var refreshed = try bookmarkResolver.refreshBookmark(for: resolved)
            refreshed.isStale = false
            syncing.sourceBookmark = refreshed
        }
        syncing.syncState = .syncing
        syncing.sourceBookmark?.isStale = false
        _ = try repository.updateProject(syncing)

        do {
            let externalRoot = try requireCanonicalDirectory(resolved.url)
            let internalRoot = snapshot.workspace.hostURL
            let internalFiles = try scanDirectory(root: internalRoot, enforceWorkspaceSafety: true)
            let baseline = try repository.readBaseline(projectId: projectId)
            var nextBaseline = baseline.files
            var conflicts: [ProjectSyncConflict] = []
            var copied = 0
            var skipped = 0

            if direction == .internalToExternal {
                try coordinator.coordinateWrite(at: externalRoot) { root in
                    try self.recoverExternalTransactions(root: root)
                    // The external snapshot must be taken while holding the
                    // write coordination. Scanning before this block leaves a
                    // TOCTOU window where a document provider can save a newer
                    // version that we then overwrite using stale conflict data.
                    let externalFiles = try self.scanDirectory(
                        root: root,
                        enforceWorkspaceSafety: false
                    )
                    let allPaths = Set(externalFiles.keys)
                        .union(internalFiles.keys)
                        .union(baseline.files.keys)
                    for path in allPaths.sorted() {
                        let externalFile = externalFiles[path]
                        let internalFile = internalFiles[path]
                        let baselineHash = baseline.files[path]?.sha256
                        switch decideProjectSyncAction(
                            direction: direction,
                            resolution: resolution,
                            baselineSha256: baselineHash,
                            externalSha256: externalFile?.sha256,
                            internalSha256: internalFile?.sha256
                        ) {
                        case .conflict:
                            conflicts.append(ProjectSyncConflict(
                                projectId: projectId,
                                relativePath: path,
                                internalSha256: internalFile?.sha256,
                                externalSha256: externalFile?.sha256,
                                baselineSha256: baselineHash
                            ))
                            skipped += 1
                        case .copyInternal:
                            guard let internalFile else { continue }
                            try self.writeExternal(root: root, relativePath: path, source: internalFile, workspaceRoot: internalRoot)
                            nextBaseline[path] = ProjectSyncFile(relativePath: path, sha256: internalFile.sha256, sizeBytes: internalFile.sizeBytes)
                            copied += 1
                        case .deleteExternal:
                            try self.deleteExternal(root: root, relativePath: path)
                            nextBaseline.removeValue(forKey: path)
                        case .deleteInternal:
                            break
                        case .updateBaseline:
                            if let hash = externalFile?.sha256 ?? internalFile?.sha256,
                               let size = externalFile?.sizeBytes ?? internalFile?.sizeBytes {
                                nextBaseline[path] = ProjectSyncFile(relativePath: path, sha256: hash, sizeBytes: size)
                            }
                        case .removeBaseline:
                            try self.assertSafeExternalMutationPath(root: root, relativePath: path)
                            nextBaseline.removeValue(forKey: path)
                        case .skip:
                            skipped += 1
                        case .copyExternal:
                            break
                        }
                    }
                }
            } else {
                try coordinator.coordinateRead(at: externalRoot) { root in
                    let externalFiles = try self.scanDirectory(
                        root: root,
                        enforceWorkspaceSafety: false
                    )
                    let allPaths = Set(externalFiles.keys)
                        .union(internalFiles.keys)
                        .union(baseline.files.keys)
                    for path in allPaths.sorted() {
                        let externalFile = externalFiles[path]
                        let internalFile = internalFiles[path]
                        let baselineHash = baseline.files[path]?.sha256
                        switch decideProjectSyncAction(
                            direction: direction,
                            resolution: resolution,
                            baselineSha256: baselineHash,
                            externalSha256: externalFile?.sha256,
                            internalSha256: internalFile?.sha256
                        ) {
                        case .conflict:
                            conflicts.append(ProjectSyncConflict(
                                projectId: projectId,
                                relativePath: path,
                                internalSha256: internalFile?.sha256,
                                externalSha256: externalFile?.sha256,
                                baselineSha256: baselineHash
                            ))
                            skipped += 1
                        case .copyExternal:
                            guard let externalFile else { continue }
                            try copyExternalToInternal(source: externalFile, workspaceRoot: internalRoot, relativePath: path)
                            nextBaseline[path] = ProjectSyncFile(relativePath: path, sha256: externalFile.sha256, sizeBytes: externalFile.sizeBytes)
                            copied += 1
                        case .deleteInternal:
                            try deleteInternal(workspaceRoot: internalRoot, relativePath: path)
                            nextBaseline.removeValue(forKey: path)
                        case .deleteExternal:
                            break
                        case .updateBaseline:
                            if let hash = externalFile?.sha256 ?? internalFile?.sha256,
                               let size = externalFile?.sizeBytes ?? internalFile?.sizeBytes {
                                nextBaseline[path] = ProjectSyncFile(relativePath: path, sha256: hash, sizeBytes: size)
                            }
                        case .removeBaseline:
                            nextBaseline.removeValue(forKey: path)
                        case .skip:
                            skipped += 1
                        case .copyInternal:
                            break
                        }
                    }
                }
            }

            try repository.writeBaseline(projectId: projectId, baseline: ProjectSyncBaseline(files: nextBaseline))
            var current = try repository.project(projectId: projectId).record
            current.lastSyncAt = now()
            current.sourceBookmark?.isStale = false
            if !conflicts.isEmpty {
                current.syncState = .conflict
            } else if skipped > 0 {
                current.syncState = .changesPending
            } else {
                current.syncState = .synced
            }
            let updated = try repository.updateProject(current)
            return ProjectSyncResult(project: updated, conflicts: conflicts, copiedFiles: copied, skippedFiles: skipped)
        } catch {
            var failed = syncing
            failed.syncState = .error
            _ = try repository.updateProject(failed)
            throw error
        }
    }

    private func scanDirectory(root: URL, enforceWorkspaceSafety: Bool) throws -> [String: ScannedFile] {
        let canonicalRoot = try requireCanonicalDirectory(root)
        var files: [String: ScannedFile] = [:]
        var stack = [canonicalRoot]
        var budget = SyncBudget()

        while let directory = stack.popLast() {
            let depth = directory.pathComponents.count - canonicalRoot.pathComponents.count
            guard depth <= 64 else { throw ProjectSynchronizerError.directoryDepthExceeded }
            let entries = try FileManager.default.contentsOfDirectory(
                at: directory,
                includingPropertiesForKeys: [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey],
                options: []
            )
            for entry in entries {
                try budget.includeEntry()
                let name = entry.lastPathComponent
                if isLingxiSyncArtifact(name) { continue }
                let values = try entry.resourceValues(forKeys: [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey])
                if values.isSymbolicLink == true { continue }
                if enforceWorkspaceSafety && !isSafeProjectWorkspacePath(root: canonicalRoot, candidate: entry) {
                    continue
                }
                if values.isDirectory == true {
                    stack.append(entry)
                } else if values.isRegularFile == true {
                    let relativePath = relativePath(from: canonicalRoot, to: entry)
                    guard isSafeRelativePath(relativePath) else { continue }
                    let hashed = try hashFile(at: entry)
                    try budget.includeFile(hashed.sizeBytes)
                    files[relativePath] = ScannedFile(url: entry, sha256: hashed.sha256, sizeBytes: hashed.sizeBytes)
                }
            }
        }
        return files
    }

    private func copyExternalToInternal(source: ScannedFile, workspaceRoot: URL, relativePath: String) throws {
        guard isSafeRelativePath(relativePath) else { throw ProjectSynchronizerError.unsafeRelativePath(relativePath) }
        let root = try requireCanonicalDirectory(workspaceRoot)
        let target = root.appendingPathComponent(relativePath)
        guard isSafeProjectWorkspacePath(root: root, candidate: target) else {
            throw ProjectSynchronizerError.copyEscapedWorkspace
        }
        let capacity = try root.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
            .volumeAvailableCapacityForImportantUsage ?? 0
        guard Int64(capacity) >= source.sizeBytes + minFreeSpaceBytes else {
            throw ProjectSynchronizerError.notEnoughFreeSpace(relativePath)
        }
        try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
        let temp = target.deletingLastPathComponent().appendingPathComponent(".\(target.lastPathComponent).\(UUID().uuidString).tmp")
        do {
            let input = try makeInputStream(at: source.url, error: .cannotReadExternal(relativePath))
            defer { input.close() }
            try copyAndVerify(input: input, to: temp, expected: source, writeError: .cannotWriteInternal(target.lastPathComponent))
            guard isSafeProjectWorkspacePath(root: root, candidate: target) else {
                throw ProjectSynchronizerError.copyEscapedWorkspace
            }
            try replaceItem(source: temp, target: target)
        } catch {
            try? FileManager.default.removeItem(at: temp)
            throw error
        }
    }

    private func writeExternal(root: URL, relativePath: String, source: ScannedFile, workspaceRoot: URL) throws {
        guard isSafeRelativePath(relativePath) else { throw ProjectSynchronizerError.unsafeRelativePath(relativePath) }
        let canonicalWorkspaceRoot = try requireCanonicalDirectory(workspaceRoot)
        guard isSafeProjectWorkspacePath(root: canonicalWorkspaceRoot, candidate: source.url) else {
            throw ProjectSynchronizerError.sourceEscapedWorkspace
        }
        let canonicalRoot = try requireCanonicalDirectory(root)
        try assertSafeExternalMutationPath(root: canonicalRoot, relativePath: relativePath)
        let target = canonicalRoot.appendingPathComponent(relativePath)
        try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
        try assertSafeExternalMutationPath(root: canonicalRoot, relativePath: relativePath)
        try commitExternalFile(
            root: canonicalRoot,
            relativePath: relativePath,
            expectedSha256: source.sha256
        ) {
            try makeInputStream(at: source.url, error: .cannotReadInternal(relativePath))
        }
    }

    private func deleteInternal(workspaceRoot: URL, relativePath: String) throws {
        guard isSafeRelativePath(relativePath) else { throw ProjectSynchronizerError.unsafeRelativePath(relativePath) }
        let root = try requireCanonicalDirectory(workspaceRoot)
        let target = root.appendingPathComponent(relativePath)
        guard isSafeProjectWorkspacePath(root: root, candidate: target) else {
            throw ProjectSynchronizerError.copyEscapedWorkspace
        }
        try deleteFileIfPresent(at: target, root: root)
    }

    private func deleteExternal(root: URL, relativePath: String) throws {
        guard isSafeRelativePath(relativePath) else { throw ProjectSynchronizerError.unsafeRelativePath(relativePath) }
        let canonicalRoot = try requireCanonicalDirectory(root)
        try assertSafeExternalMutationPath(root: canonicalRoot, relativePath: relativePath)
        let target = canonicalRoot.appendingPathComponent(relativePath)
        try deleteFileIfPresent(at: target, root: canonicalRoot)
    }

    private func assertSafeExternalMutationPath(root: URL, relativePath: String) throws {
        let target = root.appendingPathComponent(relativePath)
        guard isSafeProjectWorkspacePath(root: root, candidate: target) else {
            throw ProjectSynchronizerError.copyEscapedWorkspace
        }
        let parent = target.deletingLastPathComponent()
        guard parent == root || isSafeProjectWorkspacePath(root: root, candidate: parent) else {
            throw ProjectSynchronizerError.copyEscapedWorkspace
        }
    }

    private func recoverExternalTransactions(root: URL) throws {
        guard let enumerator = FileManager.default.enumerator(
            at: root,
            includingPropertiesForKeys: [.isDirectoryKey, .isRegularFileKey],
            options: []
        ) else { return }

        var backupsByDirectory: [URL: [(String, URL)]] = [:]
        var tempFiles: [URL] = []
        for case let fileURL as URL in enumerator {
            let values = try fileURL.resourceValues(forKeys: [.isDirectoryKey, .isRegularFileKey])
            guard values.isDirectory != true else { continue }
            let name = fileURL.lastPathComponent
            if let original = syncArtifactOriginalName(name: name, marker: syncBackupMarker) {
                backupsByDirectory[fileURL.deletingLastPathComponent(), default: []].append((original, fileURL))
            } else if syncArtifactOriginalName(name: name, marker: syncTempMarker) != nil {
                tempFiles.append(fileURL)
            }
        }

        for (directory, backups) in backupsByDirectory {
            let names = Set((try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? [])
            for (originalName, backupURL) in backups {
                let originalURL = directory.appendingPathComponent(originalName)
                if names.contains(originalName) {
                    try? FileManager.default.removeItem(at: backupURL)
                } else {
                    try? replaceItem(source: backupURL, target: originalURL)
                }
            }
        }
        tempFiles.forEach { try? FileManager.default.removeItem(at: $0) }
    }

    private func commitExternalFile(
        root: URL,
        relativePath: String,
        expectedSha256: String,
        source: () throws -> InputStream
    ) throws {
        try assertSafeExternalMutationPath(root: root, relativePath: relativePath)
        let target = root.appendingPathComponent(relativePath)
        let displayName = target.lastPathComponent
        let parent = target.deletingLastPathComponent()
        let temp = parent.appendingPathComponent("\(displayName)\(syncTempMarker)\(UUID().uuidString)")
        let backup = parent.appendingPathComponent("\(displayName)\(syncBackupMarker)\(UUID().uuidString)")
        let fm = FileManager.default
        let targetExists = fm.fileExists(atPath: target.path)
        var committed = false
        var hasBackup = false
        do {
            let input = try source()
            defer { input.close() }
            try copy(input: input, to: temp, writeError: .cannotWriteExternal(displayName))
            let verified = try hashFile(at: temp)
            guard verified.sha256 == expectedSha256 else {
                throw ProjectSynchronizerError.externalDataMutated(displayName)
            }
            try assertSafeExternalMutationPath(root: root, relativePath: relativePath)
            if targetExists {
                try replaceItem(source: target, target: backup)
                hasBackup = true
            }
            do {
                try hooks.beforePromoteTemp()
                try assertSafeExternalMutationPath(root: root, relativePath: relativePath)
                try replaceItem(source: temp, target: target)
                committed = true
                if hasBackup { try? fm.removeItem(at: backup) }
            } catch {
                if hasBackup { try? replaceItem(source: backup, target: target) }
                throw error
            }
        } catch {
            if !committed, hasBackup {
                try? replaceItem(source: backup, target: target)
            }
            try? fm.removeItem(at: temp)
            try? fm.removeItem(at: backup)
            throw error
        }
        try? fm.removeItem(at: temp)
        try? fm.removeItem(at: backup)
    }
}

func isLingxiSyncArtifact(_ name: String) -> Bool {
    syncArtifactOriginalName(name: name, marker: syncTempMarker) != nil ||
        syncArtifactOriginalName(name: name, marker: syncBackupMarker) != nil
}

private func syncArtifactOriginalName(name: String, marker: String) -> String? {
    guard let markerRange = name.range(of: marker, options: .backwards) else { return nil }
    let original = String(name[..<markerRange.lowerBound])
    let transactionID = String(name[markerRange.upperBound...])
    guard !original.isEmpty, UUID(uuidString: transactionID) != nil else { return nil }
    return original
}

private func relativePath(from root: URL, to child: URL) -> String {
    let rootComponents = root.standardizedFileURL.pathComponents
    let childComponents = child.standardizedFileURL.pathComponents
    guard childComponents.count > rootComponents.count,
          Array(childComponents.prefix(rootComponents.count)) == rootComponents
    else { return "" }
    return childComponents
        .dropFirst(rootComponents.count)
        .joined(separator: "/")
        .replacingOccurrences(of: "\\", with: "/")
}

private func makeInputStream(at url: URL, error: ProjectSynchronizerError) throws -> InputStream {
    guard let stream = InputStream(url: url) else { throw error }
    return stream
}

private func copyAndVerify(
    input: InputStream,
    to target: URL,
    expected: ScannedFile,
    writeError: ProjectSynchronizerError
) throws {
    let verified = try copy(input: input, to: target, writeError: writeError, hash: true)
    guard verified.sha256 == expected.sha256, verified.sizeBytes == expected.sizeBytes else {
        throw ProjectSynchronizerError.externalDataMutated(target.lastPathComponent)
    }
}

@discardableResult
private func copy(
    input: InputStream,
    to target: URL,
    writeError: ProjectSynchronizerError,
    hash: Bool = false
) throws -> (sha256: String, sizeBytes: Int64) {
    input.open()
    defer { input.close() }
    guard let output = OutputStream(url: target, append: false) else { throw writeError }
    output.open()
    defer { output.close() }

    var digest = SHA256()
    var total: Int64 = 0
    let bufferSize = 64 * 1024
    let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: bufferSize)
    defer { buffer.deallocate() }

    while input.hasBytesAvailable {
        let read = input.read(buffer, maxLength: bufferSize)
        if read < 0 { throw input.streamError ?? CocoaError(.fileReadUnknown) }
        if read == 0 { break }
        total += Int64(read)
        guard total <= maxSyncFileBytes else { throw ProjectSynchronizerError.fileTooLarge }
        if hash {
            digest.update(data: Data(bytes: buffer, count: read))
        }
        var written = 0
        while written < read {
            let result = output.write(buffer.advanced(by: written), maxLength: read - written)
            if result < 0 { throw output.streamError ?? CocoaError(.fileWriteUnknown) }
            if result == 0 { throw CocoaError(.fileWriteUnknown) }
            written += result
        }
    }

    return (hash ? digest.finalize().hexString : "", total)
}

private func hashFile(at url: URL) throws -> (sha256: String, sizeBytes: Int64) {
    let input = try makeInputStream(at: url, error: .cannotReadExternal(url.lastPathComponent))
    input.open()
    defer { input.close() }

    var digest = SHA256()
    var total: Int64 = 0
    let bufferSize = 64 * 1024
    let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: bufferSize)
    defer { buffer.deallocate() }

    while input.hasBytesAvailable {
        let read = input.read(buffer, maxLength: bufferSize)
        if read < 0 { throw input.streamError ?? CocoaError(.fileReadUnknown) }
        if read == 0 { break }
        total += Int64(read)
        guard total <= maxSyncFileBytes else { throw ProjectSynchronizerError.fileTooLarge }
        digest.update(data: Data(bytes: buffer, count: read))
    }

    return (digest.finalize().hexString, total)
}

private func replaceItem(source: URL, target: URL) throws {
    let fm = FileManager.default
    if fm.fileExists(atPath: target.path) {
        _ = try fm.replaceItemAt(target, withItemAt: source)
    } else {
        try fm.moveItem(at: source, to: target)
    }
}

private func deleteFileIfPresent(at target: URL, root: URL) throws {
    let fm = FileManager.default
    guard fm.fileExists(atPath: target.path) else { return }
    let values = try target.resourceValues(forKeys: [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey])
    if values.isDirectory == true {
        throw ProjectSynchronizerError.pathTypeConflict(target.lastPathComponent)
    }
    if values.isSymbolicLink == true {
        throw ProjectSynchronizerError.pathTypeConflict(target.lastPathComponent)
    }
    try fm.removeItem(at: target)
    try pruneEmptyParents(startingAt: target.deletingLastPathComponent(), root: root)
}

private func pruneEmptyParents(startingAt directory: URL, root: URL) throws {
    let fm = FileManager.default
    var current = directory
    while current.standardizedFileURL != root.standardizedFileURL {
        let contents = try fm.contentsOfDirectory(atPath: current.path)
        if contents.isEmpty {
            try fm.removeItem(at: current)
            current = current.deletingLastPathComponent()
        } else {
            break
        }
    }
}

private extension SHA256Digest {
    var hexString: String {
        map { String(format: "%02x", $0) }.joined()
    }
}

enum ProjectSynchronizerError: LocalizedError {
    case internalProjectsHaveNoExternalDirectory
    case missingBookmark
    case unsafeRelativePath(String)
    case directoryDepthExceeded
    case copyEscapedWorkspace
    case sourceEscapedWorkspace
    case notEnoughFreeSpace(String)
    case cannotReadExternal(String)
    case cannotReadInternal(String)
    case cannotWriteInternal(String)
    case cannotWriteExternal(String)
    case externalDataMutated(String)
    case tooManyEntries(Int)
    case fileTooLarge
    case totalSizeTooLarge
    case pathTypeConflict(String)

    var errorDescription: String? {
        switch self {
        case .internalProjectsHaveNoExternalDirectory:
            return "internal projects do not have an external directory"
        case .missingBookmark:
            return "project is missing its bookmarked external directory"
        case .unsafeRelativePath(let path):
            return "unsafe relative path: \(path)"
        case .directoryDepthExceeded:
            return "directory nesting exceeds the supported limit"
        case .copyEscapedWorkspace:
            return "copy escaped project workspace"
        case .sourceEscapedWorkspace:
            return "source escaped project workspace"
        case .notEnoughFreeSpace(let path):
            return "not enough free space to import \(path)"
        case .cannotReadExternal(let path):
            return "cannot read external file: \(path)"
        case .cannotReadInternal(let path):
            return "cannot read internal file: \(path)"
        case .cannotWriteInternal(let path):
            return "cannot write internal file: \(path)"
        case .cannotWriteExternal(let path):
            return "cannot write external file: \(path)"
        case .externalDataMutated(let name):
            return "external file changed while syncing \(name)"
        case .tooManyEntries(let limit):
            return "project contains more than \(limit) entries"
        case .fileTooLarge:
            return "project contains a file larger than the supported sync limit"
        case .totalSizeTooLarge:
            return "project exceeds the supported sync size"
        case .pathTypeConflict(let path):
            return "path type conflicts with \(path)"
        }
    }
}
