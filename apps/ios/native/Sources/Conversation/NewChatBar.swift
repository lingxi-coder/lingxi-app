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
