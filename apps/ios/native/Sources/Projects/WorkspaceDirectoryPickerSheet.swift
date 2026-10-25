import SwiftUI

struct WorkspaceDirectoryPickerSheet: View {
    @Environment(\.theme) private var t
    @Environment(\.dismiss) private var dismiss
    let projects: [ProjectSnapshot]
    let activeScope: ConversationScope
    let currentDirectory: String
    let contextDirectories: [String]
    let protectedPaths: () -> [String]
    let canMutate: () -> Bool
    let runtimeStatus: String
    let onSelectScope: (ConversationScope) -> Void
    let onPickDirectory: (String) async throws -> Void

    private let paths = WorkspaceDirectoryPaths.current()
    @State private var repository = WorkspaceDirectoryRepository()
    @State private var locations: [WorkspaceLocation] = []
    @State private var browser: BrowserRequest?
    @State private var editing: WorkspaceLocation?
    @State private var deleting: WorkspaceLocation?
    @State private var errorMessage: String?
    @State private var busy = false

    private struct BrowserRequest: Identifiable {
        let path: String
        let savesLocation: Bool
        var id: String { path + (savesLocation ? ":save" : ":pick") }
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    HStack(spacing: 12) {
                        Image(systemName: "person.crop.circle.fill")
                            .font(.largeTitle).foregroundStyle(t.accent)
                        VStack(alignment: .leading, spacing: 3) {
                            Text((paths.guestHome as NSString).lastPathComponent).font(.headline)
                            Text(paths.guestHome).font(.caption.monospaced()).foregroundStyle(t.text2)
                            Text("Linux · " + runtimeStatus).font(.caption).foregroundStyle(t.text3)
                        }
                    }
                    .padding(.vertical, 4)
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("workspace.runtime-user")
                }

                Section(WorkspaceBrowserStrings.text("current_folder")) {
                    Button {
                        browser = BrowserRequest(path: currentDirectory, savesLocation: false)
                    } label: {
                        pathLabel(name: (currentDirectory as NSString).lastPathComponent,
                                  path: currentDirectory, symbol: "folder.fill", selected: true)
                    }
                    .accessibilityIdentifier("workspace.current")
                }

                Section(WorkspaceBrowserStrings.text("locations")) {
                    Button {
                        Task { await choose(paths.workspace) }
                    } label: {
                        pathLabel(name: WorkspaceBrowserStrings.text("workspace"), path: paths.workspace,
                                  symbol: "tray.full", selected: currentDirectory == paths.workspace)
                    }
                    .accessibilityIdentifier("workspace.default")
                    Button {
                        browser = BrowserRequest(path: paths.guestHome, savesLocation: false)
                    } label: {
                        pathLabel(name: WorkspaceBrowserStrings.text("home"), path: paths.guestHome,
                                  symbol: "house", selected: currentDirectory == paths.guestHome)
                    }
                    .accessibilityIdentifier("workspace.home")
                    Button {
                        browser = BrowserRequest(path: currentDirectory, savesLocation: false)
                    } label: {
                        Label(WorkspaceBrowserStrings.text("browse_folders"), systemImage: "folder")
                    }
                    .accessibilityIdentifier("workspace.browse")
                }

                Section(WorkspaceBrowserStrings.text("saved_locations")) {
                    ForEach(locations) { location in
                        Button {
                            Task { await choose(location.path) }
                        } label: {
                            pathLabel(name: location.name, path: location.path, symbol: "folder",
                                      selected: currentDirectory == location.path)
                        }
                        .accessibilityIdentifier("workspace.location.\(location.id)")
                        .contextMenu {
                            Button(WorkspaceBrowserStrings.text("edit_location"), systemImage: "pencil") { editing = location }
                            Button(WorkspaceBrowserStrings.text("remove_location"), systemImage: "minus.circle") {
                                Task {
                                    await perform {
                                        locations = try await repository.removeLocation(id: location.id)
                                    }
                                }
                            }
                            Divider()
                            Button(WorkspaceBrowserStrings.text("delete_folder"), systemImage: "trash", role: .destructive) {
                                deleting = location
                            }
                            .disabled(isProtected(location.path))
                        }
                        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                            Button(WorkspaceBrowserStrings.text("remove_location"), systemImage: "minus.circle") {
                                Task { await perform { locations = try await repository.removeLocation(id: location.id) } }
                            }
                            .tint(.orange)
                            Button(WorkspaceBrowserStrings.text("edit_location"), systemImage: "pencil") { editing = location }
                        }
                    }
                    Button {
                        browser = BrowserRequest(path: currentDirectory, savesLocation: true)
                    } label: {
                        Label(WorkspaceBrowserStrings.text("add_location"), systemImage: "plus")
                    }
                    .accessibilityIdentifier("workspace.add-location")
                }

                if !contextDirectories.isEmpty {
                    Section(WorkspaceBrowserStrings.text("session_folders")) {
                        ForEach(Array(Set(contextDirectories)).sorted(), id: \.self) { path in
                            Button { Task { await choose(path) } } label: {
                                pathLabel(name: (path as NSString).lastPathComponent, path: path,
                                          symbol: "folder.badge.person.crop", selected: path == currentDirectory)
                            }
                        }
                    }
                }

                Section(WorkspaceBrowserStrings.text("projects")) {
                    scopeRow(.global, name: String(localized: "common_global"), path: paths.workspace)
                    ForEach(projects) { project in
                        scopeRow(.project(project.id), name: project.record.name, path: project.workspace.guestPath)
                    }
                }
            }
            .navigationTitle(WorkspaceBrowserStrings.text("title"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("common_close") { dismiss() } }
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        browser = BrowserRequest(path: currentDirectory, savesLocation: true)
                    } label: {
                        Label(WorkspaceBrowserStrings.text("add_location"), systemImage: "folder.badge.plus")
                    }
                }
            }
            .overlay { if busy { ProgressView().padding().background(.regularMaterial, in: .rect(cornerRadius: 12)) } }
            .task { await reload() }
            .sheet(item: $browser, onDismiss: { Task { await reload() } }) { request in
                WorkspaceFolderBrowser(repository: repository, paths: paths, initialPath: request.path,
                    protectedPaths: { protectedPaths() + [currentDirectory] }, canMutate: canMutate,
                    selectionTitle: WorkspaceBrowserStrings.text(request.savesLocation ? "add_location" : "use_folder")) { path in
                        _ = try await repository.saveLocation(path: path, name: (path as NSString).lastPathComponent)
                        if !request.savesLocation { try await onPickDirectory(path); dismiss() }
                    }
                    .environment(\.theme, t)
            }
            .sheet(item: $editing, onDismiss: { Task { await reload() } }) { location in
                WorkspaceLocationEditor(location: location, repository: repository, paths: paths)
                    .environment(\.theme, t)
            }
            .confirmationDialog(WorkspaceBrowserStrings.text("delete_folder"), isPresented: Binding(
                get: { deleting != nil }, set: { if !$0 { deleting = nil } }
            ), titleVisibility: .visible) {
                if let location = deleting {
                    Button(WorkspaceBrowserStrings.text("delete_folder"), role: .destructive) {
                        Task {
                            await perform {
                                guard canMutate() else { throw WorkspaceDirectoryError.busy }
                                try await repository.deleteDirectory(location.path, protectedPaths: protectedPaths() + [currentDirectory])
                                await reload()
                            }
                        }
                    }
                }
                Button("common_cancel", role: .cancel) { deleting = nil }
            } message: {
                Text(WorkspaceBrowserStrings.text("delete_warning") + "\n" + (deleting?.path ?? ""))
            }
            .alert(WorkspaceBrowserStrings.text("operation_failed"), isPresented: Binding(
                get: { errorMessage != nil }, set: { if !$0 { errorMessage = nil } }
            )) { Button("common_ok") { errorMessage = nil } } message: { Text(errorMessage ?? "") }
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .accessibilityIdentifier("newchat.workspace-picker")
    }

    private func isProtected(_ path: String) -> Bool {
        !canMutate() || path == paths.guestHome + "/" + paths.settings
            || path.hasPrefix(paths.guestHome + "/" + paths.settings + "/")
            || ([paths.guestHome, paths.workspace, paths.projects, currentDirectory] + protectedPaths())
                .contains { $0 == path || $0.hasPrefix(path + "/") }
    }

    private func pathLabel(name: String, path: String, symbol: String, selected: Bool) -> some View {
        HStack(spacing: 12) {
            Image(systemName: symbol).foregroundStyle(t.accent)
            VStack(alignment: .leading, spacing: 3) {
                Text(name).foregroundStyle(t.text).lineLimit(2)
                Text(paths.display(path)).font(.caption.monospaced()).foregroundStyle(t.text2).lineLimit(2)
            }
            Spacer(minLength: 4)
            if selected { Image(systemName: "checkmark").foregroundStyle(t.accent) }
        }
        .frame(minHeight: 44)
    }

    private func scopeRow(_ scope: ConversationScope, name: String, path: String) -> some View {
        Button { onSelectScope(scope); dismiss() } label: {
            pathLabel(name: name, path: path, symbol: "square.stack.3d.up", selected: scope == activeScope)
        }
        .accessibilityIdentifier("newchat.workspace.\(scope.workspaceKey)")
    }

    private func choose(_ path: String) async {
        await perform {
            try await repository.validateDirectory(path)
            try await onPickDirectory(path)
            dismiss()
        }
    }

    private func reload() async {
        do { locations = try await repository.locations() }
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

private struct WorkspaceLocationEditor: View {
    @Environment(\.dismiss) private var dismiss
    let location: WorkspaceLocation
    let repository: WorkspaceDirectoryRepository
    let paths: WorkspaceDirectoryPaths
    @State private var name: String
    @State private var path: String
    @State private var errorMessage: String?
    @State private var saving = false

    init(location: WorkspaceLocation, repository: WorkspaceDirectoryRepository, paths: WorkspaceDirectoryPaths) {
        self.location = location
        self.repository = repository
        self.paths = paths
        _name = State(initialValue: location.name)
        _path = State(initialValue: location.path)
    }

    var body: some View {
        NavigationStack {
            Form {
                TextField(WorkspaceBrowserStrings.text("location_name"), text: $name)
                    .accessibilityIdentifier("workspace.location.name")
                TextField(WorkspaceBrowserStrings.text("path"), text: $path)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                    .font(.body.monospaced())
                    .accessibilityIdentifier("workspace.location.path")
                Text(WorkspaceBrowserStrings.text("edit_location_hint")).font(.footnote).foregroundStyle(.secondary)
                if let errorMessage { Text(errorMessage).foregroundStyle(.red) }
            }
            .navigationTitle(WorkspaceBrowserStrings.text("edit_location"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("common_cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("common_save") {
                        saving = true
                        let value = path.trimmingCharacters(in: .whitespacesAndNewlines)
                        let expanded = value == "~" ? paths.guestHome : value.hasPrefix("~/") ? paths.guestHome + value.dropFirst() : value
                        Task {
                            defer { saving = false }
                            do {
                                _ = try await repository.saveLocation(path: expanded, name: name, id: location.id)
                                dismiss()
                            } catch { errorMessage = error.localizedDescription }
                        }
                    }
                    .disabled(saving || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    .accessibilityIdentifier("workspace.location.save")
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}
