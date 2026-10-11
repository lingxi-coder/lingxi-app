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
            VoiceConversationSettingsSection(capability: capability)
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

            VoiceAudioStatusSettingsSection(capability: capability)
            if let error = capability.errorMessage {
                BlurbText(text: error)
            }
        }
        .task { capability.reloadFromDefaults(); await capability.refreshCloudCapabilities() }
        .task(id: capability.configurationRevision) { await capability.refreshCloudCapabilities() }
        .onChange(of: scenePhase) { _, phase in
            guard phase == .active else { return }
            capability.reloadFromDefaults()
        }
        .onDisappear {
            Task { await capability.stopPreview() }
        }
    }

    private func handlePermissionAction() {
        let speechDenied = (capability.mode == .automatic || capability.mode == .system) && (capability.speechAuthorization == .denied
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

private struct VoiceConversationSettingsSection: View {
    let capability: VoiceCapabilityModel
    private var preference: AudioConversationPreference { capability.conversationPreference }

    var body: some View {
        SettingsSection(label: String(localized: "audio_conversation_section")) {
            SettingsRow(label: String(localized: "audio_agent_conversation"), sub: String(localized: "audio_agent_conversation_detail"), chevron: false) {
                if capability.cloudService.realtimeCapability?.supported == true || preference.mode == "realtime" {
                    Picker("audio_conversation_section", selection: Binding(get: { preference.mode }, set: { mode in
                        var next = preference; next.mode = mode; capability.setConversationPreference(next)
                    })) {
                        Text(String(localized: "audio_agent_conversation")).tag("agent")
                        Text(String(localized: "audio_realtime_conversation")).tag("realtime")
                    }.labelsHidden().frame(maxWidth: 190)
                }
            }
            Group {
                SettingsRow(label: String(localized: "audio_provider_binding"), sub: capability.cloudService.profileID(for: preference.cloud), chevron: false) {
                    Picker("audio_provider_binding", selection: Binding(get: { preference.cloud.binding }, set: { binding in
                        var next = preference; next.cloud = AudioCloudBinding(binding: binding); next.voice = nil; capability.setConversationPreference(next)
                    })) {
                        Text(String(localized: "audio_follow_session")).tag("follow_session")
                        Text(String(localized: "audio_explicit_profile")).tag("explicit_profile")
                    }.labelsHidden().frame(maxWidth: 190)
                }
                if preference.cloud.binding == "explicit_profile" {
                    SettingsRow(label: String(localized: "audio_provider_profile"), chevron: false) {
                        Picker("audio_provider_profile", selection: Binding(get: { preference.cloud.profileId ?? "" }, set: { profileID in
                            var next = preference; next.cloud.profileId = profileID.isEmpty ? nil : profileID; next.cloud.modelId = nil; next.voice = nil; capability.setConversationPreference(next)
                        })) {
                            Text(String(localized: "audio_choose_profile")).tag("")
                            ForEach(capability.cloudProfiles, id: \.id) { Text($0.displayName).tag($0.id) }
                        }.labelsHidden().frame(maxWidth: 190)
                    }
                }
                SettingsRow(label: String(localized: "audio_model"), sub: String(localized: "audio_independent_model"), chevron: false) {
                    Picker("audio_model", selection: Binding(get: { preference.cloud.modelId ?? "" }, set: { modelID in
                        var next = preference; next.cloud.modelId = modelID.isEmpty ? nil : modelID; next.voice = nil; capability.setConversationPreference(next)
                    })) {
                        Text(String(localized: "audio_catalog_default")).tag("")
                        ForEach((capability.cloudService.realtimeCapability?.models ?? []).compactMap { $0 }, id: \.self) { Text($0).tag($0) }
                    }.labelsHidden().frame(maxWidth: 190)
                }
                if let voices = capability.cloudService.realtimeCapability?.voices, !voices.isEmpty {
                    SettingsRow(label: String(localized: "voice_voice_name"), chevron: false) {
                        Picker("voice_voice_name", selection: Binding(get: { preference.voice?.id ?? "" }, set: { id in
                            var next = preference
                            next.voice = id.isEmpty ? nil : AudioVoiceSelection(source: .provider, id: id,
                                modelId: preference.cloud.modelId ?? capability.cloudService.realtimeCapability?.modelID,
                                profileId: capability.cloudService.profileID(for: preference.cloud))
                            capability.setConversationPreference(next)
                        })) {
                            Text(String(localized: "audio_catalog_default")).tag("")
                            ForEach(voices, id: \.self) { Text($0).tag($0) }
                            if let selected = preference.voice?.id, !voices.contains(selected) { Text(selected).tag(selected) }
                        }.labelsHidden().frame(maxWidth: 190)
                    }
                }
                SettingsRow(label: String(localized: "audio_realtime_interaction"), sub: String(localized: "audio_realtime_turn_based_detail"), chevron: false) {
                    Picker("audio_realtime_interaction", selection: Binding(get: { preference.interaction }, set: { value in
                        var next = preference; next.interaction = value; capability.setConversationPreference(next)
                    })) {
                        Text(String(localized: "audio_realtime_turn_based")).tag("turn_based")
                        if preference.interaction == "interruptible" {
                            Text(String(localized: "audio_realtime_interruptible")).tag("interruptible").disabled(true)
                        }
                    }.labelsHidden().frame(maxWidth: 190)
                }
            }
            SettingsRow(label: String(localized: "audio_realtime_conversation"), sub: capability.cloudService.realtimeCapability?.reason ?? (IOSRealtimeAudioService.shared.isAttached ? nil : String(localized: "audio_realtime_unavailable")), value: capability.availabilityLabel(capability.cloudService.realtimeCapability?.readiness ?? "unavailable"), chevron: false, isLast: true)
        }
    }
}

private struct VoiceCloudBindingRows: View {
    let capability: VoiceCapabilityModel
    let kind: AudioProviderKind

    private var binding: AudioCloudBinding { capability.cloudBinding(for: kind) }

    var body: some View {
        SettingsRow(label: String(localized: "audio_provider_binding"), sub: capability.cloudService.profileID(for: binding), chevron: false) {
            Picker("audio_provider_binding", selection: Binding(get: { binding.binding }, set: { value in
                capability.setCloudBinding(AudioCloudBinding(binding: value, profileId: value == "explicit_profile" ? binding.profileId : nil, modelId: nil), for: kind)
            })) {
                Text(String(localized: "audio_follow_session")).tag("follow_session")
                Text(String(localized: "audio_explicit_profile")).tag("explicit_profile")
            }.labelsHidden().frame(maxWidth: 190)
        }
        if binding.binding == "explicit_profile" {
            SettingsRow(label: String(localized: "audio_provider_profile"), chevron: false) {
                Picker("audio_provider_profile", selection: Binding(get: { binding.profileId ?? "" }, set: {
                    capability.setCloudBinding(AudioCloudBinding(binding: "explicit_profile", profileId: $0.isEmpty ? nil : $0), for: kind)
                })) {
                    Text(String(localized: "audio_choose_profile")).tag("")
                    ForEach(capability.cloudProfiles, id: \.id) { Text($0.displayName).tag($0.id) }
                }.labelsHidden().frame(maxWidth: 190)
            }
        }
        SettingsRow(label: String(localized: "audio_model"), sub: String(localized: "audio_independent_model"), chevron: false) {
            Picker("audio_model", selection: Binding(get: { binding.modelId ?? "" }, set: {
                capability.setCloudBinding(AudioCloudBinding(binding: binding.binding, profileId: binding.profileId, modelId: $0.isEmpty ? nil : $0), for: kind)
            })) {
                Text(String(localized: "audio_catalog_default")).tag("")
                ForEach((capability.cloudService.capabilities[kind]?.route.modelIds ?? []).compactMap { $0 }, id: \.self) { Text($0).tag($0) }
                if let selected = binding.modelId, !(capability.cloudService.capabilities[kind]?.route.modelIds.contains(selected) ?? false) {
                    Text(selected).tag(selected)
                }
            }.labelsHidden().frame(maxWidth: 190)
        }
        if let reason = capability.cloudService.capabilities[kind]?.reason ?? capability.cloudService.lastError {
            SettingsRow(label: String(localized: "audio_availability"), sub: reason, chevron: false)
        }
    }
}

private struct VoiceAudioStatusSettingsSection: View {
    let capability: VoiceCapabilityModel

    var body: some View {
        SettingsSection(label: String(localized: "audio_status_section")) {
            SettingsRow(label: String(localized: "audio_input_status"), value: capability.availabilityLabel(capability.recognitionRoutePreview.reason), chevron: false)
            SettingsRow(label: String(localized: "audio_output_status"), value: capability.availabilityLabel(capability.speechRoutePreview.reason), chevron: false)
            SettingsRow(label: String(localized: "audio_interruption"), sub: String(localized: "audio_interruption_detail"), chevron: false, isLast: true)
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
                    ForEach(capability.recognitionModes) { mode in
                        Text(mode.title).tag(mode)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 170)
            }
            VoiceCloudBindingRows(capability: capability, kind: .recognition)
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
            SettingsRow(label: "Playback source", chevron: false) {
                Picker("Playback source", selection: speechModeBinding) {
                    ForEach(capability.speechModes) { mode in
                        Text(mode.title).tag(mode)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 190)
            }
            SettingsRow(
                label: "Preview route",
                sub: capability.speechRoutePreview.fallbackReason,
                value: capability.speechSourceLabel,
                chevron: false
            )
            VoiceCloudBindingRows(capability: capability, kind: .speech)
            if capability.speechMode == .cloud {
                if let voices = capability.cloudService.capabilities[.speech]?.voices, !voices.isEmpty {
                    SettingsRow(label: String(localized: "voice_voice_name"), chevron: false) {
                        Picker("voice_voice_name", selection: Binding(get: { capability.cloudVoiceID }, set: capability.setCloudVoice)) {
                            Text(String(localized: "audio_catalog_default")).tag("")
                            ForEach(voices, id: \.self) { Text($0).tag($0) }
                        }.labelsHidden()
                    }
                }
            } else {
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
            }
            if capability.speechMode != .cloud || capability.cloudPreviewSupported {
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
                .disabled(capability.isPreviewing)
            }
            }
        }
    }

    private var voiceBinding: Binding<String> {
        Binding(
            get: { capability.voiceIdentifier },
            set: capability.setVoice
        )
    }

    private var speechModeBinding: Binding<VoiceSpeechMode> {
        Binding(get: { capability.speechMode }, set: capability.setSpeechMode)
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
                sub: capability.speechMode == .cloud ? String(localized: "audio_cloud_rate_fixed") : nil,
                value: capability.speed.formatted(.number.precision(.fractionLength(1))) + "x",
                chevron: false
            ) {
                if capability.speechMode == .cloud {
                    if capability.speed != 1 {
                        Button(String(localized: "audio_cloud_rate_reset")) { capability.setSpeed(1) }
                    }
                } else {
                    Slider(value: speedBinding, in: 0.5...2, step: 0.1)
                        .frame(width: 110)
                        .tint(theme.accent)
                }
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
