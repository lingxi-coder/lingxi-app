import SwiftUI

/// The strip above the composer on an empty chat: pick the mode the new chat
/// starts in and the workspace it runs in. Both default to the last choice, so
/// most people never touch it.
struct NewChatBar: View {
    @Environment(\.theme) private var t
    let mode: SessionMode
    let workspaceName: String
    let onSelectMode: (SessionMode) -> Void
    let onPickWorkspace: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            ForEach(SessionMode.allCases, id: \.self) { candidate in
                let selected = candidate == mode
                Button {
                    if !selected { onSelectMode(candidate) }
                } label: {
                    HStack(spacing: 5) {
                        candidate.glyph(t, size: 13)
                        Text(candidate.title)
                            .font(.footnote.weight(selected ? .semibold : .medium))
                    }
                    .foregroundStyle(selected ? t.text : t.text3)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(selected ? candidate.color(t).opacity(0.16) : Color.clear, in: Capsule())
                    .overlay(Capsule().stroke(selected ? candidate.color(t).opacity(0.5) : t.border, lineWidth: 0.7))
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selected ? .isSelected : [])
                .accessibilityIdentifier("newchat.mode.\(candidate.rawValue)")
            }
            Spacer(minLength: 8)
            Button(action: onPickWorkspace) {
                HStack(spacing: 5) {
                    LXIcon(name: .folder, size: 13, color: t.text3, stroke: 1.8)
                    Text(workspaceName)
                        .font(.footnote.weight(.medium))
                        .foregroundStyle(t.text2)
                        .lineLimit(1)
                        .frame(maxWidth: 140, alignment: .leading)
                    LXIcon(name: .chevron, size: 10, color: t.text4, stroke: 2)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .overlay(Capsule().stroke(t.border, lineWidth: 0.7))
                .contentShape(Capsule())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(workspaceName)
            .accessibilityHint(String(localized: "new_chat_pick_workspace_title"))
            .accessibilityIdentifier("newchat.workspace")
        }
        .padding(.horizontal, 14)
        .padding(.top, 6)
        .padding(.bottom, 2)
    }
}

/// Where should the new chat start: the global workspace or one of the projects.
struct WorkspacePickerSheet: View {
    @Environment(\.theme) private var t
    @Environment(\.dismiss) private var dismiss
    let projects: [ProjectSnapshot]
    let activeScope: ConversationScope
    let onSelect: (ConversationScope) -> Void

    var body: some View {
        NavigationStack {
            List {
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
