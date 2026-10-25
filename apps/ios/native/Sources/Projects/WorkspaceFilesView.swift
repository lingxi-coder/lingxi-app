import SwiftUI
import QuickLook
import UniformTypeIdentifiers
import ImageIO

// Separate catalog keeps this feature independent from the general app strings.
enum FilesStrings { static func text(_ key: String) -> String { String(localized: String.LocalizationValue(key), table: "Files") } }
enum FileBrowserError: LocalizedError {
    case message(String)
    var errorDescription: String? { switch self { case .message(let value): value } }
}

struct WorkspaceFilesView: View {
    let repository: WorkspaceDirectoryRepository
    let paths: WorkspaceDirectoryPaths
    let initialPath: String
    let protectedPaths: () -> [String]
    let canMutate: () -> Bool
    var selectionTitle: String? = nil
    var onSelect: ((String) async throws -> Void)? = nil
    @Environment(\.dismiss) private var dismiss
    @State private var path = ""
    @State private var entries: [WorkspaceDirectoryEntry] = []
    @State private var query = ""
    @State private var recursive = false
    @State private var hidden = false
    @State private var grid = false
    @State private var sort = WorkspaceFileSort.name
    @State private var selected: Set<String> = []
    @State private var selecting = false
    @State private var loading = true
    @State private var revision = 0
    @State private var error: String?
    @State private var name = ""
    @State private var nameDialog = false
    @State private var rename: WorkspaceDirectoryEntry?
    @State private var info: WorkspaceDirectoryEntry?
    @State private var deleting: [String] = []
    @State private var modal: FileModal?
    @State private var lastModal: FileModal?
    @State private var showConflict = false
    @State private var transfer: TransferRequest?
    @State private var conflictRequest: TransferRequest?
    @State private var shareSnapshots: [WorkspaceFileSnapshot] = []
    @State private var previewCache: WorkspaceFileSnapshot?
    @State private var previewPresented = false
    @State private var preview: WorkspaceFileSnapshot?
    @State private var job: Task<Void, Never>?
    @State private var progress: WorkspaceFileProgress?
    @State private var importFiles = false
    @State private var importFolder = false
    @State private var exportPaths: [String] = []
    @State private var systemPicker = WorkspaceSystemPickerCompletion()
    @State private var conflictImport = false
    private var importURLs: [URL] {
        get { systemPicker.urls }
        nonmutating set { systemPicker.urls = newValue }
    }
    private var destination: URL? {
        get { systemPicker.destination }
        nonmutating set { systemPicker.destination = newValue }
    }
    @State private var goTo = false
    @State private var enteredPath = ""

    private struct TransferRequest: Identifiable {
        let id = UUID(); let sources: [String]; let move: Bool; var destination: String?
    }
    private enum FileModal: Identifiable {
        case destination(TransferRequest), details(WorkspaceDirectoryEntry), preview(WorkspaceFileSnapshot), share([WorkspaceFileSnapshot])
        case importFiles, importFolder, exportDestination
        var id: String {
            switch self {
            case .destination(let request): "destination-" + request.id.uuidString
            case .details(let entry): "details-" + entry.path
            case .preview(let snapshot): "preview-" + snapshot.id.uuidString
            case .share(let snapshots): "share-" + (snapshots.first?.id.uuidString ?? "")
            case .importFiles: "import-files"
            case .importFolder: "import-folder"
            case .exportDestination: "export"
            }
        }
    }
    private var activePath: String { path.isEmpty ? initialPath : path }
    private var loadKey: String { activePath + "|\(hidden)|\(recursive)|\(query)|\(revision)|" + sort.rawValue }
    private var sortedEntries: [WorkspaceDirectoryEntry] { entries }
    private var busy: Bool { job != nil }
    private var mutablePaths: [String] { protectedPaths() + [activePath] }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                breadcrumbs
                if !query.isEmpty {
                    Toggle(FilesStrings.text("recursive"), isOn: $recursive).padding(.horizontal)
                }
                if let progress {
                    HStack {
                        VStack(alignment: .leading) {
                            Text(progress.name).lineLimit(1)
                            ProgressView(value: Double(progress.completed), total: Double(max(1, progress.total)))
                            Text("\(progress.completed) / \(progress.total)").font(.caption)
                        }
                        Button(FilesStrings.text("cancel")) { job?.cancel() }
                    }.padding()
                }
                fileContents
                if selecting { selectionBar }
                if let selectionTitle, let onSelect {
                    Button {
                        run { try await onSelect(activePath); dismiss() }
                    } label: { Text(selectionTitle).frame(maxWidth: .infinity, minHeight: 44) }
                    .buttonStyle(.borderedProminent).disabled(busy || loading || !canMutate())
                    .padding().accessibilityIdentifier("workspace.browser.choose")
                }
            }
            .navigationTitle(activePath == paths.guestHome ? WorkspaceBrowserStrings.text("home") : (activePath as NSString).lastPathComponent)
            .navigationBarTitleDisplayMode(.inline)
            .searchable(text: $query, prompt: FilesStrings.text("search"))
            .toolbar { toolbar }
            .task(id: loadKey) { await load() }
            .onDisappear { job?.cancel() }
            .onChange(of: transfer?.id) { _, _ in if let transfer { modal = .destination(transfer) } }
            .onChange(of: preview?.id) { _, _ in if let preview { modal = .preview(preview) } }
            .onChange(of: info?.id) { _, _ in if let info { modal = .details(info) } }
            .onChange(of: shareSnapshots.count) { _, count in if count > 0 { modal = .share(shareSnapshots) } }
            .onChange(of: importFiles) { _, value in if value { systemPicker.reset(exporting: false); modal = .importFiles } }
            .onChange(of: importFolder) { _, value in if value { systemPicker.reset(exporting: false); modal = .importFolder } }
            .onChange(of: exportPaths) { _, value in if !value.isEmpty { systemPicker.reset(exporting: true); modal = .exportDestination } }
            .onChange(of: modal?.id) { _, _ in if let modal { lastModal = modal } }
            .alert(FilesStrings.text("failed"), isPresented: Binding(get: { error != nil }, set: { if !$0 { error = nil } })) {
                Button("common_ok") { error = nil }
            } message: { Text(error ?? "") }
            .alert(rename == nil ? FilesStrings.text("new_folder") : FilesStrings.text("rename"), isPresented: $nameDialog) {
                TextField(FilesStrings.text("name"), text: $name).textInputAutocapitalization(.never).autocorrectionDisabled()
                Button(FilesStrings.text("save")) {
                    let target = rename; let value = name; let parent = activePath
                    run(mutation: true) {
                        if let target { _ = try await repository.renameDirectory(target.path, name: value, protectedPaths: mutablePaths) }
                        else { _ = try await repository.createDirectory(in: parent, name: value) }
                    }
                }.disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                Button("common_cancel", role: .cancel) {}
            }
            .alert(WorkspaceBrowserStrings.text("go_to_folder"), isPresented: $goTo) {
                TextField(FilesStrings.text("path"), text: $enteredPath).textInputAutocapitalization(.never).autocorrectionDisabled()
                Button(WorkspaceBrowserStrings.text("open")) {
                    let value = enteredPath == "~" ? paths.guestHome : enteredPath.hasPrefix("~/") ? paths.guestHome + enteredPath.dropFirst() : enteredPath
                    run { try await repository.validateDirectory(value); navigate(value) }
                }
                Button("common_cancel", role: .cancel) {}
            }
            .confirmationDialog(FilesStrings.text("delete"), isPresented: Binding(get: { !deleting.isEmpty }, set: { if !$0 { deleting = [] } }), titleVisibility: .visible) {
                Button(FilesStrings.text("delete"), role: .destructive) {
                    let targets = deleting; deleting = []
                    run(mutation: true) { showFailures(try await repository.deleteFiles(targets, protectedPaths: mutablePaths, progress: { @Sendable value in await MainActor.run { progress = value } })) }
                }
                Button("common_cancel", role: .cancel) { deleting = [] }
            } message: { Text(FilesStrings.text("delete_warning") + "\n\(deleting.count) " + FilesStrings.text("count") + "\n" + deleting.joined(separator: "\n")) }
            .confirmationDialog(FilesStrings.text("conflict"), isPresented: $showConflict, titleVisibility: .visible) {
                Button(FilesStrings.text("keep_both")) { resolveConflict(.keepBoth) }
                Button(FilesStrings.text("skip")) { resolveConflict(.skip) }
                Button("common_cancel", role: .cancel) { conflictRequest = nil; conflictImport = false; importURLs = []; destination = nil }
            } message: { Text(FilesStrings.text("choose_destination") + "\n" + (conflictRequest?.destination ?? destination?.lastPathComponent ?? activePath)) }
            .sheet(item: $modal, onDismiss: finishModal) { route in
                switch route {
                case .destination(let request):
                    WorkspaceFilesView(repository: repository, paths: paths, initialPath: activePath,
                        protectedPaths: protectedPaths, canMutate: canMutate,
                        selectionTitle: "\(request.sources.count) · " + FilesStrings.text("paste"), onSelect: { target in
                            var next = request; next.destination = target; conflictRequest = next
                        })
                case .details(let entry): fileDetails(entry)
                case .preview(let snapshot): WorkspaceFilePreview(repository: repository, snapshot: snapshot)
                case .share(let snapshots): WorkspaceShareSheet(urls: snapshots.map(\.url))
                case .importFiles:
                    WorkspaceSystemFilePicker { urls in
                        systemPicker.receive(urls: urls); modal = nil; completeSystemPicker()
                    }
                case .importFolder:
                    WorkspaceSystemFolderPicker { url in
                        systemPicker.receive(urls: url.map { [$0] } ?? []); modal = nil; completeSystemPicker()
                    }
                case .exportDestination:
                    WorkspaceSystemFolderPicker { url in
                        systemPicker.receive(destination: url); modal = nil; completeSystemPicker()
                    }
                }
            }

        }
        .presentationDetents([.large])
        .accessibilityIdentifier("workspace.browser")
    }

    private var breadcrumbs: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 6) {
                Button { navigate(paths.guestHome) } label: { Image(systemName: "house") }
                let components = activePath.dropFirst(paths.guestHome.count).split(separator: "/").map(String.init)
                ForEach(Array(components.enumerated()), id: \.offset) { index, item in
                    Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.secondary)
                    Button(item) { navigate(paths.guestHome + "/" + components.prefix(index + 1).joined(separator: "/")) }.lineLimit(1)
                }
            }.padding(.horizontal).frame(minHeight: 44)
        }
        .accessibilityLabel(paths.display(activePath)).accessibilityIdentifier("workspace.browser.path")
    }
    @ViewBuilder private var fileContents: some View {
        if loading && entries.isEmpty { ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity) }
        else if entries.isEmpty {
            ContentUnavailableView(FilesStrings.text(query.isEmpty ? "empty" : "no_results"), systemImage: "folder")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if grid {
            ScrollView {
                LazyVGrid(columns: [.init(.adaptive(minimum: 140))], spacing: 16) {
                    ForEach(sortedEntries) { entry in entryButton(entry, grid: true) }
                }.padding()
            }.refreshable { revision += 1 }
        } else {
            List(sortedEntries) { entry in entryButton(entry, grid: false) }
                .listStyle(.plain).refreshable { revision += 1 }
        }
    }
    private func entryButton(_ entry: WorkspaceDirectoryEntry, grid: Bool) -> some View {
        Button {
            if selecting { if selected.contains(entry.path) { selected.remove(entry.path) } else { selected.insert(entry.path) } }
            else if entry.isDirectory {
                run { let target = try await repository.canonicalDirectory(entry.path); navigate(target) }
            } else { run {
                let snapshot = try await repository.snapshot(entry.path)
                do { try Task.checkCancellation() }
                catch { Task.detached { snapshot.remove() }; throw error }
                previewCache = snapshot; previewPresented = true; preview = snapshot
            } }
        } label: {
            FileEntryLabel(entry: entry, repository: repository, grid: grid, selected: selected.contains(entry.path))
                .frame(maxWidth: .infinity)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(busy || previewPresented)
        .accessibilityIdentifier("workspace.folder.\(entry.name)")
        .contextMenu {
            Button(FilesStrings.text("select"), systemImage: "checkmark.circle") { selecting = true; selected.insert(entry.path) }
            Button(FilesStrings.text("details"), systemImage: "info.circle") { info = entry }
            Button(FilesStrings.text("copy_path"), systemImage: "doc.on.clipboard") { UIPasteboard.general.string = entry.path }
            if !entry.isDirectory {
                Button(FilesStrings.text("share"), systemImage: "square.and.arrow.up") { share([entry.path]) }
            }
            Button(FilesStrings.text("export"), systemImage: "square.and.arrow.up.on.square") { exportPaths = [entry.path] }
            Button(FilesStrings.text("rename"), systemImage: "pencil") { rename = entry; name = entry.name; nameDialog = true }.disabled(!canMutate() || busy)
            Button(FilesStrings.text("copy"), systemImage: "doc.on.doc") { transfer = .init(sources: [entry.path], move: false) }.disabled(!canMutate() || busy)
            Button(FilesStrings.text("move"), systemImage: "folder") { transfer = .init(sources: [entry.path], move: true) }.disabled(!canMutate() || busy)
            Button(FilesStrings.text("delete"), systemImage: "trash", role: .destructive) { deleting = [entry.path] }.disabled(!canMutate() || busy)
        }
    }
    private var selectionBar: some View {
        HStack {
            Text("\(selected.count)").monospacedDigit()
            Menu {
                Button(FilesStrings.text("copy")) { transfer = .init(sources: Array(selected), move: false) }
                Button(FilesStrings.text("move")) { transfer = .init(sources: Array(selected), move: true) }
                Button(FilesStrings.text("delete"), role: .destructive) { deleting = Array(selected) }
            } label: { Label(FilesStrings.text("selected"), systemImage: "ellipsis.circle") }
            .disabled(selected.isEmpty || !canMutate() || busy)
            Menu {
                Button(FilesStrings.text("export")) { exportPaths = Array(selected) }
                Button(FilesStrings.text("share")) { share(Array(selected)) }
                    .disabled(entries.filter { selected.contains($0.path) }.contains { $0.isDirectory })
                if selected.count == 1, let entry = entries.first(where: { selected.contains($0.path) }) {
                    Button(FilesStrings.text("details")) { info = entry }
                    Button(FilesStrings.text("rename")) { rename = entry; name = entry.name; nameDialog = true }.disabled(!canMutate())
                }
            } label: { Image(systemName: "square.and.arrow.up") }.disabled(selected.isEmpty || busy)
            Spacer()
            Button(FilesStrings.text("done")) { selecting = false; selected = [] }
        }.padding()
    }
    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .cancellationAction) { Button(selectionTitle == nil ? "common_close" : "common_cancel") { dismiss() } }
        ToolbarItem(placement: .topBarTrailing) {
            Button { navigate((activePath as NSString).deletingLastPathComponent) } label: { Image(systemName: "arrow.up") }
                .disabled(activePath == paths.guestHome).accessibilityIdentifier("workspace.browser.parent")
        }
        ToolbarItem(placement: .topBarTrailing) {
            Menu {
                Button(FilesStrings.text("select"), systemImage: "checkmark.circle") { selecting.toggle(); selected = [] }
                Button(FilesStrings.text(grid ? "list" : "grid"), systemImage: grid ? "list.bullet" : "square.grid.2x2") { grid.toggle() }
                Picker(FilesStrings.text("sort"), selection: $sort) { ForEach(WorkspaceFileSort.allCases) { Text(FilesStrings.text($0.rawValue)).tag($0) } }
                Toggle(FilesStrings.text("hidden"), isOn: $hidden)
                Button(FilesStrings.text("refresh"), systemImage: "arrow.clockwise") { revision += 1 }
                Button(WorkspaceBrowserStrings.text("go_to_folder")) { enteredPath = paths.display(activePath); goTo = true }
                Divider()
                Button(FilesStrings.text("new_folder"), systemImage: "folder.badge.plus") { rename = nil; name = ""; nameDialog = true }
                    .disabled(!canMutate() || busy).accessibilityIdentifier("workspace.browser.new-folder")
                Button(FilesStrings.text("import_files")) { importFiles = true }.disabled(!canMutate() || busy)
                Button(FilesStrings.text("import_folder")) { importFolder = true }.disabled(!canMutate() || busy)
            } label: { Image(systemName: "ellipsis.circle") }
            .accessibilityLabel(WorkspaceBrowserStrings.text("folder_actions")).accessibilityIdentifier("workspace.browser.actions")
        }
    }
    private func fileDetails(_ entry: WorkspaceDirectoryEntry) -> some View {
        NavigationStack {
            List {
                LabeledContent(FilesStrings.text("name"), value: entry.name)
                LabeledContent(FilesStrings.text("type"), value: entry.isSymbolicLink ? FilesStrings.text("link") : entry.isDirectory ? FilesStrings.text("folder") : entry.fileExtension)
                if !entry.isDirectory { LabeledContent(FilesStrings.text("size"), value: ByteCountFormatter.string(fromByteCount: entry.byteCount, countStyle: .file)) }
                LabeledContent(FilesStrings.text("modified"), value: entry.modified.formatted())
                Text(entry.path).font(.body.monospaced()).textSelection(.enabled)
            }.navigationTitle(FilesStrings.text("details"))
                .toolbar { Button("common_close") { modal = nil } }
        }.presentationDetents([.medium, .large])
    }
    private func finishModal() {
        switch lastModal {
        case .preview:
            if let snapshot = previewCache { Task.detached { snapshot.remove() } }
            previewCache = nil; preview = nil; previewPresented = false
        case .share:
            let snapshots = shareSnapshots
            Task.detached { snapshots.forEach { $0.remove() } }; shareSnapshots = []
        case .destination:
            transfer = nil
            if conflictRequest?.destination != nil { showConflict = true }
        case .details: info = nil
        case .importFiles, .importFolder, .exportDestination:
            systemPicker.dismissed = true
            completeSystemPicker()
        case nil: break
        }
        lastModal = nil
    }
    private func completeSystemPicker() {
        guard systemPicker.consumeIfReady() else { return }
        importFiles = false; importFolder = false
        if systemPicker.exporting {
            if destination != nil { conflictImport = true; showConflict = true }
            else { exportPaths = [] }
        } else if !importURLs.isEmpty { conflictImport = true; showConflict = true }
    }
    private func share(_ paths: [String]) {
        run {
            var created: [WorkspaceFileSnapshot] = []
            do {
                for path in paths { created.append(try await repository.snapshot(path)) }
                try Task.checkCancellation(); shareSnapshots = created
            } catch {
                let snapshots = created
                Task.detached { snapshots.forEach { $0.remove() } }; throw error
            }
        }
    }
    private func navigate(_ value: String) { path = value; query = ""; selected = []; entries = []; revision += 1 }
    private func load() async {
        loading = true
        do {
            try Task.checkCancellation()
            let values = try await repository.files(at: activePath, hidden: hidden, query: query, recursive: recursive, sort: sort)
            try Task.checkCancellation(); entries = values; loading = false
        } catch is CancellationError { } catch { self.error = error.localizedDescription; loading = false }
    }
    private func showFailures(_ failures: [WorkspaceFileFailure]) {
        if !failures.isEmpty { error = failures.map { $0.path + ": " + $0.message }.joined(separator: "\n") }
    }
    private func run(mutation: Bool = false, _ action: @escaping @MainActor () async throws -> Void) {
        guard job == nil else { return }
        progress = .init(completed: 0, total: 1, name: FilesStrings.text("progress"))
        job = Task { @MainActor in
            defer { job = nil; progress = nil; revision += 1; selected = [] }
            do { if mutation && !canMutate() { throw WorkspaceDirectoryError.busy }; try await action() }
            catch is CancellationError {} catch { self.error = error.localizedDescription }
        }
    }
    private func resolveConflict(_ conflict: WorkspaceFileConflict) {
        if let request = conflictRequest, let destination = request.destination {
            conflictRequest = nil
            run(mutation: true) { showFailures(try await repository.transfer(request.sources, to: destination, move: request.move, conflict: conflict, protectedPaths: mutablePaths, progress: { @Sendable value in await MainActor.run { progress = value } })) }
        } else if let target = destination {
            let sources = exportPaths; destination = nil; exportPaths = []; conflictImport = false
            run { showFailures(try await repository.exportItems(sources, to: target, conflict: conflict, progress: { @Sendable value in await MainActor.run { progress = value } })) }
        } else {
            let urls = importURLs; importURLs = []; conflictImport = false
            run(mutation: true) { showFailures(try await repository.importItems(urls, to: activePath, conflict: conflict, progress: { @Sendable value in await MainActor.run { progress = value } })) }
        }
    }
}

private struct FileEntryLabel: View {
    let entry: WorkspaceDirectoryEntry
    let repository: WorkspaceDirectoryRepository
    let grid: Bool
    let selected: Bool
    @State private var thumbnail: Data?
    var body: some View {
        Group {
            if grid { VStack(spacing: 8) { icon.frame(height: 70); text }.frame(maxWidth: .infinity).padding().background(.quaternary.opacity(0.5), in: .rect(cornerRadius: 14)) }
            else { HStack(spacing: 12) { icon.frame(width: 36, height: 44); text; Spacer(); if entry.isDirectory { Image(systemName: "chevron.right").font(.caption).foregroundStyle(.secondary) } }.frame(minHeight: 48) }
        }
        .overlay(alignment: .topTrailing) { if selected { Image(systemName: "checkmark.circle.fill").foregroundStyle(.tint) } }
        .task(id: entry.path + String(entry.modified.timeIntervalSince1970)) {
            if ["jpg", "jpeg", "png", "heic", "webp", "gif"].contains(entry.fileExtension) {
                thumbnail = try? await repository.thumbnail(entry.path)
            }
        }
    }
    @ViewBuilder private var icon: some View {
        if let thumbnail, let image = UIImage(data: thumbnail) { Image(uiImage: image).resizable().scaledToFit().accessibilityHidden(true) }
        else { Image(systemName: entry.symbol).font(grid ? .largeTitle : .title2).foregroundStyle(entry.isDirectory ? Color.accentColor : .secondary).accessibilityHidden(true) }
    }
    private var text: some View {
        VStack(alignment: grid ? .center : .leading, spacing: 3) {
            Text(entry.name).lineLimit(2).foregroundStyle(.primary)
            Text((entry.isDirectory ? "" : ByteCountFormatter.string(fromByteCount: entry.byteCount, countStyle: .file) + " · ") + entry.modified.formatted(date: .abbreviated, time: .omitted))
                .font(.caption).foregroundStyle(.secondary).lineLimit(1)
        }
    }
}

extension WorkspaceDirectoryRepository {
    func thumbnail(_ path: String) throws -> Data? {
        let url = try resolve(path, directoryOnly: false)
        try Task.checkCancellation()
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
              let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [kCGImageSourceCreateThumbnailFromImageAlways: true, kCGImageSourceThumbnailMaxPixelSize: 240, kCGImageSourceCreateThumbnailWithTransform: true] as CFDictionary) else { return nil }
        return UIImage(cgImage: image).pngData()
    }
}

struct WorkspaceSystemFolderPicker: UIViewControllerRepresentable {
    let completion: (URL?) -> Void
    func makeCoordinator() -> Coordinator { Coordinator(completion) }
    func makeUIViewController(context: Context) -> UIDocumentPickerViewController {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder])
        picker.delegate = context.coordinator; return picker
    }
    func updateUIViewController(_ controller: UIDocumentPickerViewController, context: Context) {}
    final class Coordinator: NSObject, UIDocumentPickerDelegate {
        let completion: (URL?) -> Void
        init(_ completion: @escaping (URL?) -> Void) { self.completion = completion }
        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) { completion(urls.first) }
        func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { completion(nil) }
    }
}

private struct WorkspaceSystemFilePicker: UIViewControllerRepresentable {
    let completion: ([URL]) -> Void
    func makeCoordinator() -> Coordinator { Coordinator(completion) }
    func makeUIViewController(context: Context) -> UIDocumentPickerViewController {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.item])
        picker.allowsMultipleSelection = true; picker.delegate = context.coordinator; return picker
    }
    func updateUIViewController(_ controller: UIDocumentPickerViewController, context: Context) {}
    final class Coordinator: NSObject, UIDocumentPickerDelegate {
        let completion: ([URL]) -> Void
        init(_ completion: @escaping ([URL]) -> Void) { self.completion = completion }
        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) { completion(urls) }
        func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { completion([]) }
    }
}

/// UIKit may deliver a document selection before or after its sheet dismissal callback.
/// Consume it once, only after both events have arrived.
final class WorkspaceSystemPickerCompletion {
    var urls: [URL] = []
    var destination: URL?
    var exporting = false
    var dismissed = false
    private var received = false
    func reset(exporting: Bool) {
        urls = []; destination = nil; self.exporting = exporting; dismissed = false; received = false
    }
    func receive(urls: [URL]) { self.urls = urls; received = true }
    func receive(destination: URL?) { self.destination = destination; received = true }
    func consumeIfReady() -> Bool {
        guard dismissed && received else { return false }
        received = false; return true
    }
}
