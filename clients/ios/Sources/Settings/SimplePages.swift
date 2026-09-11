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
    @Environment(VoiceCapabilityModel.self) private var capability
    @Environment(\.openURL) private var openURL
    @Environment(\.scenePhase) private var scenePhase
    let store: SettingsStore

    var body: some View {
        VStack(spacing: 0) {
            VoiceRecognitionSettingsSection(capability: capability)
            VoiceSpeechSettingsSection(capability: capability)
            if !capability.offlinePackStates.isEmpty {
                VoiceOfflineModelsSettingsSection(capability: capability)
            }
            VoicePlaybackSettingsSection(capability: capability)
            VoiceAccessSettingsSection(
                capability: capability,
                permissionAction: handlePermissionAction
            )

            if let error = capability.errorMessage {
                BlurbText(text: error)
            }
        }
        .task { capability.reloadFromDefaults() }
        .onChange(of: scenePhase) { _, phase in
            guard phase == .active else { return }
            capability.reloadFromDefaults()
        }
        .onDisappear {
            Task { await capability.stopPreview() }
        }
    }

    private func handlePermissionAction() {
        let speechDenied = capability.mode == .automatic && (capability.speechAuthorization == .denied
            || capability.speechAuthorization == .restricted
        )
        let microphoneDenied = capability.microphonePermissionStatus == .denied
        if speechDenied || microphoneDenied {
            guard let settingsURL = URL(string: UIApplication.openSettingsURLString) else { return }
            openURL(settingsURL)
        } else {
            Task { await capability.requestPermissions() }
        }
    }
}

private struct VoiceRecognitionSettingsSection: View {
    let capability: VoiceCapabilityModel

    var body: some View {
        SettingsSection(
            label: String(localized: "settings_voice_listen_section"),
            footer: String(localized: "settings_voice_listen_footer")
        ) {
            SettingsRow(
                label: String(localized: "voice_recognition_mode"),
                sub: capability.mode.detail,
                chevron: false
            ) {
                Picker("voice_recognition_mode", selection: modeBinding) {
                    ForEach(VoiceRecognitionMode.allCases) { mode in
                        Text(mode.title).tag(mode)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 170)
            }
            SettingsRow(
                label: String(localized: "voice_recognition_language"),
                sub: "\(String(localized: "voice_effective_language")) · \(capability.effectiveLanguageLabel)",
                chevron: false
            ) {
                Picker("voice_recognition_language", selection: languageBinding) {
                    ForEach(capability.languageOptions) { option in
                        Text(option.title).tag(option.id)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 170)
            }
            SettingsRow(
                label: String(localized: "settings_voice_effective_backend"),
                sub: capability.effectiveRecognitionStatus.fallbackReason
                    ?? capability.effectiveRecognitionStatus.detail,
                value: capability.effectiveRecognitionStatus.modeLabel,
                chevron: false,
                isLast: true
            )
        }
    }

    private var languageBinding: Binding<String> {
        Binding(get: { capability.language }, set: capability.setLanguage)
    }

    private var modeBinding: Binding<VoiceRecognitionMode> {
        Binding(get: { capability.mode }, set: capability.setMode)
    }
}

private struct VoiceSpeechSettingsSection: View {
    let capability: VoiceCapabilityModel

    var body: some View {
        SettingsSection(
            label: String(localized: "settings_voice_speak_section"),
            footer: String(localized: "settings_voice_speak_footer")
        ) {
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
                label: String(localized: "settings_voice_requested_voice"),
                value: capability.requestedVoiceLabel,
                chevron: false
            )
            SettingsRow(
                label: String(localized: "settings_voice_effective_voice"),
                sub: capability.selectedVoice?.language,
                value: capability.effectiveVoiceLabel,
                chevron: false
            )
            SettingsRow(
                label: String(localized: "voice_preview"),
                chevron: false,
                isLast: true
            ) {
                Button(
                    capability.isPreviewing
                        ? String(localized: "voice_playing")
                        : String(localized: "voice_play_sample")
                ) {
                    Task { await capability.preview() }
                }
                .disabled(capability.isPreviewing || capability.selectedVoice == nil)
            }
        }
    }

    private var voiceBinding: Binding<String> {
        Binding(
            get: { capability.selectedVoice?.id ?? capability.voiceIdentifier },
            set: capability.setVoice
        )
    }
}

private struct VoiceOfflineModelsSettingsSection: View {
    @Environment(\.theme) private var theme
    let capability: VoiceCapabilityModel

    var body: some View {
        SettingsSection(
            label: String(localized: "settings_voice_model_section"),
            footer: String(localized: "settings_voice_model_footer")
        ) {
            ForEach(capability.offlinePackStates) { status in
                SettingsRow(
                    label: displayName(for: status),
                    sub: status.model.license,
                    value: stateLabel(status.state),
                    valueColor: status.state.isReady ? theme.ok : theme.text3,
                    chevron: false
                )
            }
            SettingsRow(
                label: String(localized: "voice_offline_models_action"),
                sub: capability.effectiveLanguageLabel,
                chevron: false,
                isLast: true
            ) {
                Button(
                    downloadActive
                        ? String(localized: "common_cancel")
                        : String(localized: "voice_offline_download")
                ) {
                    if downloadActive {
                        capability.cancelOfflinePackDownload()
                    } else {
                        capability.downloadOfflinePack()
                    }
                }
            }
        }
    }

    private var downloadActive: Bool {
        capability.offlinePackStates.contains { status in
            switch status.state {
            case .queued, .downloading, .verifying, .extracting: true
            default: false
            }
        }
    }

    private func displayName(for status: VoiceOfflineModelStatus) -> String {
        status.model.displayName[Locale.current.language.languageCode?.identifier ?? "en"]
            ?? status.model.displayName["en"]
            ?? status.model.id
    }

    private func stateLabel(_ state: VoiceModelState) -> String {
        switch state {
        case .notInstalled: String(localized: "voice_model_state_not_installed")
        case .queued: String(localized: "voice_model_state_queued")
        case .downloading: String(localized: "voice_model_state_downloading")
        case .verifying: String(localized: "voice_model_state_verifying")
        case .extracting: String(localized: "voice_model_state_installing")
        case .ready: String(localized: "voice_model_state_ready")
        case let .failed(message): "\(String(localized: "voice_model_state_failed")): \(message)"
        }
    }
}

private struct VoicePlaybackSettingsSection: View {
    @Environment(\.theme) private var theme
    let capability: VoiceCapabilityModel

    var body: some View {
        SettingsSection(
            label: String(localized: "settings_voice_playback_options"),
            footer: String(localized: "settings_voice_playback_footer")
        ) {
            SettingsRow(
                label: String(localized: "voice_speech_rate"),
                value: capability.speed.formatted(.number.precision(.fractionLength(1))) + "x",
                chevron: false
            ) {
                Slider(value: speedBinding, in: 0.5...2, step: 0.1)
                    .frame(width: 110)
                    .tint(theme.accent)
            }
            SettingsRow(
                label: String(localized: "voice_auto_play"),
                chevron: false,
                isLast: true
            ) {
                LXToggle(isOn: autoPlayBinding)
            }
        }
    }

    private var speedBinding: Binding<Double> {
        Binding(get: { capability.speed }, set: capability.setSpeed)
    }

    private var autoPlayBinding: Binding<Bool> {
        Binding(get: { capability.autoPlay }, set: capability.setAutoPlay)
    }
}

private struct VoiceAccessSettingsSection: View {
    @Environment(\.theme) private var theme
    let capability: VoiceCapabilityModel
    let permissionAction: () -> Void

    var body: some View {
        SettingsSection(
            label: String(localized: "settings_voice_permissions_section"),
            footer: String(localized: "settings_voice_permissions_footer")
        ) {
            SettingsRow(
                label: String(localized: "voice_speech_recognition"),
                sub: speechPermissionDetail,
                value: speechPermissionLabel,
                valueColor: speechPermissionReady ? theme.ok : theme.text3,
                chevron: false
            )
            SettingsRow(
                label: String(localized: "voice_microphone"),
                sub: capability.microphonePermission.detail,
                value: capability.microphonePermission.label,
                valueColor: capability.microphonePermissionStatus == .granted
                    ? theme.ok : theme.text3,
                chevron: false
            )
            SettingsRow(
                label: String(localized: "settings_voice_system_recognizer"),
                sub: capability.effectiveRecognitionLabel,
                value: capability.recognizerAvailable
                    ? String(localized: "settings_status_available")
                    : String(localized: "settings_status_needs_check"),
                valueColor: capability.recognizerAvailable ? theme.ok : theme.text3,
                chevron: false
            )
            SettingsRow(
                label: String(localized: "settings_voice_blocking_issues"),
                sub: blockingIssueSummary,
                value: blockingIssues.count.formatted(),
                valueColor: blockingIssues.isEmpty ? theme.ok : theme.danger,
                chevron: false,
                isLast: true
            ) {
                Button(permissionActionLabel, action: permissionAction)
            }
        }
    }

    private var blockingIssues: [VoiceConfigurationIssue] {
        capability.configurationReadiness.issues
    }

    private var blockingIssueSummary: String {
        let summary = blockingIssues.map(\.message).joined(separator: " · ")
        return summary.isEmpty ? String(localized: "settings_voice_all_clear") : summary
    }

    private var speechPermissionReady: Bool {
        capability.mode == .onDevice || capability.speechAuthorization == .authorized
    }

    private var speechPermissionLabel: String {
        capability.mode == .onDevice
            ? String(localized: "voice_permission_not_required")
            : capability.speechPermission.label
    }

    private var speechPermissionDetail: String {
        capability.mode == .onDevice
            ? String(localized: "voice_speech_permission_not_required_detail")
            : capability.speechPermission.detail
    }

    private var permissionActionLabel: String {
        let speechDenied = capability.mode == .automatic
            && (capability.speechAuthorization == .denied
                || capability.speechAuthorization == .restricted)
        let microphoneDenied = capability.microphonePermissionStatus == .denied
        return speechDenied || microphoneDenied
            ? String(localized: "voice_system_settings")
            : String(localized: "voice_check_permissions")
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
