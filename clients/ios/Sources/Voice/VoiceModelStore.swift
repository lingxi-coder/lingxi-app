import CryptoKit
import Foundation
import Observation

enum VoiceModelState: Equatable, Sendable {
    case notInstalled
    case queued
    case downloading(receivedBytes: Int64, totalBytes: Int64)
    case verifying
    case extracting
    case ready
    case failed(String)

    var isReady: Bool {
        if case .ready = self { return true }
        return false
    }
}

enum VoiceModelFiles {
    static let rootDirectory: URL = {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return base.appending(path: "VoiceModels", directoryHint: .isDirectory)
    }()

    static func modelDirectory(_ id: String) -> URL {
        rootDirectory.appending(path: id, directoryHint: .isDirectory)
    }

    static func modelRoot(for entry: GeneratedOfflineModelEntry) -> URL? {
        findModelRoot(in: modelDirectory(entry.id), entry: entry)
    }

    static func isReady(_ entry: GeneratedOfflineModelEntry) -> Bool {
        modelRoot(for: entry) != nil
    }

    static func findModelRoot(in root: URL, entry: GeneratedOfflineModelEntry) -> URL? {
        let fileManager = FileManager.default
        guard fileManager.fileExists(atPath: root.path) else { return nil }
        let candidates = [root] + ((try? fileManager.contentsOfDirectory(
            at: root,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        )) ?? []).filter { (try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true }
        return candidates.first { candidate in
            entry.files.allSatisfy { fileManager.fileExists(atPath: candidate.appending(path: $0).path) }
                && entry.requiredDirectories.allSatisfy {
                    var isDirectory: ObjCBool = false
                    return fileManager.fileExists(
                        atPath: candidate.appending(path: $0).path,
                        isDirectory: &isDirectory
                    ) && isDirectory.boolValue
                }
        }
    }
}

@Observable
@MainActor
final class VoiceModelStore {
    static let shared = VoiceModelStore()

    private(set) var states: [String: VoiceModelState] = [:]
    private var tasks: [String: Task<Void, Never>] = [:]

    init() {
        prepareRootDirectory()
        reconcileFromDisk()
    }

    func state(for modelID: String) -> VoiceModelState {
        states[modelID] ?? .notInstalled
    }

    func recognitionModel(for languageIdentifier: String) -> GeneratedOfflineModelEntry? {
        let language = Self.languageBase(languageIdentifier)
        return GeneratedVoiceModelCatalog.packFor(language).first { $0.kind == .stt }
    }

    func speechModels(for languageIdentifier: String) -> [GeneratedOfflineModelEntry] {
        let language = Self.languageBase(languageIdentifier)
        return GeneratedVoiceModelCatalog.packFor(language).filter { $0.kind == .tts }
    }

    func isRecognitionReady(languageIdentifier: String) -> Bool {
        recognitionModel(for: languageIdentifier).map { state(for: $0.id).isReady } == true
    }

    func downloadPack(languageIdentifier: String) {
        let language = Self.languageBase(languageIdentifier)
        for entry in GeneratedVoiceModelCatalog.packFor(language) {
            download(entry)
        }
    }

    func download(_ entry: GeneratedOfflineModelEntry) {
        guard tasks[entry.id] == nil, !state(for: entry.id).isReady else { return }
        states[entry.id] = .queued
        let task = Task { [weak self] in
            guard let self else { return }
            await self.performDownload(entry)
        }
        tasks[entry.id] = task
    }

    func cancel(_ modelID: String) {
        tasks.removeValue(forKey: modelID)?.cancel()
        if !(states[modelID]?.isReady ?? false) {
            states[modelID] = .notInstalled
        }
    }

    func remove(_ entry: GeneratedOfflineModelEntry) {
        cancel(entry.id)
        try? FileManager.default.removeItem(at: VoiceModelFiles.modelDirectory(entry.id))
        states[entry.id] = .notInstalled
    }

    func reconcileFromDisk() {
        states = Dictionary(uniqueKeysWithValues: GeneratedVoiceModelCatalog.all.map {
            let existing = states[$0.id]
            let active: Bool = switch existing {
            case .queued?, .downloading?, .verifying?, .extracting?: true
            default: false
            }
            return (
                $0.id,
                active ? existing! : (VoiceModelFiles.isReady($0) ? .ready : .notInstalled)
            )
        })
    }

    private func performDownload(_ entry: GeneratedOfflineModelEntry) async {
        defer { tasks[entry.id] = nil }
        do {
            guard let sourceURL = URL(string: entry.sourceURL) else {
                throw VoiceModelInstallError.invalidURL
            }
            states[entry.id] = .downloading(receivedBytes: 0, totalBytes: entry.approxSizeBytes)
            let archiveURL = VoiceModelFiles.rootDirectory.appending(
                path: ".download-\(entry.id).tar.bz2"
            )
            let resumeDataURL = VoiceModelFiles.rootDirectory.appending(
                path: ".download-\(entry.id).resume"
            )
            let downloader = ResumableVoiceModelDownload(
                destinationURL: archiveURL,
                resumeDataURL: resumeDataURL
            ) { [weak self] received, total in
                Task { @MainActor [weak self] in
                    self?.states[entry.id] = .downloading(
                        receivedBytes: received,
                        totalBytes: max(total, entry.approxSizeBytes)
                    )
                }
            }
            let (temporaryURL, response) = try await downloader.run(url: sourceURL)
            try Task.checkCancellation()
            guard let response = response as? HTTPURLResponse, 200 ..< 300 ~= response.statusCode else {
                throw VoiceModelInstallError.downloadFailed
            }
            states[entry.id] = .verifying
            let digest = try await Self.sha256(of: temporaryURL)
            guard digest == entry.sha256 else { throw VoiceModelInstallError.checksumMismatch }
            try Task.checkCancellation()
            states[entry.id] = .extracting
            let staging = VoiceModelFiles.rootDirectory.appending(
                path: ".stage-\(entry.id)-\(UUID().uuidString)",
                directoryHint: .isDirectory
            )
            try FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: staging) }
            let extracted = try await Self.extractArchive(
                temporaryURL: temporaryURL,
                staging: staging
            )
            guard extracted,
                  let modelRoot = VoiceModelFiles.findModelRoot(in: staging, entry: entry)
            else { throw VoiceModelInstallError.invalidArchive }
            try Task.checkCancellation()
            let finalURL = VoiceModelFiles.modelDirectory(entry.id)
            let activationURL = VoiceModelFiles.rootDirectory.appending(
                path: ".activate-\(entry.id)-\(UUID().uuidString)",
                directoryHint: .isDirectory
            )
            try FileManager.default.moveItem(at: modelRoot, to: activationURL)
            defer { try? FileManager.default.removeItem(at: activationURL) }
            if FileManager.default.fileExists(atPath: finalURL.path) {
                _ = try FileManager.default.replaceItemAt(
                    finalURL,
                    withItemAt: activationURL,
                    backupItemName: nil,
                    options: []
                )
            } else {
                try FileManager.default.moveItem(at: activationURL, to: finalURL)
            }
            guard VoiceModelFiles.isReady(entry) else { throw VoiceModelInstallError.invalidArchive }
            states[entry.id] = .ready
            try? FileManager.default.removeItem(at: archiveURL)
            try? FileManager.default.removeItem(at: resumeDataURL)
        } catch is CancellationError {
            states[entry.id] = .notInstalled
        } catch let error as URLError where error.code == .cancelled {
            states[entry.id] = .notInstalled
        } catch {
            states[entry.id] = .failed(error.localizedDescription)
        }
    }

    private func prepareRootDirectory() {
        try? FileManager.default.createDirectory(
            at: VoiceModelFiles.rootDirectory,
            withIntermediateDirectories: true
        )
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        var root = VoiceModelFiles.rootDirectory
        try? root.setResourceValues(values)
    }

    private nonisolated static func languageBase(_ identifier: String) -> String {
        identifier.replacingOccurrences(of: "_", with: "-")
            .split(separator: "-").first.map(String.init)?.lowercased() ?? ""
    }

    private nonisolated static func sha256(of url: URL) async throws -> String {
        try await Task.detached(priority: .utility) {
            let handle = try FileHandle(forReadingFrom: url)
            defer { try? handle.close() }
            var hash = SHA256()
            while true {
                try Task.checkCancellation()
                let data = try handle.read(upToCount: 1024 * 1024) ?? Data()
                if data.isEmpty { break }
                hash.update(data: data)
            }
            return hash.finalize().map { String(format: "%02x", $0) }.joined()
        }.value
    }

    private nonisolated static func extractArchive(
        temporaryURL: URL,
        staging: URL
    ) async throws -> Bool {
        try await Task.detached(priority: .utility) {
            try Task.checkCancellation()
            let extracted = temporaryURL.path.withCString { archive in
                staging.path.withCString { destination in
                    LXExtractTarBz2(archive, destination)
                }
            }
            try Task.checkCancellation()
            return extracted == 1
        }.value
    }
}

private final class ResumableVoiceModelDownload: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    typealias Progress = @Sendable (Int64, Int64) -> Void

    private let lock = NSLock()
    private let destinationURL: URL
    private let resumeDataURL: URL
    private let progress: Progress
    private var continuation: CheckedContinuation<(URL, URLResponse), Error>?
    private var session: URLSession?
    private var task: URLSessionDownloadTask?
    private var response: URLResponse?
    private var finished = false

    init(destinationURL: URL, resumeDataURL: URL, progress: @escaping Progress) {
        self.destinationURL = destinationURL
        self.resumeDataURL = resumeDataURL
        self.progress = progress
    }

    func run(url: URL) async throws -> (URL, URLResponse) {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                lock.lock()
                self.continuation = continuation
                let session = URLSession(configuration: .default, delegate: self, delegateQueue: nil)
                self.session = session
                let resumeData = try? Data(contentsOf: resumeDataURL)
                let task = resumeData.map(session.downloadTask(withResumeData:))
                    ?? session.downloadTask(with: url)
                self.task = task
                lock.unlock()
                task.resume()
            }
        } onCancel: {
            cancel()
        }
    }

    func urlSession(
        _: URLSession,
        downloadTask: URLSessionDownloadTask,
        didWriteData bytesWritten: Int64,
        totalBytesWritten: Int64,
        totalBytesExpectedToWrite: Int64
    ) {
        progress(totalBytesWritten, totalBytesExpectedToWrite)
    }

    func urlSession(
        _: URLSession,
        downloadTask: URLSessionDownloadTask,
        didFinishDownloadingTo location: URL
    ) {
        do {
            try? FileManager.default.removeItem(at: destinationURL)
            try FileManager.default.moveItem(at: location, to: destinationURL)
            response = downloadTask.response
        } catch {
            finish(.failure(error))
        }
    }

    func urlSession(
        _: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: Error?
    ) {
        if let error {
            let nsError = error as NSError
            if let resumeData = nsError.userInfo[NSURLSessionDownloadTaskResumeData] as? Data {
                try? resumeData.write(to: resumeDataURL, options: .atomic)
            }
            finish(.failure(error))
            return
        }
        guard let response else {
            finish(.failure(VoiceModelInstallError.downloadFailed))
            return
        }
        finish(.success((destinationURL, response)))
    }

    private func cancel() {
        lock.lock()
        let task = task
        lock.unlock()
        task?.cancel(byProducingResumeData: { [resumeDataURL] resumeData in
            if let resumeData {
                try? resumeData.write(to: resumeDataURL, options: .atomic)
            }
        })
    }

    private func finish(_ result: Result<(URL, URLResponse), Error>) {
        lock.lock()
        guard !finished else {
            lock.unlock()
            return
        }
        finished = true
        let continuation = continuation
        self.continuation = nil
        let session = session
        self.session = nil
        task = nil
        lock.unlock()
        session?.finishTasksAndInvalidate()
        continuation?.resume(with: result)
    }
}

private enum VoiceModelInstallError: LocalizedError {
    case invalidURL
    case downloadFailed
    case checksumMismatch
    case invalidArchive

    var errorDescription: String? {
        switch self {
        case .invalidURL: "Invalid model download URL."
        case .downloadFailed: "The model download failed."
        case .checksumMismatch: "The downloaded model did not pass integrity verification."
        case .invalidArchive: "The model archive is incomplete or invalid."
        }
    }
}
