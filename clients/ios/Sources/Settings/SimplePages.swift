import SwiftUI

// MARK: - Account
struct AccountPage: View {
    @Environment(\.theme) private var t
    var body: some View {
        VStack(spacing: 0) {
            VStack(spacing: 0) {
                Circle().fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
                    .frame(width: 76, height: 76)
                    .overlay(Text("Y").font(.system(size: 30, weight: .semibold)).foregroundColor(.white))
                    .padding(.bottom, 12)
                Text("Yuxin Yang").font(.system(size: 18, weight: .bold)).foregroundColor(t.text)
                Text("yuxin@axielix.com").font(.system(size: 13)).foregroundColor(t.text4).padding(.top, 4)
                Text("settings_account_pro_renewal")
                    .font(.system(size: 11.5, weight: .semibold)).foregroundColor(t.accent)
                    .padding(.horizontal, 12).padding(.vertical, 4)
                    .background(t.accent.tint(0.18)).clipShape(Capsule()).padding(.top, 10)
            }
            .padding(.top, 8).padding(.bottom, 18)

            SettingsSection(label: String(localized: "settings_section_monthly_usage")) {
                SettingsRow(label: String(localized: "settings_conversation_count"), value: "247 / 1000", chevron: false)
                SettingsRow(label: String(localized: "settings_inference_duration"), value: String(localized: "settings_inference_duration_value"), chevron: false)
                SettingsRow(label: String(localized: "settings_storage"), value: "1.2 / 10 GB", chevron: false, isLast: true)
            }
            SettingsSection {
                SettingsRow(icon: .brain, label: String(localized: "settings_manage_subscription"), onTap: {})
                SettingsRow(icon: .link, label: String(localized: "settings_sync_devices"),
                            sub: String(localized: "settings_sync_devices_sub"), onTap: {})
                SettingsRow(icon: .x, label: String(localized: "settings_logout"), danger: true, isLast: true, onTap: {})
            }
        }
    }
}

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
struct NotificationsPage: View {
    @Bindable var store: SettingsStore
    var body: some View {
        SettingsSection(label: String(localized: "settings_section_notification_type"),
                        footer: String(localized: "settings_notifications_footer")) {
            SettingsRow(label: String(localized: "settings_notif_workflow_complete"),
                        sub: String(localized: "settings_notif_workflow_complete_sub"), chevron: false) { LXToggle(isOn: $store.notifs.workflows) }
            SettingsRow(label: String(localized: "settings_notif_mention"),
                        sub: String(localized: "settings_notif_mention_sub"), chevron: false) { LXToggle(isOn: $store.notifs.mentions) }
            SettingsRow(label: String(localized: "settings_notif_cron_report"),
                        sub: String(localized: "settings_notif_cron_report_sub"), chevron: false) { LXToggle(isOn: $store.notifs.crons) }
            SettingsRow(label: String(localized: "settings_notif_product_update"),
                        sub: String(localized: "settings_notif_product_update_sub"), chevron: false, isLast: true) { LXToggle(isOn: $store.notifs.marketing) }
        }
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
    @State private var contribute = false
    @State private var crash = true
    var body: some View {
        VStack(spacing: 0) {
            SettingsSection(label: String(localized: "settings_section_data")) {
                SettingsRow(icon: .brain, label: String(localized: "settings_export_data"), onTap: {})
                SettingsRow(icon: .x, label: String(localized: "settings_delete_account"), danger: true, isLast: true, onTap: {})
            }
            SettingsSection(label: String(localized: "settings_section_visibility"),
                            footer: String(localized: "settings_privacy_footer")) {
                SettingsRow(label: String(localized: "settings_contribute_training"), chevron: false) { LXToggle(isOn: $contribute) }
                SettingsRow(label: String(localized: "settings_crash_report"), chevron: false, isLast: true) { LXToggle(isOn: $crash) }
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

// MARK: - Voice TTS
struct VoicePage: View {
    @Environment(AppState.self) private var app
    @Environment(\.openURL) private var openURL
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    @State private var capability = VoiceCapabilityModel()
    @State private var saveMessage: String?

    var body: some View {
        VStack(spacing: 0) {
            SettingsSection(
                label: String(localized: "voice_section_config_status"),
                footer: saveMessage ?? capability.configurationReadiness.message
            ) {
                SettingsRow(
                    label: String(localized: "voice_speech_recognition"),
                    value: capability.speechConfigurationConfirmed ? String(localized: "voice_status_saved") : String(localized: "voice_status_pending"),
                    valueColor: capability.speechConfigurationConfirmed ? t.ok : t.text3,
                    chevron: false
                )
                SettingsRow(
                    label: String(localized: "voice_speech_playback"),
                    value: capability.ttsConfigurationConfirmed ? String(localized: "voice_status_saved") : String(localized: "voice_status_pending"),
                    valueColor: capability.ttsConfigurationConfirmed ? t.ok : t.text3,
                    chevron: false
                )
                SettingsRow(label: String(localized: "voice_save_config"), chevron: false, isLast: true) {
                    Button("voice_save_button") {
                        saveVoiceConfiguration()
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(t.accent)
                    .accessibilityIdentifier("settings.voice.save")
                }
            }

            SettingsSection(label: String(localized: "voice_speech_recognition"),
                            footer: capability.effectiveRecognitionLabel) {
                SettingsRow(label: String(localized: "voice_recognition_language"), chevron: false) {
                    Picker("voice_recognition_language", selection: languageBinding) {
                        ForEach(capability.languageOptions) { option in
                            Text(option.title).tag(option.id)
                        }
                    }
                    .labelsHidden()
                    .frame(maxWidth: 190)
                }
                SettingsRow(
                    label: String(localized: "voice_current_selection"),
                    value: capability.selectedLanguageLabel,
                    chevron: false
                )
                SettingsRow(
                    label: String(localized: "voice_effective_language"),
                    value: capability.effectiveLanguageLabel,
                    chevron: false
                )
                SettingsRow(label: String(localized: "voice_recognition_mode"), chevron: false) {
                    Picker("voice_recognition_mode", selection: modeBinding) {
                        ForEach(VoiceRecognitionMode.allCases) { mode in
                            Text(mode.title).tag(mode)
                        }
                    }
                    .labelsHidden()
                    .frame(maxWidth: 190)
                }
                SettingsRow(
                    label: String(localized: "voice_active_mode"),
                    value: capability.effectiveRecognitionStatus.modeLabel,
                    chevron: false,
                    isLast: capability.effectiveRecognitionStatus.fallbackReason == nil
                )
                if let fallbackReason = capability.effectiveRecognitionStatus.fallbackReason {
                    SettingsRow(
                        label: String(localized: "voice_online_fallback_reason"),
                        sub: fallbackReason,
                        value: String(localized: "voice_status_fallback"),
                        chevron: false,
                        isLast: true
                    )
                }
            }

            SettingsSection(label: String(localized: "voice_section_permissions"),
                            footer: String(localized: "voice_permissions_footer")) {
                SettingsRow(
                    label: String(localized: "voice_speech_recognition"),
                    sub: capability.speechPermission.detail,
                    value: capability.speechPermission.label,
                    valueColor: capability.speechAuthorization == .authorized ? t.ok : t.text3,
                    chevron: false
                )
                SettingsRow(
                    label: String(localized: "voice_microphone"),
                    sub: capability.microphonePermission.detail,
                    value: capability.microphonePermission.label,
                    chevron: false,
                    isLast: true
                ) {
                    Button(permissionActionLabel) {
                        handlePermissionAction()
                    }
                }
            }

            SettingsSection(label: String(localized: "voice_section_system_tts"),
                            footer: String(localized: "voice_tts_footer")) {
                SettingsRow(label: String(localized: "voice_voice_name"), chevron: false) {
                    Picker("voice_voice_name", selection: voiceBinding) {
                        ForEach(capability.voices) { voice in
                            Text("\(voice.name) · \(voice.language)").tag(voice.id)
                        }
                    }
                    .labelsHidden()
                    .frame(maxWidth: 190)
                }
                SettingsRow(
                    label: String(localized: "voice_speech_rate"),
                    value: capability.speed.formatted(.number.precision(.fractionLength(1))) + "x",
                    chevron: false
                ) {
                    Slider(value: speedBinding, in: 0.5...2, step: 0.1)
                        .frame(width: 110)
                        .tint(t.accent)
                }
                SettingsRow(label: String(localized: "voice_auto_play"), chevron: false) {
                    LXToggle(isOn: autoPlayBinding)
                }
                SettingsRow(label: String(localized: "voice_preview"), chevron: false, isLast: true) {
                    Button(capability.isPreviewing ? String(localized: "voice_playing") : String(localized: "voice_play_sample")) {
                        Task { await capability.preview() }
                    }
                    .disabled(capability.isPreviewing)
                }
            }

            if let error = capability.errorMessage {
                BlurbText(text: error)
            }
        }
        .task { capability.reloadFromDefaults() }
        .onChange(of: scenePhase) { _, phase in
            guard phase == .active else { return }
            saveMessage = nil
            capability.reloadFromDefaults()
        }
        .onDisappear {
            Task { await capability.stopPreview() }
        }
    }

    private var languageBinding: Binding<String> {
        Binding(
            get: { capability.language },
            set: { value in
                saveMessage = nil
                capability.setLanguage(value)
                app.voiceLanguage = value
            }
        )
    }

    private var modeBinding: Binding<VoiceRecognitionMode> {
        Binding(
            get: { capability.mode },
            set: { value in
                saveMessage = nil
                capability.setMode(value)
                app.voiceRecognitionMode = value.rawValue
            }
        )
    }

    private var voiceBinding: Binding<String> {
        Binding(
            get: { capability.selectedVoice?.id ?? "" },
            set: { value in
                saveMessage = nil
                capability.setVoice(value)
            }
        )
    }

    private var speedBinding: Binding<Double> {
        Binding(get: { capability.speed }, set: capability.setSpeed)
    }

    private var autoPlayBinding: Binding<Bool> {
        Binding(get: { capability.autoPlay }, set: capability.setAutoPlay)
    }

    private var permissionActionLabel: String {
        let speechDenied = capability.speechAuthorization == .denied
            || capability.speechAuthorization == .restricted
        let microphoneDenied = capability.microphonePermissionStatus == .denied
        return speechDenied || microphoneDenied ? String(localized: "voice_system_settings") : String(localized: "voice_check_permissions")
    }

    private func handlePermissionAction() {
        let speechDenied = capability.speechAuthorization == .denied
            || capability.speechAuthorization == .restricted
        let microphoneDenied = capability.microphonePermissionStatus == .denied
        if speechDenied || microphoneDenied {
            guard let settingsURL = URL(string: UIApplication.openSettingsURLString) else { return }
            openURL(settingsURL)
        } else {
            Task { await capability.requestPermissions() }
        }
    }

    private func saveVoiceConfiguration() {
        let readiness = capability.saveConfiguration()
        app.voiceLanguage = capability.language
        app.voiceRecognitionMode = capability.mode.rawValue
        saveMessage = VoiceCapabilityModel.configurationSaveMessage(for: readiness)
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
        Text(text).font(.system(size: 11.5)).foregroundColor(t.text3).lineSpacing(4)
            .frame(maxWidth: .infinity, alignment: .leading).padding(.bottom, 14)
    }
}
