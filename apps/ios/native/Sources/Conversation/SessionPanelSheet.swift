import SwiftUI

/// What a row of the session panel asks the chat to do once the panel is gone.
/// Sheets cannot present one another, so the chat runs these from the panel's
/// `onDismiss`.
enum SessionPanelAction: Hashable {
    case model
    case permission
    case details
    case togglePinned
    case makeScheduled
    case terminal
    case continueInOtherMode
    case archive
}

/// One panel for everything about the current session, replacing the controls
/// and details that used to live in two different places. The pickers it opens
/// (model and effort, permission) are the composer's own.
struct SessionPanelSheet: View {
    @Environment(\.theme) private var t
    let title: String
    let mode: SessionMode
    let workspaceName: String
    let messageCount: Int
    let modelName: String
    let effortLabel: String
    /// nil hides the row: a Chat session has no file permissions to ask about.
    let permissionLabel: String?
    let isPinned: Bool
    /// false hides the pin row: a chat that has not started has no id to pin.
    let canPin: Bool
    let canArchive: Bool
    let onAction: (SessionPanelAction) -> Void

    private var otherMode: SessionMode { mode == .chat ? .code : .chat }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(title)
                            .font(.system(size: 17, weight: .semibold))
                            .foregroundStyle(t.text)
                            .lineLimit(2)
                        Text("\(mode.title) · \(workspaceName) · \(messageCount)")
                            .font(.caption)
                            .foregroundStyle(mode.color(t))
                    }
                    .padding(.vertical, 2)
                    .accessibilityIdentifier("session-panel.header")
                }

                Section {
                    row(
                        title: String(localized: "cron_mode_label"),
                        value: mode.title,
                        footnote: String(localized: "session_panel_mode_hint"),
                        identifier: "session-panel.mode"
                    ) { onAction(.continueInOtherMode) }
                    row(
                        title: String(localized: "session_details_model"),
                        value: modelName,
                        identifier: "session-panel.model"
                    ) { onAction(.model) }
                    row(title: "Effort", value: effortLabel, identifier: "session-panel.effort") {
                        onAction(.model)
                    }
                    if let permissionLabel {
                        row(
                            title: String(localized: "settings_parity_permissions"),
                            value: permissionLabel,
                            identifier: "session-panel.permission"
                        ) { onAction(.permission) }
                    }
                }

                Section {
                    if canPin {
                        actionRow(
                            icon: isPinned ? "pin.slash" : "pin",
                            title: String(localized: isPinned ? "drawer_session_unpin" : "drawer_session_pin"),
                            identifier: "session-panel.pin"
                        ) { onAction(.togglePinned) }
                    }
                    actionRow(
                        icon: "clock.badge",
                        title: String(localized: "session_panel_make_scheduled"),
                        identifier: "session-panel.schedule"
                    ) { onAction(.makeScheduled) }
                    actionRow(
                        icon: "terminal",
                        title: String(localized: "settings_linux_section_terminal"),
                        identifier: "session-panel.terminal"
                    ) { onAction(.terminal) }
                    actionRow(
                        icon: "arrow.triangle.branch",
                        title: String(
                            localized: otherMode == .chat
                                ? "drawer_session_continue_chat" : "drawer_session_continue_code"
                        ),
                        identifier: "session-panel.fork"
                    ) { onAction(.continueInOtherMode) }
                    actionRow(
                        icon: "info.circle",
                        title: String(localized: "session_details_title"),
                        identifier: "session-panel.details"
                    ) { onAction(.details) }
                }

                if canArchive {
                    Section {
                        Button(role: .destructive) { onAction(.archive) } label: {
                            Label(String(localized: "settings_parity_archive"), systemImage: "archivebox")
                        }
                        .accessibilityIdentifier("session-panel.archive")
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(t.windowBg.ignoresSafeArea())
            .navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .accessibilityIdentifier("session-panel")
    }

    private func row(
        title: String,
        value: String,
        footnote: String? = nil,
        identifier: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 3) {
                HStack {
                    Text(title).foregroundStyle(t.text)
                    Spacer(minLength: 12)
                    Text(value)
                        .foregroundStyle(t.text3)
                        .lineLimit(1)
                    LXIcon(name: .chevronR, size: 12, color: t.text4, stroke: 1.7)
                }
                if let footnote {
                    Text(footnote).font(.caption2).foregroundStyle(t.text4)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(identifier)
    }

    private func actionRow(
        icon: String,
        title: String,
        identifier: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Label(title, systemImage: icon).foregroundStyle(t.text)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(identifier)
    }
}
