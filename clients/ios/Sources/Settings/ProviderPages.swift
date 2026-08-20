import SwiftUI

// MARK: - Unified LLM provider management

/// The LLM settings screen owns the provider workflow. Rows render the
/// repository snapshot; edits live in a value draft until the user applies.
struct ProviderListPage: View {
    @Environment(\.theme) private var t
    @State private var repository = ProviderRepository.shared
    @State private var editingDraft: ProviderEditorDraft?
    @State private var showingRouting = false
    @State private var retryMaxAttemptsText = ""
    @State private var retryBackoffMsText = ""
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let kind: ProviderKind

    var body: some View {
        if kind != .llm {
            unsupportedProviderNotice
        } else {
            llmContent
                .sheet(item: $editingDraft) { draft in
                    ProviderEditorSheet(draft: draft) { editingDraft = nil }
                        .presentationDragIndicator(.visible)
                }
                .task {
                    retryMaxAttemptsText = String(repository.routingSettings.retryMaxAttempts)
                    retryBackoffMsText = String(repository.routingSettings.retryBackoffMs)
                    store.llmProviders = repository.legacyProviders()
                    await repository.refreshCatalog()
                    await repository.refreshCredentialStatus()
                }
        }
    }

    private var llmContent: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(String(localized: "provider_llm_blurb"))
                .font(.system(size: 11.5)).foregroundColor(t.text3).lineSpacing(4).padding(.bottom, 14)
            summaryCard
            SettingsSection(label: String(localized: "settings_provider_section_added_count \(repository.settingsSummary.totalCount)")) {
                if repository.profiles.isEmpty {
                    Text(String(localized: "provider_no_providers_added"))
                        .font(.system(size: 13)).foregroundColor(t.text4).frame(maxWidth: .infinity).padding(.vertical, 24)
                }
                ForEach(Array(repository.profiles.enumerated()), id: \.element.id) { index, state in
                    providerRow(state, isLast: index == repository.profiles.count - 1)
                }
            }
            Text(String(localized: "settings_provider_picker_intro"))
                .font(.system(size: 12))
                .foregroundStyle(t.text3)
                .lineSpacing(4)
                .padding(.horizontal, 4)
                .padding(.bottom, 10)
            SettingsSection(label: String(localized: "settings_title_add_llm")) {
                ForEach(Array(repository.catalogPresets.enumerated()), id: \.element.id) { index, preset in
                    presetRow(preset, isLast: index == repository.catalogPresets.count - 1)
                }
            }
            routingSection
        }
        .onChange(of: repository.syncRevision) { _, _ in
            store.llmProviders = repository.legacyProviders()
        }
    }

    private var summaryCard: some View {
        let summary = repository.settingsSummary
        return VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 12) {
                Circle().fill(summary.defaultProfile == nil ? t.text4 : t.accent).frame(width: 10, height: 10).padding(.top, 4)
                VStack(alignment: .leading, spacing: 3) {
                    Text(summary.defaultProfile.map { String(localized: "settings_provider_default \($0.name)") } ?? String(localized: "settings_provider_unconfigured"))
                        .font(.system(size: 15, weight: .semibold)).foregroundColor(t.text)
                    Text(summary.defaultModelID ?? "—").font(.system(size: 11.5, design: .monospaced)).foregroundColor(t.text4)
                }
                Spacer()
                Text(String(localized: "settings_providers_enabled_count \(summary.enabledCount)"))
                    .font(.system(size: 11.5, weight: .medium)).foregroundColor(t.text3)
            }
            if let active = summary.runtime.activeModelID, !summary.runtimeMatchesDefault, !active.isEmpty {
                Divider().overlay(t.border)
                HStack(spacing: 6) {
                    LXIcon(name: .warning, size: 13, color: t.text3, stroke: 1.8)
                    Text(String(localized: "settings_provider_runtime_active \(active)")).font(.system(size: 11.5)).foregroundColor(t.text3).lineLimit(2)
                }
            }
            if let error = summary.runtime.lastError, !error.isEmpty {
                Text(error).font(.system(size: 11.5)).foregroundColor(t.danger).lineLimit(3)
            }
        }
        .padding(14).background(t.surface).clipShape(RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(t.border, lineWidth: 0.5)).padding(.bottom, 20)
    }

    private func providerRow(_ state: ProviderProfileState, isLast: Bool) -> some View {
        let preset = repository.preset(for: state.profile.presetID)
        return SettingsRow(
            labelView: AnyView(HStack(spacing: 6) {
                Text(state.profile.name).font(.system(size: 14, weight: .medium)).foregroundColor(t.text)
                if state.profile.isDefault { badge(String(localized: "settings_badge_default"), t.accent) }
                if !state.profile.enabled { badge(String(localized: "settings_badge_disabled"), t.text4) }
                if state.oauthState?.signedIn == true { badge(String(localized: "settings_provider_status_oauth"), t.ok) }
            }),
            subView: AnyView(HStack(spacing: 5) {
                Circle().fill(state.legacyStatus.dot(t)).frame(width: 6, height: 6)
                Text("\(state.statusLabel) · \(state.profile.modelID.isEmpty ? "—" : state.profile.modelID)")
                    .font(.system(size: 11.5, design: .monospaced)).foregroundColor(t.text4).lineLimit(1)
            }),
            chevron: true, isLast: isLast,
            onTap: { editingDraft = repository.makeDraft(for: state.id) }
        ) {
            Text(String(state.profile.name.prefix(1))).font(.system(size: 13, weight: .bold)).foregroundColor(preset.color)
                .frame(width: 28, height: 28).background(preset.color.mix(with: t.surface, amount: 0.18))
                .clipShape(RoundedRectangle(cornerRadius: 7)).overlay(RoundedRectangle(cornerRadius: 7).stroke(preset.color.tint(0.28), lineWidth: 0.5))
        }
    }

    private func presetRow(_ preset: ProviderPreset, isLast: Bool) -> some View {
        SettingsRow(
            label: preset.name,
            sub: preset.sub,
            chevron: true,
            isLast: isLast,
            onTap: { editingDraft = repository.makeNewDraft(presetID: preset.id) }
        ) {
            Text(String(preset.name.prefix(1)))
                .font(.system(size: 13, weight: .bold))
                .foregroundStyle(preset.color)
                .frame(width: 28, height: 28)
                .background(preset.color.mix(with: t.surface, amount: 0.18))
                .clipShape(.rect(cornerRadius: 7))
                .overlay {
                    RoundedRectangle(cornerRadius: 7)
                        .stroke(preset.color.tint(0.28), lineWidth: 0.5)
                }
        }
        .accessibilityIdentifier("provider.preset.\(preset.id)")
    }

    private var routingSection: some View {
        DisclosureGroup(isExpanded: $showingRouting) {
            VStack(alignment: .leading, spacing: 12) {
                routeField(label: String(localized: "settings_provider_default_model"), text: .constant(repository.makeLaunchSnapshot().defaultModelID ?? String(localized: "settings_provider_unconfigured")), editable: false)
                routeField(label: String(localized: "settings_provider_max_retries"), text: $retryMaxAttemptsText)
                routeField(label: String(localized: "settings_provider_backoff_ms"), text: $retryBackoffMsText)
                fallbackList
                if let message = repository.routingMessage { Text(message).font(.system(size: 11.5)).foregroundColor(t.text3) }
                Button {
                    Task {
                        let applied = await repository.applyRoutingChanges(retryMaxAttemptsText: retryMaxAttemptsText, retryBackoffMsText: retryBackoffMsText)
                        if applied {
                            retryMaxAttemptsText = String(repository.routingSettings.retryMaxAttempts)
                            retryBackoffMsText = String(repository.routingSettings.retryBackoffMs)
                        }
                    }
                } label: {
                    Text(String(localized: "settings_provider_apply_routing")).font(.system(size: 13, weight: .semibold)).foregroundColor(.white)
                        .frame(maxWidth: .infinity).padding(.vertical, 10).background(hasPendingRoutingChanges ? t.accent : t.text4).clipShape(RoundedRectangle(cornerRadius: 10))
                }
                .disabled(!hasPendingRoutingChanges).buttonStyle(.plain)
            }
            .padding(.top, 12)
        } label: {
            Text(String(localized: "settings_provider_section_routing")).font(.system(size: 13, weight: .semibold)).foregroundColor(t.text2)
        }
        .tint(t.text3).padding(.horizontal, 14).padding(.vertical, 12).background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 14)).overlay(RoundedRectangle(cornerRadius: 14).stroke(t.border, lineWidth: 0.5))
    }

    private var fallbackList: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(String(localized: "settings_provider_fallback_order")).font(.system(size: 12.5, weight: .medium)).foregroundColor(t.text2)
            let candidates = repository.fallbackCandidates()
            if candidates.isEmpty {
                Text(String(localized: "settings_provider_fallback_hint")).font(.system(size: 11.5)).foregroundColor(t.text4)
            } else {
                ForEach(candidates) { candidate in
                    HStack(spacing: 8) {
                        Button { repository.toggleFallbackProfile(candidate.profileID) } label: {
                            Circle().strokeBorder(candidate.selected ? t.accent : t.border, lineWidth: 1.5).frame(width: 18, height: 18)
                                .overlay(candidate.selected ? Circle().fill(t.accent).frame(width: 9, height: 9) : nil)
                        }.buttonStyle(.plain)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(candidate.name).font(.system(size: 13, weight: .medium)).foregroundColor(t.text)
                            Text(candidate.modelID).font(.system(size: 11.5, design: .monospaced)).foregroundColor(t.text4)
                        }
                        Spacer()
                    }
                    .padding(.horizontal, 10).padding(.vertical, 8).background(t.windowBg).clipShape(RoundedRectangle(cornerRadius: 9))
                }
            }
        }
    }

    private var hasPendingRoutingChanges: Bool {
        repository.routingDirty || retryMaxAttemptsText.trimmingCharacters(in: .whitespacesAndNewlines) != String(repository.routingSettings.retryMaxAttempts) || retryBackoffMsText.trimmingCharacters(in: .whitespacesAndNewlines) != String(repository.routingSettings.retryBackoffMs)
    }

    @ViewBuilder private func routeField(label: String, text: Binding<String>, editable: Bool = true) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            FieldLabel(text: label)
            if editable { SettingsField(text: text) }
            else { Text(text.wrappedValue).font(.system(size: 12.5, design: .monospaced)).foregroundColor(t.text4).frame(maxWidth: .infinity, alignment: .leading).padding(11).background(t.windowBg).clipShape(RoundedRectangle(cornerRadius: 10)) }
        }
    }

    private func badge(_ text: String, _ color: Color) -> some View {
        Text(text).font(.system(size: 10, weight: .medium)).foregroundColor(color).padding(.horizontal, 6).padding(.vertical, 1).background(color.tint(0.18)).clipShape(RoundedRectangle(cornerRadius: 4))
    }

    private var unsupportedProviderNotice: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(String(localized: "settings_provider_not_connected")).font(.system(size: 13.5, weight: .semibold)).foregroundColor(t.text)
            Text(String(localized: "settings_provider_unsupported_notice \(kind.title)")) .font(.system(size: 11.5)).foregroundColor(t.text4).lineSpacing(4)
        }
        .frame(maxWidth: .infinity, alignment: .leading).padding(14).background(t.surface).clipShape(RoundedRectangle(cornerRadius: 12)).overlay(RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.5))
    }
}

// MARK: - Draft editor sheet

private struct ProviderEditorSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.theme) private var t
    @State private var repository = ProviderRepository.shared
    @State private var draft: ProviderEditorDraft
    @State private var showKey = false
    @State private var editingStoredCredential = false
    @State private var saving = false
    @State private var removing = false
    @State private var actionError: String?
    let onSaved: () -> Void

    init(draft: ProviderEditorDraft, onSaved: @escaping () -> Void) {
        _draft = State(initialValue: draft); self.onSaved = onSaved
    }

    var body: some View {
        NavigationStack {
            ScrollView(showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    statusBanner
                    FieldLabel(text: String(localized: "settings_display_name"))
                    SettingsField(text: textBinding(\.name), mono: false).padding(.bottom, 14)
                    if repository.isOfficialEndpoint(for: draft.profile) {
                        officialEndpoint
                    } else {
                        FieldLabel(text: String(localized: "settings_api_url"))
                        SettingsField(text: textBinding(\.baseURL), placeholder: repository.preset(for: draft.profile.presetID).defaultUrl)
                        FieldHint(String(localized: "settings_provider_custom_url_hint"))
                    }
                    credentialSection
                    modelSection
                    SettingsSection {
                        SettingsRow(label: String(localized: "settings_provider_enable"), chevron: false) { LXToggle(isOn: boolBinding(\.enabled)) }
                        SettingsRow(label: String(localized: "settings_provider_set_default"), chevron: false, isLast: true, onTap: { draft.profile.isDefault = true; draft.profile.enabled = true }) {
                            if draft.profile.isDefault { LXIcon(name: .check, size: 16, color: t.accent, stroke: 2.2) }
                        }
                    }
                }.padding(16)
            }
            .disabled(saving || removing || draft.operationInFlight || draft.connectionState == .testing)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                bottomActions
            }
            .navigationTitle(draft.profile.name).navigationBarTitleDisplayMode(.inline)
        }
    }

    private var bottomActions: some View {
        HStack(spacing: 9) {
            Button(String(localized: "common_cancel")) { dismiss() }
                .font(.system(size: 13.5, weight: .medium))
                .foregroundColor(t.text2)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 11)
                .background(t.windowBg)
                .clipShape(RoundedRectangle(cornerRadius: 10))
                .accessibilityIdentifier("provider.cancel")
            if !draft.isNew {
                Button {
                    removing = true
                    Task {
                        let removed = await repository.removeProfile(draft.id)
                        removing = false
                        if removed {
                            dismiss()
                            onSaved()
                        } else {
                            let message = repository.lastRepositoryError
                                ?? String(localized: "settings_provider_remove_failed")
                            actionError = message
                            draft.connectionState = .failed
                            draft.detailMessage = message
                        }
                    }
                } label: {
                    Text(String(localized: "settings_provider_remove"))
                        .font(.system(size: 13.5, weight: .medium))
                        .foregroundColor(t.danger)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 11)
                }
                .background(t.windowBg)
                .clipShape(RoundedRectangle(cornerRadius: 10))
            }
            Button(String(localized: "settings_provider_save_and_apply")) { save() }
                .font(.system(size: 13.5, weight: .semibold))
                .foregroundColor(.white)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 11)
                .background(saving || removing || draft.operationInFlight || draft.connectionState == .testing ? t.text4 : t.accent)
                .clipShape(RoundedRectangle(cornerRadius: 10))
                .accessibilityIdentifier("provider.save")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(.ultraThinMaterial)
        .overlay(alignment: .top) { Rectangle().fill(t.border).frame(height: 0.5) }
        .disabled(saving || removing || draft.operationInFlight || draft.connectionState == .testing)
    }

    private var statusBanner: some View {
        HStack(spacing: 8) {
            Circle().fill(statusColor).frame(width: 8, height: 8)
            VStack(alignment: .leading, spacing: 2) {
                Text(statusLabel).font(.system(size: 12.5, weight: .medium)).foregroundColor(t.text2)
                if let detail = draft.validationMessage ?? draft.detailMessage ?? actionError { Text(detail).font(.system(size: 11.5)).foregroundColor(t.text4).lineLimit(3) }
            }
            Spacer()
            Button(String(localized: "settings_provider_test_connection")) { testConnection() }
                .font(.system(size: 12, weight: .medium)).foregroundColor(t.text2).padding(.horizontal, 9).padding(.vertical, 6).background(t.windowBg).clipShape(RoundedRectangle(cornerRadius: 7)).disabled(draft.operationInFlight || draft.connectionState == .testing)
        }
        .padding(12).background(statusColor.tint(0.08)).clipShape(RoundedRectangle(cornerRadius: 10)).overlay(RoundedRectangle(cornerRadius: 10).stroke(statusColor.tint(0.24), lineWidth: 0.5)).padding(.bottom, 18)
    }

    private var officialEndpoint: some View {
        VStack(alignment: .leading, spacing: 6) {
            FieldLabel(text: String(localized: "settings_api_url"))
            Text(draft.profile.baseURL).font(.system(size: 12.5, design: .monospaced)).foregroundColor(t.text4).frame(maxWidth: .infinity, alignment: .leading).padding(11).background(t.windowBg).clipShape(RoundedRectangle(cornerRadius: 10))
            FieldHint(String(localized: "settings_provider_official_url_hint \(draft.profile.baseURL)"))
        }
    }

    private var credentialSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            if !isOAuthOnly {
                FieldLabel(text: String(localized: "settings_provider_api_key"))
                if draft.hasStoredAPIKey && !editingStoredCredential && draft.pendingSecret.isEmpty && !draft.clearCredentialOnApply {
                    HStack {
                        Text("••••••••••••").font(.system(size: 13.5, design: .monospaced)).foregroundColor(t.text); Spacer()
                        Button(String(localized: "settings_provider_replace_key")) {
                            editingStoredCredential = true
                            draft.clearCredentialOnApply = false
                        }.font(.system(size: 11, weight: .medium)).foregroundColor(t.text3)
                    }.padding(12).background(t.surface).clipShape(RoundedRectangle(cornerRadius: 10)).overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
                } else {
                    HStack(spacing: 0) {
                        Group {
                            if showKey {
                                TextField(String(localized: "settings_provider_api_key_placeholder"), text: $draft.pendingSecret)
                            } else {
                                SecureField(String(localized: "settings_provider_api_key_placeholder"), text: $draft.pendingSecret)
                            }
                        }
                            .font(.system(size: 13.5, design: .monospaced)).textInputAutocapitalization(.never).autocorrectionDisabled().padding(.vertical, 11).padding(.leading, 12)
                            .accessibilityIdentifier("provider.api-key")
                        Button(showKey ? String(localized: "settings_provider_hide_key") : String(localized: "settings_provider_show_key")) { showKey.toggle() }
                            .font(.system(size: 11, weight: .medium)).foregroundColor(t.text3).padding(.horizontal, 10)
                            .accessibilityIdentifier("provider.api-key.visibility")
                    }.background(t.surface).clipShape(RoundedRectangle(cornerRadius: 10)).overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
                }
                if draft.hasStoredAPIKey || draft.clearCredentialOnApply {
                    Button(draft.clearCredentialOnApply ? String(localized: "settings_provider_keep_key") : String(localized: "settings_provider_clear_key")) {
                        draft.clearCredentialOnApply.toggle()
                        if draft.clearCredentialOnApply {
                            draft.pendingSecret = ""
                            editingStoredCredential = false
                        } else {
                            editingStoredCredential = false
                        }
                    }
                    .font(.system(size: 11.5, weight: .medium)).foregroundColor(draft.clearCredentialOnApply ? t.text3 : t.danger)
                    .accessibilityIdentifier("provider.api-key.clear")
                }
            } else {
                FieldHint(String(localized: "settings_provider_oauth_only"))
            }
            if repository.oauthProvider(for: draft.profile.presetID) != nil { oauthControls }
        }.padding(.bottom, 14)
    }

    private var isOAuthOnly: Bool {
        draft.profile.presetID == "openai-chatgpt"
    }

    private var oauthControls: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(String(localized: "settings_provider_oauth")).font(.system(size: 12.5, weight: .medium)).foregroundColor(t.text2)
            if let oauth = draft.oauthState, oauth.signedIn {
                Text(oauth.accountLabel ?? String(localized: "settings_provider_status_configured")).font(.system(size: 11.5)).foregroundColor(t.ok)
                HStack(spacing: 12) {
                    Button(String(localized: "settings_provider_test_connection")) { testConnection() }
                    Button(String(localized: "settings_provider_logout")) { logoutOAuth() }.foregroundColor(t.danger)
                }.font(.system(size: 12, weight: .medium))
            } else {
                Button(String(localized: "settings_provider_login")) { loginOAuth() }.font(.system(size: 13, weight: .semibold)).foregroundColor(t.accent)
            }
        }.padding(.top, 4)
    }

    private var modelSection: some View {
        let preset = repository.preset(for: draft.profile.presetID)
        let runtimeModels = repository.runtimeSnapshot.models.compactMap { ref -> String? in
            guard let slash = ref.firstIndex(of: "/") else { return nil }
            guard String(ref[..<slash]) == draft.profile.id || String(ref[..<slash]) == draft.profile.presetID else { return nil }
            return String(ref[ref.index(after: slash)...])
        }
        var models = Array(NSOrderedSet(array: runtimeModels + preset.models + [draft.profile.modelID]).compactMap { $0 as? String })
        models.removeAll { $0.isEmpty }
        return VStack(alignment: .leading, spacing: 8) {
            FieldLabel(text: String(localized: "settings_provider_model_id"))
            if !models.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 7) {
                        ForEach(models, id: \.self) { model in
                            Button { draft.profile.modelID = model } label: {
                                Text(model).font(.system(size: 11.5, design: .monospaced)).foregroundColor(draft.profile.modelID == model ? .white : t.text3).padding(.horizontal, 9).padding(.vertical, 6).background(draft.profile.modelID == model ? t.accent : t.windowBg).clipShape(Capsule())
                            }.buttonStyle(.plain)
                        }
                    }
                }
            }
            SettingsField(text: textBinding(\.modelID), placeholder: preset.models.first ?? String(localized: "settings_provider_model_placeholder"))
            FieldHint(String(localized: "settings_provider_custom_model_hint"))
        }
    }

    private var statusLabel: String {
        switch draft.connectionState {
        case .testing: return String(localized: "settings_provider_status_testing")
        case .connected: return String(localized: "settings_provider_status_connected")
        case .failed: return String(localized: "settings_provider_status_failed")
        case .idle:
            if draft.hasPendingSecret { return String(localized: "settings_provider_status_pending_apply") }
            return draft.hasStoredCredential ? String(localized: "settings_provider_status_configured") : String(localized: "settings_provider_unconfigured")
        }
    }

    private var statusColor: Color {
        switch draft.connectionState {
        case .testing: return t.statusTesting
        case .connected: return t.statusConnected
        case .failed: return t.statusError
        case .idle: return draft.hasStoredCredential || draft.hasPendingSecret ? t.statusConnected : t.text4
        }
    }

    private func textBinding(_ keyPath: WritableKeyPath<ProviderStoredProfile, String>) -> Binding<String> {
        Binding(get: { draft.profile[keyPath: keyPath] }, set: { draft.profile[keyPath: keyPath] = $0 })
    }

    private func boolBinding(_ keyPath: WritableKeyPath<ProviderStoredProfile, Bool>) -> Binding<Bool> {
        Binding(get: { draft.profile[keyPath: keyPath] }, set: { draft.profile[keyPath: keyPath] = $0 })
    }

    private func testConnection() {
        guard !draft.operationInFlight else { return }
        var request = draft
        request.operationInFlight = false
        actionError = nil
        draft.operationInFlight = true
        draft.connectionState = .testing
        Task { @MainActor in
            draft = await repository.testConnection(for: request)
        }
    }

    private func loginOAuth() {
        guard !draft.operationInFlight else { return }
        let request = draft
        actionError = nil
        draft.operationInFlight = true
        Task { @MainActor in
            draft = await repository.loginOAuth(for: request)
        }
    }

    private func logoutOAuth() {
        guard !draft.operationInFlight else { return }
        let request = draft
        actionError = nil
        draft.operationInFlight = true
        Task { @MainActor in
            draft = await repository.logoutOAuth(for: request)
        }
    }

    private func save() {
        guard !saving, !removing, !draft.operationInFlight else { return }
        actionError = nil
        saving = true
        Task {
            let applied = await repository.applyDraft(draft)
            saving = false
            if applied { dismiss(); onSaved() }
            else { draft.validationMessage = repository.lastRepositoryError; draft.connectionState = .failed }
        }
    }
}

// Compatibility wrappers for deep links created by older settings snapshots.
struct ProviderPickerPage: View {
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let kind: ProviderKind
    var body: some View { ProviderListPage(store: store, host: host, kind: kind) }
}

struct ProviderEditPage: View {
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let kind: ProviderKind
    let providerId: String
    @State private var repository = ProviderRepository.shared
    var body: some View {
        if kind == .llm, let draft = repository.makeDraft(for: providerId) {
            ProviderEditorSheet(draft: draft) { host.pop() }
        } else {
            Text(String(localized: "settings_provider_edit_unavailable"))
        }
    }
}
