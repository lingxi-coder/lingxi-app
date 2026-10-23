import SwiftUI
import UniformTypeIdentifiers
import CryptoKit

#if canImport(harness_runtimeFFI)
/// Original-file transfers have a separate picker from camera/image compression.
@MainActor
struct RemoteFilesView: View {
    let manager: RemoteControlManager
    let hostID: String
    let sessionID: String
    let enabled: Bool
    @State private var files: [RemoteFileFfi] = []
    @State private var picking = false
    @State private var expanded = false
    @State private var busy = false
    @State private var status = ""
    @State private var problem: String?
    @State private var selected: URL?
    @State private var prepared: RemoteFileFfi?
    @State private var transferID: String?
    @State private var total: UInt64 = 0
    @State private var committed: UInt64 = 0
    @State private var exported: URL?
    @State private var operation: Task<Void, Never>?

    var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Button("remote_file_upload") { picking = true }.disabled(!enabled || busy)
                    if selected != nil && !busy { Button("common_retry") { upload() }.disabled(!enabled) }
                    if busy { Button("common_cancel") { cancel() } }
                }
                if busy {
                    ProgressView(value: Double(committed), total: Double(max(total, 1)))
                    Text(status).font(.caption)
                    Text("\(committed.formatted()) / \(total.formatted()) B").font(.caption.monospacedDigit())
                }
                if let problem { Text(problem).font(.caption).foregroundStyle(.red).textSelection(.enabled) }
                ForEach(files, id: \.transferId) { file in
                    HStack {
                        Text(file.name).lineLimit(1)
                        Spacer()
                        Button("remote_file_attach") { send(file) }.disabled(!enabled || busy)
                        Button("remote_file_download") { download(file) }.disabled(!enabled || busy)
                    }.font(.caption)
                }
                if let exported { ShareLink(item: exported) { Label("remote_file_export", systemImage: "square.and.arrow.up") } }
                if !enabled { Text("remote_file_requires_webrtc").font(.caption).foregroundStyle(.secondary) }
            }.padding(.top, 8)
        } label: { Label("remote_files", systemImage: "doc") }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .fileImporter(isPresented: $picking, allowedContentTypes: [.data], allowsMultipleSelection: false) { result in
            guard case let .success(urls) = result, let url = urls.first else { return }
            operation = Task {
                busy = true; problem = nil; status = String(localized: "remote_file_preparing")
                do {
                    let destination = try cacheRoot().appendingPathComponent(UUID().uuidString, isDirectory: true)
                    try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
                    let target = destination.appendingPathComponent(url.lastPathComponent)
                    let copy = Task.detached(priority: .utility) {
                        let scoped = url.startAccessingSecurityScopedResource()
                        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                        guard FileManager.default.createFile(atPath: target.path, contents: nil) else { throw CocoaError(.fileWriteUnknown) }
                        let input = try FileHandle(forReadingFrom: url)
                        let output = try FileHandle(forWritingTo: target)
                        defer { try? input.close(); try? output.close() }
                        do {
                            while let bytes = try input.read(upToCount: 64 * 1024), !bytes.isEmpty {
                                try Task.checkCancellation()
                                try output.write(contentsOf: bytes)
                            }
                        } catch { try? FileManager.default.removeItem(at: destination); throw error }
                    }
                    try await withTaskCancellationHandler { try await copy.value } onCancel: { copy.cancel() }
                    try Task.checkCancellation()
                    selected = target; prepared = nil; busy = false; upload()
                } catch { problem = error.localizedDescription; busy = false }
            }
        }
        .onDisappear { operation?.cancel() }
        .task(id: enabled) { if enabled { await refresh() } }
        .task(id: transferID) {
            guard let id = transferID else { return }
            while !Task.isCancelled && busy {
                if let client = try? manager.activeClient() { committed = client.fileProgress(transferId: id) }
                try? await Task.sleep(for: .milliseconds(300))
            }
        }
    }
    private func cacheRoot() throws -> URL {
        let scope = "\(manager.uid ?? "")\0\(hostID)\0\(sessionID)"
        let key = SHA256.hash(data: Data(scope.utf8)).map { String(format: "%02x", $0) }.joined()
        let root = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("RemoteFiles", isDirectory: true).appendingPathComponent(key, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }
    private func refresh() async {
        do { files = try await manager.activeClient().listFiles(sessionId: sessionID) }
        catch { problem = error.localizedDescription }
    }
    private func upload() {
        guard let selected else { return }
        operation = Task {
            busy = true; problem = nil; committed = 0; status = String(localized: "remote_file_preparing")
            defer { busy = false; transferID = nil }
            do {
                let client = try manager.activeClient()
                transferID = selected.path
                let file: RemoteFileFfi
                if let prepared { file = prepared }
                else { file = try await client.prepareFile(sessionId: sessionID, path: selected.path, name: selected.lastPathComponent); prepared = file }
                transferID = file.transferId; total = file.size; status = String(localized: "remote_file_uploading")
                let receipt = try await client.uploadFile(file: file, path: selected.path)
                files.removeAll { $0.transferId == receipt.transferId }; files.append(receipt)
                self.selected = nil; prepared = nil
                try? FileManager.default.removeItem(at: selected.deletingLastPathComponent())
            } catch { problem = error.localizedDescription }
        }
    }
    private func download(_ file: RemoteFileFfi) {
        operation = Task {
            busy = true; problem = nil; committed = 0; total = file.size; transferID = file.transferId
            status = String(localized: "remote_file_downloading")
            defer { busy = false; transferID = nil }
            do {
                guard file.name == (file.name as NSString).lastPathComponent, file.name != ".", file.name != ".." else { throw CocoaError(.fileReadInvalidFileName) }
                let directory = try cacheRoot().appendingPathComponent(file.transferId, isDirectory: true)
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                let path = directory.appendingPathComponent(file.name)
                _ = try await manager.activeClient().downloadFile(file: file, path: path.path)
                exported = path
            } catch { problem = error.localizedDescription }
        }
    }
    private func send(_ file: RemoteFileFfi) {
        operation = Task {
            do { try await manager.activeClient().sendPromptWithFiles(sessionId: sessionID, text: file.name, images: [], fileRefs: [file.fileRef]) }
            catch { problem = error.localizedDescription }
        }
    }
    private func cancel() {
        if let transferID { try? manager.activeClient().cancelFile(transferId: transferID) }
        else { operation?.cancel() }
        // Keep the source selection so Retry can reuse its durable transfer id.
    }
}
#endif
