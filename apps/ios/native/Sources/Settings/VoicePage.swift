import SwiftUI

// MARK: - Voice settings

/// Voice settings as a native grouped form, in the same order as the desktop
/// page: input, output (with voice, speed and auto-play), offline models and a
/// collapsed "advanced & permissions" block. Changes apply immediately.
struct VoicePage: View {
    @Environment(\.theme) private var t
    @Environment(VoiceCapabilityModel.self) private var capability
    @Environment(\.openURL) private var openURL
    @Environment(\.scenePhase) private var scenePhase
    @State private var modelsExpanded = false
    @State private var advancedExpanded = false
    let store: SettingsStore

    var body: some View {
        Form {
            inputSection
            outputSection
            if let error = capability.errorMessage {
                Section {
                    Text(error).font(.footnote).foregroundStyle(t.danger)
                }
                .listRowBackground(t.surface)
            }
            offlineModelsSection
            advancedSection
        }
        .scrollContentBackground(.hidden)
        .background(t.windowBg)
        .task {
            capability.reloadFromDefaults()
            modelsExpanded = capability.mode == .onDevice
                || capability.speechMode == .offline
                || capability.allOfflineModels.contains(where: \.isBusyOrFailed)
            advancedExpanded = capability.microphonePermissionStatus != .granted
            await capability.refreshCloudCapabilities()
        }
        .onChange(of: capability.mode) { _, mode in if mode == .onDevice { modelsExpanded = true } }
        .onChange(of: capability.speechMode) { _, mode in if mode == .offline { modelsExpanded = true } }
        .task(id: capability.configurationRevision) { await capability.refreshCloudCapabilities() }
        .onChange(of: scenePhase) { _, phase in
            guard phase == .active else { return }
            capability.reloadFromDefaults()
        }
        .onDisappear {
            Task { await capability.stopPreview() }
        }
    }

    // MARK: Input

    private var inputSection: some View {
        Section {
            Picker(String(localized: "voice_recognition_language"), selection: Binding(
                get: { capability.language }, set: capability.setLanguage
            )) {
                ForEach(capability.languageOptions) { option in
                    Text(option.title).tag(option.id)
                }
            }
            Picker(String(localized: "voice_recognition_mode"), selection: Binding(
                get: { capability.mode }, set: capability.setMode
            )) {
                ForEach(capability.recognitionModes) { mode in
                    Text(mode.title).tag(mode)
                }
            }
            LabeledContent {
                Text(capability.availabilityLabel(capability.recognitionRoutePreview.reason))
                    .foregroundStyle(capability.recognitionRoutePreview.status == .ready ? t.ok : t.text3)
            } label: {
                Text(String(localized: "audio_availability")).foregroundStyle(t.text)
            }
            if capability.mode == .onDevice {
                offlineModelPicker(.stt)
            }
            VoiceCloudRows(capability: capability, kind: .recognition)
        } header: {
            Text(String(localized: "audio_settings_input"))
        } footer: {
            Text("\(String(localized: "voice_effective_language")) · \(capability.effectiveLanguageLabel)\n\(String(localized: "audio_language_hint"))")
        }
        .listRowBackground(t.surface)
    }

    // MARK: Output

    private var outputSection: some View {
        Section {
            Picker(String(localized: "audio_source_title"), selection: Binding(
                get: { capability.speechMode }, set: capability.setSpeechMode
            )) {
                ForEach(capability.speechModes) { mode in
                    Text(mode.title).tag(mode)
                }
            }
            LabeledContent {
                Text(capability.availabilityLabel(capability.speechRoutePreview.reason))
                    .foregroundStyle(capability.speechRoutePreview.status == .ready ? t.ok : t.text3)
            } label: {
                Text(String(localized: "audio_availability")).foregroundStyle(t.text)
            }
            if capability.speechMode == .offline {
                offlineModelPicker(.tts)
            }
            VoiceCloudRows(capability: capability, kind: .speech)
            voiceRow
            if capability.speechMode != .cloud || capability.cloudPreviewSupported {
                Button {
                    Task { await capability.preview() }
                } label: {
                    HStack {
                        Text(capability.isPreviewing
                            ? String(localized: "voice_playing")
                            : String(localized: "voice_play_sample"))
                            .foregroundStyle(t.accent)
                        Spacer(minLength: 0)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(capability.isPreviewing)
            }
            speedRow
            Toggle(isOn: Binding(get: { capability.autoPlay }, set: capability.setAutoPlay)) {
                Text(String(localized: "voice_auto_play")).foregroundStyle(t.text)
            }
            .tint(t.accent)
        } header: {
            Text(String(localized: "audio_settings_output"))
        } footer: {
            Text(String(localized: "settings_voice_playback_footer"))
        }
        .listRowBackground(t.surface)
    }

    @ViewBuilder private var voiceRow: some View {
        if capability.speechMode == .cloud {
            if let voices = capability.cloudService.capabilities[.speech]?.voices, !voices.isEmpty {
                Picker(String(localized: "voice_voice_name"), selection: Binding(
                    get: { capability.cloudVoiceID }, set: capability.setCloudVoice
                )) {
                    Text(String(localized: "audio_catalog_default")).tag("")
                    ForEach(voices, id: \.self) { Text($0).tag($0) }
                }
            }
        } else {
            Picker(String(localized: "voice_voice_name"), selection: Binding(
                get: { capability.voiceIdentifier }, set: capability.setVoice
            )) {
                ForEach(capability.speechVoiceOptions) { voice in
                    Text("\(voice.name) · \(voice.language)").tag(voice.id)
                }
            }
        }
    }

    @ViewBuilder private var speedRow: some View {
        if capability.speechMode == .cloud {
            LabeledContent {
                HStack(spacing: 10) {
                    Text(capability.speed.formatted(.number.precision(.fractionLength(1))) + "×")
                        .foregroundStyle(t.text3)
                    if capability.speed != 1 {
                        Button(String(localized: "audio_cloud_rate_reset")) { capability.setSpeed(1) }
                            .buttonStyle(.plain)
                            .foregroundStyle(t.accent)
                    }
                }
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text(String(localized: "voice_speech_rate")).foregroundStyle(t.text)
                    Text(String(localized: "audio_cloud_rate_fixed")).font(.footnote).foregroundStyle(t.text4)
                }
            }
        } else {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Text(String(localized: "voice_speech_rate")).foregroundStyle(t.text)
                    Spacer(minLength: 8)
                    Text(capability.speed.formatted(.number.precision(.fractionLength(1))) + "×")
                        .foregroundStyle(t.text3)
                }
                HStack(spacing: 8) {
                    Button {
                        capability.setSpeed(((capability.speed - 0.1) * 10).rounded() / 10)
                    } label: {
                        Label(String(localized: "voice_rate_decrease"), systemImage: "minus")
                            .labelStyle(.iconOnly)
                            .frame(minWidth: 44, minHeight: 44)
                    }
                    .buttonStyle(.borderless)
                    .disabled(capability.speed <= 0.5)
                    .accessibilityIdentifier("voice.rate.decrease")

                    Slider(value: Binding(get: { capability.speed }, set: capability.setSpeed), in: 0.5...2, step: 0.1)
                        .tint(t.accent)
                        .accessibilityLabel(Text("voice_speech_rate"))

                    Button {
                        capability.setSpeed(((capability.speed + 0.1) * 10).rounded() / 10)
                    } label: {
                        Label(String(localized: "voice_rate_increase"), systemImage: "plus")
                            .labelStyle(.iconOnly)
                            .frame(minWidth: 44, minHeight: 44)
                    }
                    .buttonStyle(.borderless)
                    .disabled(capability.speed >= 2)
                    .accessibilityIdentifier("voice.rate.increase")
                }
                .tint(t.accent)
            }
        }
    }

    private func offlineModelPicker(_ kind: GeneratedVoiceModelKind) -> some View {
        Picker(
            String(localized: kind == .stt ? "audio_offline_recognition_models" : "audio_offline_speech_models"),
            selection: Binding(get: {
                let configuration = capability.configurationSnapshot.configuration
                return (kind == .stt ? configuration.recognition.offlineModelId : configuration.speech.offlineModelId) ?? ""
            }, set: { capability.setOfflineModel($0.isEmpty ? nil : $0, for: kind) })
        ) {
            Text(String(localized: "audio_language_default_model")).tag("")
            ForEach(capability.offlineModels(for: kind)) { status in
                Text("\(status.displayName) · \(status.state.isReady ? String(localized: "audio_model_installed") : String(localized: "audio_model_not_installed"))")
                    .tag(status.id)
            }
        }
        .accessibilityIdentifier(kind == .stt ? "voice.offline-recognition-model" : "voice.offline-speech-model")
    }

    // MARK: Offline models

    private var offlineModelsSection: some View {
        let models = capability.allOfflineModels
        let installed = models.filter { $0.state.isReady }.count
        return Section {
            DisclosureGroup(isExpanded: $modelsExpanded) {
                ForEach(models) { status in
                    offlineModelRow(status)
                }
            } label: {
                LabeledContent {
                    Text(String(localized: "voice_offline_models_installed_count \(installed)"))
                        .foregroundStyle(t.text3)
                } label: {
                    Text(String(localized: "audio_settings_offline_models")).foregroundStyle(t.text)
                }
            }
            .tint(t.accent)
        } footer: {
            Text(String(localized: "audio_offline_models_hint"))
        }
        .listRowBackground(t.surface)
    }

    private func offlineModelRow(_ status: VoiceOfflineModelStatus) -> some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(status.displayName).foregroundStyle(t.text)
                Text("\(status.model.kind == .stt ? String(localized: "settings_section_voice_input") : String(localized: "audio_settings_output")) · \(status.sizeLabel)")
                    .font(.footnote)
                    .foregroundStyle(t.text4)
                Text(stateLabel(status.state))
                    .font(.footnote)
                    .foregroundStyle(stateColor(status.state))
                    .lineLimit(2)
            }
            Spacer(minLength: 8)
            Button(actionLabel(status.state)) { perform(status) }
                .buttonStyle(.plain)
                .foregroundStyle(status.state.isReady ? t.danger : t.accent)
        }
    }

    private func perform(_ status: VoiceOfflineModelStatus) {
        switch status.state {
        case .ready: capability.removeOfflineModel(status.id)
        case .queued, .downloading, .verifying, .extracting: capability.cancelOfflineModel(status.id)
        case .notInstalled, .failed: capability.installOfflineModel(status.id)
        }
    }

    private func actionLabel(_ state: VoiceModelState) -> String {
        switch state {
        case .ready: String(localized: "common_delete")
        case .queued, .downloading, .verifying, .extracting: String(localized: "common_cancel")
        case .failed: String(localized: "common_retry")
        case .notInstalled: String(localized: "voice_offline_download")
        }
    }

    private func stateColor(_ state: VoiceModelState) -> Color {
        switch state {
        case .ready: t.ok
        case .failed: t.danger
        default: t.text3
        }
    }

    private func stateLabel(_ state: VoiceModelState) -> String {
        switch state {
        case .notInstalled: String(localized: "voice_model_state_not_installed")
        case .queued: String(localized: "voice_model_state_queued")
        case let .downloading(received, total):
            total > 0
                ? "\(String(localized: "voice_model_state_downloading")) \(min(100, Int(Double(received) / Double(total) * 100)))%"
                : String(localized: "voice_model_state_downloading")
        case .verifying: String(localized: "voice_model_state_verifying")
        case .extracting: String(localized: "voice_model_state_installing")
        case .ready: String(localized: "voice_model_state_ready")
        case let .failed(message): "\(String(localized: "voice_model_state_failed")): \(message)"
        }
    }

    // MARK: Advanced & permissions

    private var advancedSection: some View {
        Section {
            DisclosureGroup(isExpanded: $advancedExpanded) {
                conversationRows
                permissionRows
            } label: {
                Text(String(localized: "audio_settings_advanced")).foregroundStyle(t.text)
            }
            .tint(t.accent)
        } footer: {
            Text(String(localized: "settings_voice_permissions_footer"))
        }
        .listRowBackground(t.surface)
    }

    @ViewBuilder private var conversationRows: some View {
        let preference = capability.conversationPreference
        let realtimeSupported = capability.cloudService.realtimeCapability?.supported == true || preference.mode == "realtime"
        if realtimeSupported {
            Picker(String(localized: "audio_conversation_section"), selection: Binding(get: { preference.mode }, set: { mode in
                var next = preference; next.mode = mode; capability.setConversationPreference(next)
            })) {
                Text(String(localized: "audio_agent_conversation")).tag("agent")
                Text(String(localized: "audio_realtime_conversation")).tag("realtime")
            }
        } else {
            LabeledContent {
                Text(String(localized: "audio_agent_conversation")).foregroundStyle(t.text3)
            } label: {
                Text(String(localized: "audio_conversation_section")).foregroundStyle(t.text)
            }
        }
        if preference.mode == "realtime" {
            Picker(String(localized: "audio_provider_binding"), selection: Binding(get: { preference.cloud.binding }, set: { binding in
                var next = preference; next.cloud = AudioCloudBinding(binding: binding); next.voice = nil; capability.setConversationPreference(next)
            })) {
                Text(String(localized: "audio_follow_session")).tag("follow_session")
                Text(String(localized: "audio_explicit_profile")).tag("explicit_profile")
            }
            if preference.cloud.binding == "explicit_profile" {
                Picker(String(localized: "audio_provider_profile"), selection: Binding(get: { preference.cloud.profileId ?? "" }, set: { profileID in
                    var next = preference; next.cloud.profileId = profileID.isEmpty ? nil : profileID; next.cloud.modelId = nil; next.voice = nil
                    capability.setConversationPreference(next)
                })) {
                    Text(String(localized: "audio_choose_profile")).tag("")
                    ForEach(capability.cloudProfiles, id: \.id) { Text($0.displayName).tag($0.id) }
                }
            }
            Picker(String(localized: "audio_model"), selection: Binding(get: { preference.cloud.modelId ?? "" }, set: { modelID in
                var next = preference; next.cloud.modelId = modelID.isEmpty ? nil : modelID; next.voice = nil
                capability.setConversationPreference(next)
            })) {
                Text(String(localized: "audio_catalog_default")).tag("")
                ForEach((capability.cloudService.realtimeCapability?.models ?? []).compactMap { $0 }, id: \.self) { Text($0).tag($0) }
            }
            if let voices = capability.cloudService.realtimeCapability?.voices, !voices.isEmpty {
                Picker(String(localized: "voice_voice_name"), selection: Binding(get: { preference.voice?.id ?? "" }, set: { id in
                    var next = preference
                    next.voice = id.isEmpty ? nil : AudioVoiceSelection(
                        source: .provider, id: id,
                        modelId: preference.cloud.modelId ?? capability.cloudService.realtimeCapability?.modelID,
                        profileId: capability.cloudService.profileID(for: preference.cloud)
                    )
                    capability.setConversationPreference(next)
                })) {
                    Text(String(localized: "audio_catalog_default")).tag("")
                    ForEach(voices, id: \.self) { Text($0).tag($0) }
                    if let selected = preference.voice?.id, !voices.contains(selected) { Text(selected).tag(selected) }
                }
            }
            Picker(String(localized: "audio_realtime_interaction"), selection: Binding(get: { preference.interaction }, set: { value in
                var next = preference; next.interaction = value; capability.setConversationPreference(next)
            })) {
                Text(String(localized: "audio_realtime_turn_based")).tag("turn_based")
                if preference.interaction == "interruptible" {
                    Text(String(localized: "audio_realtime_interruptible")).tag("interruptible").disabled(true)
                }
            }
            LabeledContent {
                Text(capability.availabilityLabel(capability.cloudService.realtimeCapability?.readiness ?? "unavailable"))
                    .foregroundStyle(t.text3)
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text(String(localized: "audio_realtime_conversation")).foregroundStyle(t.text)
                    if let reason = capability.cloudService.realtimeCapability?.reason
                        ?? (IOSRealtimeAudioService.shared.isAttached ? nil : String(localized: "audio_realtime_unavailable")) {
                        Text(reason).font(.footnote).foregroundStyle(t.text4)
                    }
                }
            }
        }
    }

    @ViewBuilder private var permissionRows: some View {
        let issues = capability.configurationReadiness.issues
        LabeledContent {
            Text(speechPermissionLabel).foregroundStyle(speechPermissionReady ? t.ok : t.text3)
        } label: {
            Text(String(localized: "voice_speech_recognition")).foregroundStyle(t.text)
        }
        LabeledContent {
            Text(capability.microphonePermission.label)
                .foregroundStyle(capability.microphonePermissionStatus == .granted ? t.ok : t.text3)
        } label: {
            Text(String(localized: "voice_microphone")).foregroundStyle(t.text)
        }
        LabeledContent {
            Text(capability.recognizerAvailable
                ? String(localized: "settings_status_available")
                : String(localized: "settings_status_needs_check"))
                .foregroundStyle(capability.recognizerAvailable ? t.ok : t.text3)
        } label: {
            Text(String(localized: "settings_voice_system_recognizer")).foregroundStyle(t.text)
        }
        if !issues.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text(String(localized: "settings_voice_blocking_issues")).foregroundStyle(t.text)
                Text(issues.map(\.message).joined(separator: " · "))
                    .font(.footnote)
                    .foregroundStyle(t.danger)
            }
        }
        Button(permissionActionLabel, action: handlePermissionAction)
            .buttonStyle(.plain)
            .foregroundStyle(t.accent)
    }

    private var speechPermissionReady: Bool {
        capability.mode == .onDevice || capability.speechAuthorization == .authorized
    }

    private var speechPermissionLabel: String {
        capability.mode == .onDevice
            ? String(localized: "voice_permission_not_required")
            : capability.speechPermission.label
    }

    private var permissionDenied: Bool {
        let speechDenied = (capability.mode == .automatic || capability.mode == .system)
            && (capability.speechAuthorization == .denied || capability.speechAuthorization == .restricted)
        return speechDenied || capability.microphonePermissionStatus == .denied
    }

    private var permissionActionLabel: String {
        permissionDenied
            ? String(localized: "voice_system_settings")
            : String(localized: "voice_check_permissions")
    }

    private func handlePermissionAction() {
        if permissionDenied {
            guard let settingsURL = URL(string: UIApplication.openSettingsURLString) else { return }
            openURL(settingsURL)
        } else {
            Task { await capability.requestPermissions() }
        }
    }
}

// MARK: - Cloud provider rows

/// Account, model and availability for a cloud-backed recognition or speech
/// source. Renders nothing unless that source is the selected one.
private struct VoiceCloudRows: View {
    @Environment(\.theme) private var t
    let capability: VoiceCapabilityModel
    let kind: AudioProviderKind

    private var isActive: Bool {
        kind == .recognition ? capability.mode == .cloud : capability.speechMode == .cloud
    }

    private var binding: AudioCloudBinding { capability.cloudBinding(for: kind) }

    var body: some View {
        if isActive {
            Picker(String(localized: "audio_provider_binding"), selection: Binding(
                get: { binding.binding },
                set: { value in
                    capability.setCloudBinding(
                        AudioCloudBinding(
                            binding: value,
                            profileId: value == "explicit_profile" ? binding.profileId : nil,
                            modelId: nil
                        ),
                        for: kind
                    )
                }
            )) {
                Text(String(localized: "audio_follow_session")).tag("follow_session")
                Text(String(localized: "audio_explicit_profile")).tag("explicit_profile")
            }
            if binding.binding == "explicit_profile" {
                Picker(String(localized: "audio_provider_profile"), selection: Binding(
                    get: { binding.profileId ?? "" },
                    set: {
                        capability.setCloudBinding(
                            AudioCloudBinding(binding: "explicit_profile", profileId: $0.isEmpty ? nil : $0),
                            for: kind
                        )
                    }
                )) {
                    Text(String(localized: "audio_choose_profile")).tag("")
                    ForEach(capability.cloudProfiles, id: \.id) { Text($0.displayName).tag($0.id) }
                }
            }
            Picker(String(localized: "audio_model"), selection: Binding(
                get: { binding.modelId ?? "" },
                set: {
                    capability.setCloudBinding(
                        AudioCloudBinding(binding: binding.binding, profileId: binding.profileId, modelId: $0.isEmpty ? nil : $0),
                        for: kind
                    )
                }
            )) {
                Text(String(localized: "audio_catalog_default")).tag("")
                let modelIDs = (capability.cloudService.capabilities[kind]?.route.modelIds ?? []).compactMap { $0 }
                ForEach(modelIDs, id: \.self) { Text($0).tag($0) }
                if let selected = binding.modelId, !modelIDs.contains(selected) {
                    Text(selected).tag(selected)
                }
            }
            if let reason = capability.cloudService.capabilities[kind]?.reason ?? capability.cloudService.lastError {
                Text(reason).font(.footnote).foregroundStyle(t.text4)
            }
        }
    }
}
