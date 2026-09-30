import SwiftUI

struct AppIntegrationPage: View {
    @Environment(\.openURL) private var openURL
    @Environment(\.theme) private var t

    var body: some View {
        VStack(spacing: 0) {
            SettingsSection(
                label: String(localized: "settings_section_system_integration"),
                footer: String(localized: "settings_app_integration_system_footer")
            ) {
                capabilityRow(
                    icon: .sparkle,
                    title: String(localized: "settings_app_open_app"),
                    detail: String(localized: "settings_app_open_app_detail")
                )
                capabilityRow(
                    icon: .edit,
                    title: String(localized: "settings_app_new_conversation"),
                    detail: String(localized: "settings_app_new_conversation_detail")
                )
                capabilityRow(
                    icon: .message,
                    title: String(localized: "settings_app_ask_question"),
                    detail: String(localized: "settings_app_ask_question_detail")
                )
                capabilityRow(
                    icon: .workflow,
                    title: String(localized: "settings_app_open_terminal"),
                    detail: String(localized: "settings_app_open_terminal_detail"),
                    isLast: true
                )
            }

            SettingsSection(
                label: String(localized: "settings_section_shortcuts"),
                footer: String(localized: "settings_shortcuts_footer")
            ) {
                SettingsRow(
                    icon: .workflow,
                    iconColor: t.accent,
                    label: String(localized: "settings_open_shortcuts"),
                    sub: String(localized: "settings_shortcuts_sub"),
                    value: String(localized: "settings_shortcuts_actions_count"),
                    isLast: true,
                    onTap: openShortcuts
                )
                .accessibilityIdentifier("settings.appIntegration.openShortcuts")
            }

            SettingsSection(label: String(localized: "settings_section_platform_boundary")) {
                VStack(alignment: .leading, spacing: 8) {
                    Label("settings_platform_note_1", systemImage: "checkmark.shield")
                    Label("settings_platform_note_2", systemImage: "hand.raised")
                    Label("settings_platform_note_3", systemImage: "square.and.arrow.up")
                }
                .font(.system(size: 12.5))
                .foregroundStyle(t.text2)
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private func capabilityRow(
        icon: LXIconName,
        title: String,
        detail: String,
        isLast: Bool = false
    ) -> some View {
        SettingsRow(
            icon: icon,
            iconColor: t.accent,
            label: title,
            sub: detail,
            value: String(localized: "settings_status_available"),
            valueColor: t.ok,
            chevron: false,
            isLast: isLast
        )
    }

    private func openShortcuts() {
        guard let url = URL(string: "shortcuts://") else { return }
        openURL(url)
    }
}
