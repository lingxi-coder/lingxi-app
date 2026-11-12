import SwiftUI

/// Browser routing and browser automation have separate, platform-owned capabilities.
struct BrowserSettingsPage: View {
    var body: some View {
        VStack(spacing: 0) {
            SettingsSection(
                label: String(localized: "settings_browser_opening"),
                footer: String(localized: "settings_browser_default_description")
            ) {
                SettingsRow(
                    icon: .globe,
                    label: String(localized: "settings_browser_system_default"),
                    sub: String(localized: "settings_browser_ios_default_settings"),
                    chevron: false,
                    isLast: true
                )
                .accessibilityIdentifier("settings.browser.opening")
            }
            SettingsSection(
                label: String(localized: "settings_browser_automation"),
                footer: String(localized: "settings_browser_desktop_note")
            ) {
                SettingsRow(
                    label: String(localized: "settings_browser_ios_control"),
                    sub: String(localized: "settings_browser_ios_unavailable"),
                    value: String(localized: "settings_browser_unavailable"),
                    chevron: false,
                    isLast: true
                )
                .accessibilityIdentifier("settings.browser.automation.unavailable")
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("settings.browser")
    }
}
