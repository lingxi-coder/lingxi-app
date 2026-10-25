import SwiftUI

enum WorkspaceBrowserStrings {
    static func text(_ key: String) -> String {
        String(localized: String.LocalizationValue(key), table: "WorkspaceBrowser")
    }
}

/// One browser for picking an existing directory, saving a location and
/// managing actual folders in the persistent runtime account.
struct WorkspaceFolderBrowser: View {
    @Environment(\.dismiss) private var dismiss
    let repository: WorkspaceDirectoryRepository
    let paths: WorkspaceDirectoryPaths
    let initialPath: String
    let protectedPaths: () -> [String]
    let canMutate: () -> Bool
    let selectionTitle: String
    let onSelect: (String) async throws -> Void
    @State private var navigation: [String] = []

    var body: some View {
        NavigationStack(path: $navigation) {
            page(initialPath)
                .navigationDestination(for: String.self) { page($0) }
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("common_cancel") { dismiss() }
                    }
                }
        }
        .presentationDetents([.large])
        .accessibilityIdentifier("workspace.browser")
    }

    private func page(_ path: String) -> some View {
        WorkspaceFolderPage(repository: repository, paths: paths, path: path,
            protectedPaths: protectedPaths, canMutate: canMutate, selectionTitle: selectionTitle,
            onSelect: { selected in try await onSelect(selected); dismiss() })
    }
}

private struct WorkspaceFolderPage: View {
    @Environment(\.theme) private var t
    let repository: WorkspaceDirectoryRepository
    let paths: WorkspaceDirectoryPaths
    let path: String
    let protectedPaths: () -> [String]
    let canMutate: () -> Bool
    let selectionTitle: String
    let onSelect: (String) async throws -> Void
    @State private var entries: [WorkspaceDirectoryEntry] = []
    @State private var includingHidden = false
    @State private var loading = true
    @State private var busy = false
    @State private var errorMessage: String?
    @State private var editingName = false
    @State private var renameTarget: WorkspaceDirectoryEntry?
    @State private var deletingTarget: WorkspaceDirectoryEntry?
    @State private var name = ""
    @State private var showingPathEntry = false
    @State private var enteredPath = ""
    @State private var requestedPath: String?

    private var mutableEntries: [String] { protectedPaths() + [paths.guestHome, paths.workspace, paths.projects] }

    var body: some View {
        List {
            Section {
                Label(paths.display(path), systemImage: path == paths.guestHome ? "house" : "folder")
                    .font(.subheadline.monospaced())
                    .textSelection(.enabled)
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel(paths.display(path))
                    .accessibilityIdentifier("workspace.browser.path")
            }
            Section {
                if loading {
                    ProgressView().frame(maxWidth: .infinity)
                } else if entries.isEmpty {
                    ContentUnavailableView(WorkspaceBrowserStrings.text("empty_title"), systemImage: "folder")
                }
                ForEach(entries) { entry in
                    if entry.isDirectory {
                        NavigationLink(value: entry.path) { entryLabel(entry) }
                            .accessibilityIdentifier("workspace.folder.\(entry.name)")
                            .contextMenu {
                                Button(WorkspaceBrowserStrings.text("rename_folder"), systemImage: "pencil") {
                                    renameTarget = entry
                                    name = entry.name
                                    editingName = true
                                }
                                .disabled(!canMutate() || isProtected(entry))
                                Button(WorkspaceBrowserStrings.text("delete_folder"), systemImage: "trash", role: .destructive) {
                                    deletingTarget = entry
                                }
                                .disabled(!canMutate() || isProtected(entry))
                            }
                    } else {
                        entryLabel(entry).foregroundStyle(t.text2)
                    }
                }
            } header: {
                Text(WorkspaceBrowserStrings.text("contents"))
            }
        }
        .navigationTitle(path == paths.guestHome ? WorkspaceBrowserStrings.text("home") : (path as NSString).lastPathComponent)
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await load() }
        .task(id: includingHidden) { await load() }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    requestedPath = (path as NSString).deletingLastPathComponent
                } label: {
                    Label(WorkspaceBrowserStrings.text("parent_folder"), systemImage: "arrow.up")
                }
                .disabled(path == paths.guestHome)
                .accessibilityIdentifier("workspace.browser.parent")
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button(WorkspaceBrowserStrings.text("new_folder"), systemImage: "folder.badge.plus") {
                        renameTarget = nil
                        name = ""
                        editingName = true
                    }
                    .disabled(!canMutate())
                    .accessibilityIdentifier("workspace.browser.new-folder")
                    Button(WorkspaceBrowserStrings.text("go_to_folder"), systemImage: "arrow.right.to.line") {
                        enteredPath = paths.display(path)
                        showingPathEntry = true
                    }
                    Toggle(WorkspaceBrowserStrings.text("show_hidden"), isOn: $includingHidden)
                } label: {
                    Label(WorkspaceBrowserStrings.text("folder_actions"), systemImage: "ellipsis.circle")
                }
                .accessibilityIdentifier("workspace.browser.actions")
            }
        }
        .safeAreaInset(edge: .bottom) {
            Button {
                Task { await perform { try await onSelect(path) } }
            } label: {
                HStack {
                    if busy { ProgressView() }
                    Text(selectionTitle).fontWeight(.semibold)
                }
                .frame(maxWidth: .infinity, minHeight: 36)
            }
            .buttonStyle(.borderedProminent)
            .disabled(busy || loading)
            .padding()
            .background(t.windowBg)
            .accessibilityIdentifier("workspace.browser.choose")
        }
        .alert(renameTarget == nil ? WorkspaceBrowserStrings.text("new_folder") : WorkspaceBrowserStrings.text("rename_folder"), isPresented: $editingName) {
            TextField(WorkspaceBrowserStrings.text("folder_name"), text: $name)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            Button("common_cancel", role: .cancel) { }
            Button("common_save") {
                let target = renameTarget
                let value = name
                Task {
                    await perform {
                        guard canMutate() else { throw WorkspaceDirectoryError.busy }
                        if let target {
                            _ = try await repository.renameDirectory(target.path, name: value, protectedPaths: mutableEntries)
                        } else {
                            _ = try await repository.createDirectory(in: path, name: value)
                        }
                        await load()
                    }
                }
            }
            .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
        .confirmationDialog(WorkspaceBrowserStrings.text("delete_folder"), isPresented: Binding(
            get: { deletingTarget != nil }, set: { if !$0 { deletingTarget = nil } }
        ), titleVisibility: .visible) {
            if let entry = deletingTarget {
                Button(WorkspaceBrowserStrings.text("delete_folder"), role: .destructive) {
                    Task {
                        await perform {
                            guard canMutate() else { throw WorkspaceDirectoryError.busy }
                            try await repository.deleteDirectory(entry.path, protectedPaths: mutableEntries)
                            await load()
                        }
                    }
                }
            }
            Button("common_cancel", role: .cancel) { deletingTarget = nil }
        } message: {
            Text(WorkspaceBrowserStrings.text("delete_warning") + "\n" + (deletingTarget?.path ?? ""))
        }
        .alert(WorkspaceBrowserStrings.text("go_to_folder"), isPresented: $showingPathEntry) {
            TextField(paths.display(paths.workspace), text: $enteredPath)
                .textInputAutocapitalization(.never).autocorrectionDisabled()
            Button("common_cancel", role: .cancel) { }
            Button(WorkspaceBrowserStrings.text("open")) {
                let value = enteredPath.trimmingCharacters(in: .whitespacesAndNewlines)
                let expanded = value == "~" ? paths.guestHome : value.hasPrefix("~/") ? paths.guestHome + value.dropFirst() : value
                Task {
                    await perform {
                        try await repository.validateDirectory(expanded)
                        if expanded != path { requestedPath = expanded }
                    }
                }
            }
        }
        .navigationDestination(item: $requestedPath) { value in
            WorkspaceFolderPage(repository: repository, paths: paths, path: value,
                protectedPaths: protectedPaths, canMutate: canMutate, selectionTitle: selectionTitle, onSelect: onSelect)
        }
        .alert(WorkspaceBrowserStrings.text("operation_failed"), isPresented: Binding(
            get: { errorMessage != nil }, set: { if !$0 { errorMessage = nil } }
        )) {
            Button("common_ok") { errorMessage = nil }
        } message: { Text(errorMessage ?? "") }
    }

    private func entryLabel(_ entry: WorkspaceDirectoryEntry) -> some View {
        HStack(spacing: 12) {
            Image(systemName: entry.isSymbolicLink ? "folder.badge.questionmark" : entry.isDirectory ? "folder.fill" : "doc")
                .foregroundStyle(entry.isDirectory ? t.accent : t.text3)
            Text(entry.name).lineLimit(2)
            Spacer(minLength: 4)
            if !entry.isDirectory {
                Text(ByteCountFormatter.string(fromByteCount: entry.byteCount, countStyle: .file))
                    .font(.caption).foregroundStyle(t.text3)
            }
        }
        .frame(minHeight: 36)
    }

    private func isProtected(_ entry: WorkspaceDirectoryEntry) -> Bool {
        entry.isSymbolicLink || mutableEntries.contains { $0 == entry.path || $0.hasPrefix(entry.path + "/") }
            || entry.path == paths.guestHome + "/" + paths.settings
            || entry.path.hasPrefix(paths.guestHome + "/" + paths.settings + "/")
    }

    private func load() async {
        loading = true
        defer { loading = false }
        do { entries = try await repository.entries(at: path, includingHidden: includingHidden) }
        catch { errorMessage = error.localizedDescription }
    }

    private func perform(_ operation: () async throws -> Void) async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        do { try await operation() }
        catch { errorMessage = error.localizedDescription }
    }
}
