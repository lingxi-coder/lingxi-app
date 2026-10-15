import SwiftUI

// MARK: - Knowledge
struct KnowledgePage: View {
    @Environment(\.theme) private var t
    @State private var autoRecall = true
    var body: some View {
        VStack(spacing: 0) {
            blurb(String(localized: "settings_knowledge_blurb"))
            SettingsSection(label: String(localized: "settings_section_storage")) {
                SettingsRow(label: String(localized: "settings_used_space"), value: "142 MB", chevron: false)
                SettingsRow(label: String(localized: "settings_file_count"),
                            value: String(localized: "settings_knowledge_file_count"), chevron: false)
                SettingsRow(label: String(localized: "settings_index_model"), value: "bge-m3-local", isLast: true, onTap: {})
            }
            SettingsSection(label: String(localized: "settings_section_behavior")) {
                SettingsRow(label: String(localized: "settings_auto_recall"),
                            sub: String(localized: "settings_auto_recall_sub"), chevron: false) { LXToggle(isOn: $autoRecall) }
                SettingsRow(label: String(localized: "settings_recall_limit"),
                            value: String(localized: "settings_recall_limit_count"), isLast: true, onTap: {})
            }
        }
    }
}

// MARK: - Memory
struct MemoryPage: View {
    @Environment(\.theme) private var t
    var body: some View {
        VStack(spacing: 0) {
            blurb(String(localized: "settings_memory_blurb"))
            SettingsSection(label: String(localized: "settings_section_recent_memory")) {
                SettingsRow(label: String(localized: "settings_memory_mock_item_1"),
                            sub: String(localized: "settings_memory_mock_sub_1"), onTap: {})
                SettingsRow(label: String(localized: "settings_memory_mock_item_2"),
                            sub: String(localized: "settings_memory_mock_sub_2"), onTap: {})
                SettingsRow(label: String(localized: "settings_memory_mock_item_3"),
                            sub: String(localized: "settings_memory_mock_sub_3"), isLast: true, onTap: {})
            }
            SettingsSection {
                SettingsRow(icon: .x, label: String(localized: "settings_clear_all_memory"),
                            chevron: false, danger: true, isLast: true, onTap: {})
            }
        }
    }
}

// MARK: - Workflows
struct WorkflowsPage: View {
    @State private var w1 = true
    @State private var w2 = true
    @State private var w3 = true
    @State private var w4 = false
    var body: some View {
        SettingsSection(label: String(localized: "settings_section_automation"),
                        footer: String(localized: "settings_workflows_footer")) {
            SettingsRow(label: String(localized: "settings_workflow_daily_brief"),
                        sub: String(localized: "settings_workflow_weekday_0830"), chevron: false) { LXToggle(isOn: $w1) }
            SettingsRow(label: String(localized: "settings_workflow_weekly_report"),
                        sub: String(localized: "settings_workflow_friday_1700"), chevron: false) { LXToggle(isOn: $w2) }
            SettingsRow(label: String(localized: "settings_workflow_customer_feedback"),
                        sub: String(localized: "settings_workflow_monday_0900"), chevron: false) { LXToggle(isOn: $w3) }
            SettingsRow(label: String(localized: "settings_workflow_midnight_log"),
                        sub: String(localized: "settings_status_paused"), chevron: false, isLast: true) { LXToggle(isOn: $w4) }
        }
    }
}

// MARK: - Notifications

/// The idle-threshold choices, in render order. Pure and internal so the set
/// and its ordering can be pinned without rendering anything.
///
/// One minute is upstream Claude Code's `messageIdleNotifThresholdMs` default;
/// the others exist because a threshold that can only be the default is not a
/// setting.
func idleThresholdChoices() -> [(ms: Int, label: String)] {
    [
        (15_000, "15s"),
        (NotificationPolicy.defaultIdleNotifThresholdMs, "1m"),
        (180_000, "3m"),
        (600_000, "10m"),
    ]
}

/// Real notification preferences.
///
/// These four toggles used to be `workflows` / `mentions` / `crons` /
/// `marketing` bound to an in-memory struct that was never persisted and that
/// nothing ever read — and two of those names described things this app has no
/// concept of. They now name the four moments that actually notify, and every
/// edit is persisted and pushed at the notifier.
struct NotificationsPage: View {
    @Bindable var store: SettingsStore
    var body: some View {
        SettingsSection(label: String(localized: "settings_notifications"),
                        footer: String(localized: "settings_notifications_footer")) {
            SettingsRow(label: String(localized: "settings_notif_enabled"),
                        sub: String(localized: "settings_notif_enabled_sub"), chevron: false, isLast: true) {
                LXToggle(isOn: binding(\.enabled))
            }
        }
        SettingsSection(label: String(localized: "settings_section_notification_type")) {
            SettingsRow(label: String(localized: "settings_notif_idle_prompt"),
                        sub: String(localized: "settings_notif_idle_prompt_sub"), chevron: false) {
                LXToggle(isOn: binding(\.idlePromptNotifEnabled)).disabled(!store.notifs.enabled)
            }
            SettingsRow(label: String(localized: "settings_notif_needs_input"),
                        sub: String(localized: "settings_notif_needs_input_sub"), chevron: false) {
                LXToggle(isOn: binding(\.inputNeededNotifEnabled)).disabled(!store.notifs.enabled)
            }
            SettingsRow(label: String(localized: "settings_notif_background_task"),
                        sub: String(localized: "settings_notif_background_task_sub"), chevron: false) {
                LXToggle(isOn: binding(\.taskCompleteNotifEnabled)).disabled(!store.notifs.enabled)
            }
            SettingsRow(label: String(localized: "settings_notif_cron_report"),
                        sub: String(localized: "settings_notif_cron_report_sub"), chevron: false, isLast: true) {
                LXToggle(isOn: binding(\.scheduledRunNotifEnabled)).disabled(!store.notifs.enabled)
            }
        }
        SettingsSection(label: String(localized: "settings_notif_idle_threshold"),
                        footer: String(localized: "settings_notif_idle_threshold_sub")) {
            Picker(String(localized: "settings_notif_idle_threshold"),
                   selection: Binding(
                       get: { store.notifs.messageIdleNotifThresholdMs },
                       set: { newValue in
                           var next = store.notifs
                           next.messageIdleNotifThresholdMs = newValue
                           store.setNotifs(next)
                       }
                   )) {
                ForEach(idleThresholdChoices(), id: \.ms) { choice in
                    Text(choice.label).tag(choice.ms)
                }
            }
            .pickerStyle(.segmented)
            .disabled(!store.notifs.enabled)
        }
    }

    /// Every write goes through `SettingsStore.setNotifs`, which persists AND
    /// hands the value to the notifier — the notifier holds armed timers and
    /// cannot notice a preference it is never told about.
    private func binding(_ path: WritableKeyPath<NotifConfig, Bool>) -> Binding<Bool> {
        Binding(
            get: { store.notifs[keyPath: path] },
            set: { newValue in
                var next = store.notifs
                next[keyPath: path] = newValue
                store.setNotifs(next)
            }
        )
    }
}

// MARK: - Input
struct InputPage: View {
    @State private var autoSend = true
    @State private var smartSugg = true
    @State private var fromHistory = true
    var body: some View {
        VStack(spacing: 0) {
            SettingsSection(label: String(localized: "settings_section_voice_input")) {
                SettingsRow(label: String(localized: "settings_push_to_talk_language"),
                            value: String(localized: "settings_auto"), onTap: {})
                SettingsRow(label: String(localized: "settings_auto_send"), chevron: false, isLast: true) { LXToggle(isOn: $autoSend) }
            }
            SettingsSection(label: String(localized: "settings_section_suggestions")) {
                SettingsRow(label: String(localized: "settings_smart_suggestions"), chevron: false) { LXToggle(isOn: $smartSugg) }
                SettingsRow(label: String(localized: "settings_suggestions_history"), chevron: false, isLast: true) { LXToggle(isOn: $fromHistory) }
            }
        }
    }
}

// MARK: - Privacy
struct PrivacyPage: View {
    @Environment(\.openURL) private var openURL
    var body: some View {
        VStack(spacing: 0) {
            SettingsSection(label: String(localized: "settings_section_data"),
                            footer: String(localized: "privacy_data_summary")) {
                SettingsRow(label: String(localized: "privacy_policy_link"), onTap: {
                    openURL(URL(string: "https://lingxios.net/privacy")!)
                })
                SettingsRow(label: String(localized: "privacy_deletion_link"), onTap: {
                    openURL(URL(string: "https://lingxios.net/data-deletion")!)
                })
                SettingsRow(label: "support.lingxi@gmail.com", isLast: true, onTap: {
                    openURL(URL(string: "mailto:support.lingxi@gmail.com")!)
                })
            }
        }
    }
}

// MARK: - Language
struct LanguagePage: View {
    @Environment(LocalizationManager.self) private var localization
    @State private var follow = true
    var body: some View {
        @Bindable var l10n = localization
        VStack(spacing: 0) {
            SettingsSection(label: String(localized: "settings_language_title"),
                            footer: String(localized: "settings_language_footer")) {
                RadioList(
                    options: LocalizationManager.supported.map { .init(value: $0.code, label: $0.label) },
                    value: $l10n.language
                )
            }
            SettingsSection(label: String(localized: "settings_section_region")) {
                SettingsRow(label: String(localized: "settings_date_format"), value: "2026/5/14", isLast: true, onTap: {})
            }
            SettingsSection(label: String(localized: "settings_section_ai_reply_language")) {
                SettingsRow(label: String(localized: "settings_follow_interface"),
                            sub: String(localized: "settings_follow_interface_sub"), chevron: false, isLast: true) { LXToggle(isOn: $follow) }
            }
        }
    }
}

// MARK: - shared helper
@ViewBuilder
func blurb(_ text: String) -> some View {
    BlurbText(text: text)
}
private struct BlurbText: View {
    @Environment(\.theme) private var t
    let text: String
    var body: some View {
        Text(text).font(.caption).foregroundColor(t.text3).lineSpacing(4)
            .frame(maxWidth: .infinity, alignment: .leading).padding(.bottom, 14)
    }
}
