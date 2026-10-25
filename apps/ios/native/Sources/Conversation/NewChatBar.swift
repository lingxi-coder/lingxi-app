import SwiftUI

/// The strip above the composer on an empty chat: pick the mode the new chat
/// starts in and the workspace it runs in. Both default to the last choice, so
/// most people never touch it.
struct NewChatBar: View {
    @Environment(\.theme) private var t
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    let mode: SessionMode
    let workspaceName: String
    let onSelectMode: (SessionMode) -> Void
    let onPickWorkspace: () -> Void

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 12) {
                modePicker
                workspaceButton
            }
            VStack(alignment: .leading, spacing: 4) {
                modePicker
                workspaceButton
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
    }

    @ViewBuilder
    private var modePicker: some View {
        if dynamicTypeSize.isAccessibilitySize {
            modeSelection
                .pickerStyle(.menu)
                .font(.body)
                .frame(minHeight: 44)
        } else {
            modeSelection
                .pickerStyle(.segmented)
                .frame(minWidth: 160, minHeight: 44)
        }
    }

    private var modeSelection: some View {
        Picker(selection: Binding(get: { mode }, set: onSelectMode)) {
            ForEach(SessionMode.allCases, id: \.self) { candidate in
                Text(candidate.title)
                    .tag(candidate)
                    .accessibilityIdentifier("newchat.mode.\(candidate.rawValue)")
            }
        } label: {
            Text(mode.title)
        }
    }

    private var workspaceButton: some View {
        Button(action: onPickWorkspace) {
            HStack(spacing: 6) {
                Image(systemName: "folder")
                Text(workspaceName)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Image(systemName: "chevron.down")
                    .font(.caption2.weight(.semibold))
            }
            .font(.subheadline)
            .foregroundStyle(t.text2)
            .padding(.horizontal, 8)
            .frame(minHeight: 44)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(workspaceName)
        .accessibilityHint(String(localized: "new_chat_pick_workspace_title"))
        .accessibilityIdentifier("newchat.workspace")
    }
}

/// Where should the new chat start: the global workspace or one of the projects.
struct WorkspacePickerSheet: View {
    @Environment(\.theme) private var t
    @Environment(\.dismiss) private var dismiss
    let projects: [ProjectSnapshot]
    let activeScope: ConversationScope
    let onSelect: (ConversationScope) -> Void

    var directories: [String] = []
    var activeDirectory: String?
    var onPickDirectory: (String) -> Void = { _ in }
    var onAddDirectory: (String) -> Void = { _ in }
    @State private var addingDirectory = false
    @State private var directoryInput = ""

    var body: some View {
        NavigationStack {
            List {
                if !directories.isEmpty {
                    Section("workspace_linked_directories") {
                        ForEach(Array(Set(directories)).sorted(), id: \.self) { path in
                            Button {
                                onPickDirectory(path)
                                dismiss()
                            } label: {
                                HStack {
                                    Text(path).font(.system(.caption, design: .monospaced)).foregroundStyle(t.text)
                                    Spacer()
                                    if path == activeDirectory { Image(systemName: "checkmark").foregroundStyle(t.accent) }
                                }
                            }
                        }
                        Button("workspace_add_directory", systemImage: "folder.badge.plus") { addingDirectory = true }
                    }
                }

                row(scope: .global, title: String(localized: "common_global"), subtitle: nil)
                ForEach(projects) { project in
                    row(
                        scope: .project(project.id),
                        title: project.record.name,
                        subtitle: project.workspace.guestPath
                    )
                }
            }
            .navigationTitle(String(localized: "new_chat_pick_workspace_title"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("common_close") { dismiss() }
                }
            }
        }
        .alert("workspace_add_directory", isPresented: $addingDirectory) {
            TextField("~/lingxi/my-worktree", text: $directoryInput)
                .textInputAutocapitalization(.never).autocorrectionDisabled()
            Button("workspace_confirm_add") { onAddDirectory(directoryInput.trimmingCharacters(in: .whitespacesAndNewlines)); dismiss() }
                .disabled(directoryInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            Button("common_cancel", role: .cancel) {}
        }
        .presentationDetents([.medium, .large])
        .accessibilityIdentifier("newchat.workspace-picker")
    }

    private func row(scope: ConversationScope, title: String, subtitle: String?) -> some View {
        Button {
            onSelect(scope)
            dismiss()
        } label: {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).foregroundStyle(t.text)
                    if let subtitle {
                        Text(subtitle)
                            .font(.system(.caption, design: .monospaced))
                            .foregroundStyle(t.text4)
                            .lineLimit(1)
                    }
                }
                Spacer()
                if scope == activeScope {
                    Image(systemName: "checkmark").foregroundStyle(t.accent)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("newchat.workspace.\(scope.workspaceKey)")
    }
}
