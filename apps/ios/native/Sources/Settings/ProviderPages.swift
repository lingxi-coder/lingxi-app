import SwiftUI

// MARK: - Models hub

/// The whole LLM workflow on one native inset-grouped page: the default model,
/// the providers (tap one to edit it), adding a provider, routing and the other
/// model services. It replaces the old group → credentials list → editor sheet
/// chain, so a provider is now two taps from the settings home.
struct ModelsSettingsPage: View {
    @Environment(\.theme) private var t
    @State private var repository = ProviderRepository.shared
    @State private var desktop = DesktopSettingsRepository.shared
    @State private var addingProvider = false
    @State private var removeError: String?
    @State private var retryMaxAttemptsText = ""
    @State private var retryBackoffMsText = ""
    @Bindable var store: SettingsStore
    let host: SettingsHost

    var body: some View {
        List {
            if showsApplyBanner {
                Section {
                    SettingsApplyChangesBanner(reconnect: host.onReconnectAfterSecretChange)
                }
                .listRowBackground(t.surface)
            }
            defaultModelSection
            providersSection
            routingSection
            fallbackSection
            servicesSection
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(t.windowBg)
        .sheet(isPresented: $addingProvider) {
            ProviderAddSheet { addingProvider = false }
                .presentationDragIndicator(.visible)
        }
        .alert(
            String(localized: "settings_provider_remove_failed"),
            isPresented: Binding(get: { removeError != nil }, set: { if !$0 { removeError = nil } })
        ) {
            Button(String(localized: "settings_done")) { removeError = nil }
        } message: {
            Text(removeError ?? "")
        }
        .task {
            retryMaxAttemptsText = String(repository.routingSettings.retryMaxAttempts)
            retryBackoffMsText = String(repository.routingSettings.retryBackoffMs)
            store.llmProviders = repository.legacyProviders()
            await repository.refreshCatalog()
            await repository.refreshCredentialStatus()
        }
        .onChange(of: repository.syncRevision) { _, _ in
            store.llmProviders = repository.legacyProviders()
        }
    }

    /// The banner renders nothing visible without a reconnect callback or a
    /// pending change; keep the empty row out of the list in that case.
    private var showsApplyBanner: Bool {
        desktop.connected
            && (host.onReconnectAfterSecretChange != nil
                || desktop.needsReconnect
                || PluginSecretRepository.shared.needsReconnect)
    }

    // MARK: Default model

    private var defaultModelSection: some View {
        let summary = repository.settingsSummary
        return Section {
            HStack(spacing: 12) {
                Circle()
                    .fill(summary.defaultProfile == nil ? t.text4 : t.accent)
                    .frame(width: 10, height: 10)
                VStack(alignment: .leading, spacing: 2) {
                    Text(summary.defaultProfile?.name ?? String(localized: "settings_provider_unconfigured"))
                        .font(.body.weight(.semibold))
                        .foregroundStyle(t.text)
                    Text(summary.defaultModelID.flatMap { $0.isEmpty ? nil : $0 } ?? "—")
                        .font(.footnote.monospaced())
                        .foregroundStyle(t.text4)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                Text(String(localized: "settings_providers_enabled_count \(summary.enabledCount)"))
                    .font(.footnote)
                    .foregroundStyle(t.text3)
            }
            .padding(.vertical, 2)
        } header: {
            Text(String(localized: "settings_provider_default_model"))
        } footer: {
            VStack(alignment: .leading, spacing: 4) {
                if let active = summary.runtime.activeModelID, !summary.runtimeMatchesDefault, !active.isEmpty {
                    Text(String(localized: "settings_provider_runtime_active \(active)"))
                }
                if let error = summary.runtime.lastError, !error.isEmpty {
                    Text(error).foregroundStyle(t.danger)
                }
            }
        }
        .listRowBackground(t.surface)
    }

    // MARK: Providers

    private var providersSection: some View {
        Section {
            if repository.profiles.isEmpty {
                Text(String(localized: "provider_no_providers_added"))
                    .font(.subheadline)
                    .foregroundStyle(t.text4)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 8)
            }
            ForEach(repository.profiles, id: \.id) { state in
                providerRow(state)
            }
            Button { addingProvider = true } label: {
                HStack(spacing: 10) {
                    Image(systemName: "plus.circle.fill")
                        .font(.title3)
                        .foregroundStyle(t.accent)
                    Text(String(localized: "settings_provider_add_row"))
                        .foregroundStyle(t.accent)
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("provider.add")
        } header: {
            Text(String(localized: "settings_provider_section_added_count \(repository.settingsSummary.totalCount)"))
        } footer: {
            Text(String(localized: "provider_llm_blurb"))
        }
        .listRowBackground(t.surface)
    }

    private func providerRow(_ state: ProviderProfileState) -> some View {
        let preset = repository.preset(for: state.profile.presetID)
        return SettingsRow(
            labelView: AnyView(HStack(spacing: 6) {
                Text(state.profile.name).font(.subheadline.weight(.medium)).foregroundColor(t.text)
                if state.profile.isDefault { badge(String(localized: "settings_badge_default"), t.accent) }
                if !state.profile.enabled { badge(String(localized: "settings_badge_disabled"), t.text4) }
                if state.oauthState?.signedIn == true { badge(String(localized: "settings_provider_status_oauth"), t.ok) }
            }),
            subView: AnyView(HStack(spacing: 5) {
                Circle().fill(state.legacyStatus.dot(t)).frame(width: 6, height: 6)
                Text("\(state.statusLabel) · \(state.profile.modelID.isEmpty ? "—" : state.profile.modelID)")
                    .font(.system(.caption, design: .monospaced)).foregroundColor(t.text4).lineLimit(1)
            }),
            chevron: true,
            isLast: true,
            onTap: { host.push(.providerEdit(.init(.llm), state.id)) }
        ) {
            ProviderTile(preset: preset, name: state.profile.name)
        }
        .listRowInsets(EdgeInsets())
        .accessibilityIdentifier("provider.row.\(state.id)")
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            Button(role: .destructive) {
                Task {
                    if await !repository.removeProfile(state.id) {
                        removeError = repository.lastRepositoryError
                            ?? String(localized: "settings_provider_remove_failed")
                    }
                }
            } label: {
                Text(String(localized: "common_removed"))
            }
        }
    }

    private func badge(_ text: String, _ color: Color) -> some View {
        Text(text).font(.caption.weight(.medium)).foregroundColor(color)
            .padding(.horizontal, 6).padding(.vertical, 1)
            .background(color.tint(0.18)).clipShape(RoundedRectangle(cornerRadius: 4))
    }

    // MARK: Routing

    private var routingSection: some View {
        Section {
            Toggle(isOn: Binding(
                get: { repository.visionDelegationEnabled },
                set: { repository.setVisionDelegationEnabled($0) }
            )) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(String(localized: "settings_provider_vision_delegation"))
                        .foregroundStyle(t.text)
                    Text(String(localized: "settings_provider_vision_delegation_hint"))
                        .font(.footnote)
                        .foregroundStyle(t.text4)
                }
            }
            .tint(t.accent)
            numberRow(String(localized: "settings_provider_max_retries"), text: $retryMaxAttemptsText)
            numberRow(String(localized: "settings_provider_backoff_ms"), text: $retryBackoffMsText)
            if hasPendingRoutingChanges {
                Button {
                    Task {
                        let applied = await repository.applyRoutingChanges(
                            retryMaxAttemptsText: retryMaxAttemptsText,
                            retryBackoffMsText: retryBackoffMsText
                        )
                        if applied {
                            retryMaxAttemptsText = String(repository.routingSettings.retryMaxAttempts)
                            retryBackoffMsText = String(repository.routingSettings.retryBackoffMs)
                        }
                    }
                } label: {
                    Text(String(localized: "settings_provider_apply_routing"))
                        .fontWeight(.semibold)
                        .foregroundStyle(t.accent)
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.plain)
            }
        } header: {
            Text(String(localized: "settings_provider_section_routing"))
        } footer: {
            if let message = repository.routingMessage { Text(message) }
        }
        .listRowBackground(t.surface)
    }

    private var fallbackSection: some View {
        let candidates = repository.fallbackCandidates()
        return Section {
            if candidates.isEmpty {
                Text(String(localized: "settings_provider_fallback_hint"))
                    .font(.footnote)
                    .foregroundStyle(t.text4)
            } else {
                ForEach(candidates) { candidate in
                    Button { repository.toggleFallbackProfile(candidate.profileID) } label: {
                        HStack(spacing: 10) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(candidate.name).foregroundStyle(t.text)
                                Text(candidate.modelID)
                                    .font(.footnote.monospaced())
                                    .foregroundStyle(t.text4)
                                    .lineLimit(1)
                            }
                            Spacer(minLength: 8)
                            if candidate.selected {
                                Image(systemName: "checkmark")
                                    .fontWeight(.semibold)
                                    .foregroundStyle(t.accent)
                            }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        } header: {
            Text(String(localized: "settings_provider_fallback_order"))
        }
        .listRowBackground(t.surface)
    }

    private var hasPendingRoutingChanges: Bool {
        repository.routingDirty
            || retryMaxAttemptsText.trimmingCharacters(in: .whitespacesAndNewlines) != String(repository.routingSettings.retryMaxAttempts)
            || retryBackoffMsText.trimmingCharacters(in: .whitespacesAndNewlines) != String(repository.routingSettings.retryBackoffMs)
    }

    private func numberRow(_ label: String, text: Binding<String>) -> some View {
        LabeledContent {
            TextField("", text: text)
                .keyboardType(.numberPad)
                .multilineTextAlignment(.trailing)
                .foregroundStyle(t.text3)
        } label: {
            Text(label).foregroundStyle(t.text)
        }
    }

    // MARK: Other model services

    private var servicesSection: some View {
        let entries = DesktopSettingsEntry.all.filter {
            $0.availableOnMobile && $0.mobileGroup == .models && $0.id != "provider-credentials"
        }
        return Section {
            ForEach(entries) { entry in
                SettingsRow(icon: entry.icon, label: entry.title, isLast: true, onTap: { host.push(entry.page) })
                    .listRowInsets(EdgeInsets())
                    .accessibilityIdentifier(entry.accessibilityID)
            }
            NavigationLink {
                EngineCredentialsPage()
            } label: {
                SettingsRow(icon: .pin, label: String(localized: "settings_parity_credentials"), chevron: false, isLast: true)
            }
            .listRowInsets(EdgeInsets())
        }
        .listRowBackground(t.surface)
    }
}

/// The colored initial used for a provider throughout the model settings.
private struct ProviderTile: View {
    @Environment(\.theme) private var t
    let preset: ProviderPreset
    let name: String

    var body: some View {
        Text(String(name.prefix(1)))
            .font(.footnote.weight(.bold))
            .foregroundStyle(preset.color)
            .frame(width: 28, height: 28)
            .background(preset.color.mix(with: t.surface, amount: 0.18))
            .clipShape(.rect(cornerRadius: 7))
            .overlay {
                RoundedRectangle(cornerRadius: 7)
                    .stroke(preset.color.tint(0.28), lineWidth: 0.5)
            }
    }
}

/// Engine-profile credentials are an advanced, desktop-parity editor; it lives
/// one level below the hub instead of at the bottom of the provider list.
private struct EngineCredentialsPage: View {
    @Environment(\.theme) private var t

    var body: some View {
        ScrollView {
            EngineProfileCredentialsEditor().padding(16)
        }
        .background(t.windowBg)
        .navigationTitle(String(localized: "settings_parity_credentials"))
        .navigationBarTitleDisplayMode(.inline)
    }
}

// MARK: - Non-LLM provider kinds

/// Search and fetch providers are not wired to a real store on iOS yet; they
/// keep the explanatory empty state. The LLM kind is the models hub.
struct ProviderListPage: View {
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let kind: ProviderKind

    var body: some View {
        if kind == .llm {
            ModelsSettingsPage(store: store, host: host)
        } else {
            unsupportedProviderNotice
        }
    }

    private var unsupportedProviderNotice: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(String(localized: "settings_provider_not_connected")).font(.footnote.weight(.semibold)).foregroundColor(t.text)
            Text(String(localized: "settings_provider_unsupported_notice \(kind.title)")) .font(.caption).foregroundColor(t.text4).lineSpacing(4)
        }
        .frame(maxWidth: .infinity, alignment: .leading).padding(14).background(t.surface).clipShape(RoundedRectangle(cornerRadius: 12)).overlay(RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.5))
    }
}

// MARK: - Add flow

/// "Add provider": pick a preset, then fill in the editor. Both steps live in
/// one modal so the settings stack underneath does not grow.
struct ProviderAddSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.theme) private var t
    @State private var repository = ProviderRepository.shared
    @State private var path: [String] = []
    let onDone: () -> Void

    var body: some View {
        NavigationStack(path: $path) {
            List {
                Section {
                    ForEach(repository.catalogPresets) { preset in
                        Button { path.append(preset.id) } label: {
                            HStack(spacing: 12) {
                                ProviderTile(preset: preset, name: preset.name)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(preset.name).foregroundStyle(t.text)
                                    Text(preset.sub).font(.footnote).foregroundStyle(t.text4)
                                }
                                Spacer(minLength: 8)
                                Image(systemName: "chevron.right")
                                    .font(.footnote.weight(.semibold))
                                    .foregroundStyle(t.text4.opacity(0.7))
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("provider.preset.\(preset.id)")
                    }
                } footer: {
                    Text(String(localized: "settings_provider_picker_intro"))
                }
                .listRowBackground(t.surface)
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(t.windowBg)
            .navigationTitle(String(localized: "settings_title_add_llm"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(String(localized: "common_cancel")) { dismiss() }
                        .accessibilityIdentifier("provider.cancel")
                }
            }
            .navigationDestination(for: String.self) { presetID in
                ProviderEditorForm(draft: repository.makeNewDraft(presetID: presetID)) {
                    dismiss()
                    onDone()
                }
            }
        }
        .task {
            await repository.refreshCatalog()
        }
    }
}

// MARK: - Editor

/// A modal editor for one draft (onboarding, or any place without a settings
/// stack of its own). Inside the settings stack the editor is pushed directly
/// as `ProviderEditorForm`.
struct ProviderEditorSheet: View {
    @Environment(\.dismiss) private var dismiss
    let draft: ProviderEditorDraft
    let onSaved: () -> Void

    init(draft: ProviderEditorDraft, onSaved: @escaping () -> Void) {
        self.draft = draft
        self.onSaved = onSaved
    }

    var body: some View {
        NavigationStack {
            ProviderEditorForm(draft: draft) {
                dismiss()
                onSaved()
            }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(String(localized: "common_cancel")) { dismiss() }
                        .accessibilityIdentifier("provider.cancel")
                }
            }
        }
    }
}

/// The provider editor as a native grouped form. It owns the draft; whoever
/// presents it decides what "done" means through `onSaved`, which is called
/// after a successful save or removal.
struct ProviderEditorForm: View {
    @Environment(\.theme) private var t
    @State private var repository = ProviderRepository.shared
    @State private var draft: ProviderEditorDraft
    @State private var title: String
    @State private var showKey = false
    @State private var editingStoredCredential = false
    @State private var saving = false
    @State private var removing = false
    @State private var confirmingRemove = false
    @State private var actionError: String?
    let onSaved: () -> Void

    init(draft: ProviderEditorDraft, onSaved: @escaping () -> Void) {
        _draft = State(initialValue: draft)
        _title = State(initialValue: draft.profile.name)
        self.onSaved = onSaved
    }

    private var busy: Bool {
        saving || removing || draft.operationInFlight || draft.connectionState == .testing
    }

    var body: some View {
        Form {
            statusSection
            identitySection
            apiKeySection
            oauthSection
            modelSection
            stateSection
            connectionsSection
            removeSection
        }
        .scrollContentBackground(.hidden)
        .background(t.windowBg)
        .disabled(busy)
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .confirmationAction) {
                if saving {
                    ProgressView()
                } else {
                    Button(String(localized: "settings_save")) { save() }
                        .fontWeight(.semibold)
                        .disabled(busy)
                        .accessibilityIdentifier("provider.save")
                }
            }
        }
        .confirmationDialog(
            String(localized: "settings_provider_remove"),
            isPresented: $confirmingRemove,
            titleVisibility: .visible
        ) {
            Button(String(localized: "common_removed"), role: .destructive) { remove() }
            Button(String(localized: "common_cancel"), role: .cancel) {}
        }
    }

    // MARK: Status

    private var statusSection: some View {
        Section {
            HStack(spacing: 10) {
                Circle().fill(statusColor).frame(width: 8, height: 8)
                Text(statusLabel).foregroundStyle(t.text)
                Spacer(minLength: 8)
                if draft.connectionState == .testing {
                    ProgressView()
                } else {
                    Button(String(localized: "settings_provider_test_connection")) { testConnection() }
                        .buttonStyle(.plain)
                        .foregroundStyle(t.accent)
                        .disabled(draft.operationInFlight)
                }
            }
        } footer: {
            if let detail = draft.validationMessage ?? draft.detailMessage ?? actionError {
                Text(detail)
                    .foregroundStyle(draft.connectionState == .failed || draft.validationMessage != nil || actionError != nil ? t.danger : t.text3)
            }
        }
        .listRowBackground(t.surface)
    }

    // MARK: Name and endpoint

    private var identitySection: some View {
        Section {
            LabeledContent {
                TextField("", text: textBinding(\.name))
                    .multilineTextAlignment(.trailing)
                    .foregroundStyle(t.text3)
            } label: {
                Text(String(localized: "settings_display_name")).foregroundStyle(t.text)
            }
            if repository.isOfficialEndpoint(for: draft.profile) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(String(localized: "settings_api_url")).font(.footnote).foregroundStyle(t.text3)
                    Text(draft.profile.baseURL)
                        .font(.callout.monospaced())
                        .foregroundStyle(t.text4)
                        .textSelection(.enabled)
                }
            } else {
                fieldRow(
                    String(localized: "settings_api_url"),
                    text: textBinding(\.baseURL),
                    placeholder: repository.preset(for: draft.profile.presetID).defaultUrl
                )
            }
        } footer: {
            if repository.isOfficialEndpoint(for: draft.profile) {
                Text(String(localized: "settings_provider_official_url_hint \(draft.profile.baseURL)"))
            } else {
                Text(String(localized: "settings_provider_custom_url_hint"))
            }
        }
        .listRowBackground(t.surface)
    }

    /// A caption over a full-width field: URLs and ids are too long for a
    /// trailing value.
    private func fieldRow(_ label: String, text: Binding<String>, placeholder: String = "") -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).font(.footnote).foregroundStyle(t.text3)
            TextField(placeholder, text: text)
                .font(.callout.monospaced())
                .foregroundStyle(t.text)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
        }
    }

    // MARK: Credentials

    private var isOAuthOnly: Bool {
        draft.profile.presetID == "openai-chatgpt"
    }

    /// Render the OAuth block when this preset can sign in, OR when a session
    /// already exists for a preset whose login is withheld — otherwise a
    /// previously-signed-in account would keep a credential the UI can no
    /// longer show or revoke.
    private var showsOAuth: Bool {
        repository.oauthProvider(for: draft.profile.presetID) != nil
            && (repository.oauthLoginAvailable(for: draft.profile.presetID) || draft.oauthState?.signedIn == true)
    }

    private var storedKeyCollapsed: Bool {
        draft.hasStoredAPIKey && !editingStoredCredential && draft.pendingSecret.isEmpty && !draft.clearCredentialOnApply
    }

    @ViewBuilder private var apiKeySection: some View {
        if isOAuthOnly {
            if !showsOAuth {
                Section {
                    Text(String(localized: "settings_provider_oauth_only"))
                        .font(.footnote)
                        .foregroundStyle(t.text3)
                }
                .listRowBackground(t.surface)
            }
        } else {
            Section {
                if storedKeyCollapsed {
                    LabeledContent {
                        Text("••••••••••••").font(.callout.monospaced()).foregroundStyle(t.text3)
                    } label: {
                        Text(String(localized: "settings_provider_api_key")).foregroundStyle(t.text)
                    }
                    Button(String(localized: "settings_provider_replace_key")) {
                        editingStoredCredential = true
                        draft.clearCredentialOnApply = false
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(t.accent)
                } else {
                    HStack(spacing: 8) {
                        Group {
                            if showKey {
                                TextField(String(localized: "settings_provider_api_key_placeholder"), text: $draft.pendingSecret)
                            } else {
                                SecureField(String(localized: "settings_provider_api_key_placeholder"), text: $draft.pendingSecret)
                            }
                        }
                        .font(.callout.monospaced())
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("provider.api-key")
                        Button(showKey ? String(localized: "settings_provider_hide_key") : String(localized: "settings_provider_show_key")) {
                            showKey.toggle()
                        }
                        .buttonStyle(.plain)
                        .font(.subheadline)
                        .foregroundStyle(t.accent)
                        .accessibilityIdentifier("provider.api-key.visibility")
                    }
                }
                if draft.hasStoredAPIKey || draft.clearCredentialOnApply {
                    Button(role: draft.clearCredentialOnApply ? nil : .destructive) {
                        draft.clearCredentialOnApply.toggle()
                        if draft.clearCredentialOnApply { draft.pendingSecret = "" }
                        editingStoredCredential = false
                    } label: {
                        Text(draft.clearCredentialOnApply
                            ? String(localized: "settings_provider_keep_key")
                            : String(localized: "settings_provider_clear_key"))
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(draft.clearCredentialOnApply ? t.accent : t.danger)
                    .accessibilityIdentifier("provider.api-key.clear")
                }
            } header: {
                Text(String(localized: "settings_provider_api_key"))
            }
            .listRowBackground(t.surface)
        }
    }

    @ViewBuilder private var oauthSection: some View {
        if showsOAuth {
            Section {
                if let oauth = draft.oauthState, oauth.signedIn {
                    LabeledContent {
                        Text(oauth.accountLabel ?? String(localized: "settings_provider_status_configured"))
                            .foregroundStyle(t.ok)
                    } label: {
                        Text(String(localized: "settings_provider_oauth")).foregroundStyle(t.text)
                    }
                    Button(String(localized: "settings_provider_test_connection")) { testConnection() }
                        .buttonStyle(.plain)
                        .foregroundStyle(t.accent)
                    Button(String(localized: "settings_provider_logout"), role: .destructive) { logoutOAuth() }
                        .buttonStyle(.plain)
                        .foregroundStyle(t.danger)
                } else if repository.oauthLoginAvailable(for: draft.profile.presetID) {
                    Button(String(localized: "settings_provider_login")) { loginOAuth() }
                        .buttonStyle(.plain)
                        .fontWeight(.semibold)
                        .foregroundStyle(t.accent)
                        .accessibilityIdentifier("provider.oauth.login")
                }
            } header: {
                Text(String(localized: "settings_provider_oauth"))
            } footer: {
                if isOAuthOnly { Text(String(localized: "settings_provider_oauth_only")) }
            }
            .listRowBackground(t.surface)
        }
    }

    // MARK: Model

    private var modelSection: some View {
        Section {
            NavigationLink {
                ProviderModelsPage(draft: $draft)
            } label: {
                LabeledContent {
                    Text(draft.profile.modelID.isEmpty ? "—" : draft.profile.modelID)
                        .font(.callout.monospaced())
                        .foregroundStyle(t.text3)
                        .lineLimit(1)
                } label: {
                    Text(String(localized: "settings_provider_model_id")).foregroundStyle(t.text)
                }
            }
            .accessibilityIdentifier("provider.model")
        }
        .listRowBackground(t.surface)
    }

    // MARK: Enable / default

    private var stateSection: some View {
        Section {
            Toggle(String(localized: "settings_provider_enable"), isOn: boolBinding(\.enabled))
                .tint(t.accent)
            Button {
                draft.profile.isDefault = true
                draft.profile.enabled = true
            } label: {
                HStack {
                    Text(String(localized: "settings_provider_set_default")).foregroundStyle(t.text)
                    Spacer(minLength: 8)
                    if draft.profile.isDefault {
                        Image(systemName: "checkmark").fontWeight(.semibold).foregroundStyle(t.accent)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
        .listRowBackground(t.surface)
    }

    // MARK: Extra connections

    @ViewBuilder private var connectionsSection: some View {
        Section {
            Button(String(localized: "settings_provider_connection_add")) {
                draft.profile.addConnection()
            }
            .buttonStyle(.plain)
            .foregroundStyle(t.accent)
        } header: {
            Text(String(localized: "settings_provider_connections"))
        } footer: {
            Text(draft.profile.connections.isEmpty
                ? String(localized: "settings_provider_connections_single")
                : String(localized: "settings_provider_connections_hint"))
        }
        .listRowBackground(t.surface)
        // Identified by position: ids start empty and stay editable, so an
        // id-keyed ForEach would collapse every new row onto one.
        ForEach(draft.profile.connections.indices, id: \.self) { index in
            connectionSection(index: index)
        }
    }

    private func connectionSection(index: Int) -> some View {
        Section {
            fieldRow(
                String(localized: "settings_provider_connection_id"),
                text: connectionBinding(index: index, keyPath: \.id),
                placeholder: "cn"
            )
            fieldRow(
                String(localized: "settings_api_url"),
                text: connectionBinding(index: index, keyPath: \.baseURL),
                placeholder: repository.preset(for: draft.profile.presetID).defaultUrl
            )
            fieldRow(
                String(localized: "settings_provider_connection_models"),
                text: connectionModelsBinding(index: index),
                placeholder: draft.profile.modelID
            )
            Button(String(localized: "common_removed"), role: .destructive) {
                draft.profile.removeConnection(at: index)
            }
            .buttonStyle(.plain)
            .foregroundStyle(t.danger)
        } footer: {
            Text(String(localized: "settings_provider_connection_id_hint"))
        }
        .listRowBackground(t.surface)
    }

    /// Removing a row shortens the array while SwiftUI still holds bindings for
    /// the rows that were after it, so every accessor is bounds-checked rather
    /// than subscripting straight into `connections`.
    private func connectionBinding(
        index: Int,
        keyPath: WritableKeyPath<ProviderStoredConnection, String>
    ) -> Binding<String> {
        Binding(
            get: { draft.profile.connections.indices.contains(index) ? draft.profile.connections[index][keyPath: keyPath] : "" },
            set: { value in
                guard draft.profile.connections.indices.contains(index) else { return }
                draft.profile.connections[index][keyPath: keyPath] = value
            }
        )
    }

    /// A comma-separated list, because a connection that serves every model
    /// leaves this empty and one that serves a few names only those.
    private func connectionModelsBinding(index: Int) -> Binding<String> {
        Binding(
            get: {
                guard draft.profile.connections.indices.contains(index) else { return "" }
                return (draft.profile.connections[index].modelIDs ?? []).joined(separator: ", ")
            },
            set: { value in
                guard draft.profile.connections.indices.contains(index) else { return }
                let ids = value
                    .split(separator: ",")
                    .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                    .filter { !$0.isEmpty }
                draft.profile.connections[index].modelIDs = ids.isEmpty ? nil : ids
            }
        )
    }

    // MARK: Remove

    @ViewBuilder private var removeSection: some View {
        if !draft.isNew {
            Section {
                Button(role: .destructive) { confirmingRemove = true } label: {
                    Text(String(localized: "settings_provider_remove"))
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.plain)
                .foregroundStyle(t.danger)
            }
            .listRowBackground(t.surface)
        }
    }

    // MARK: State helpers

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

    // MARK: Actions

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
        guard !busy else { return }
        actionError = nil
        saving = true
        Task {
            let applied = await repository.applyDraft(draft)
            saving = false
            if applied { onSaved() }
            else { draft.validationMessage = repository.lastRepositoryError; draft.connectionState = .failed }
        }
    }

    private func remove() {
        guard !busy else { return }
        removing = true
        Task {
            let removed = await repository.removeProfile(draft.id)
            removing = false
            if removed {
                onSaved()
            } else {
                let message = repository.lastRepositoryError
                    ?? String(localized: "settings_provider_remove_failed")
                actionError = message
                draft.connectionState = .failed
                draft.detailMessage = message
            }
        }
    }
}

// MARK: - Model selection

/// Which model a provider uses by default and which of its models appear in
/// the chat model picker. It is a drill-in of the editor, like iOS pickers.
private struct ProviderModelsPage: View {
    @Environment(\.theme) private var t
    @State private var repository = ProviderRepository.shared
    @State private var selectedModelDetails: ModelRuntimeDetails?
    @Binding var draft: ProviderEditorDraft

    private struct Catalog {
        var models: [String] = []
        var details: [String: ModelRuntimeDetails] = [:]
        var placeholder: String
        var summary: String
        var isReady = false
        var isLoading = false
    }

    private var catalog: Catalog {
        let fallbackPlaceholder = draft.profile.modelID.isEmpty
            ? String(localized: "settings_provider_model_placeholder")
            : draft.profile.modelID
        switch repository.editorModelCatalog(
            for: draft.profile,
            runtimeModelReferences: repository.runtimeSnapshot.models
        ) {
        case .ready(let models, let details):
            return Catalog(
                models: models,
                details: details,
                placeholder: models.first ?? String(localized: "settings_provider_model_placeholder"),
                summary: "\(repository.visibleModelCount(for: draft.profile, from: models)) / \(models.count)",
                isReady: true
            )
        case .loading:
            return Catalog(
                placeholder: fallbackPlaceholder,
                summary: String(localized: "settings_provider_model_catalog_loading"),
                isLoading: true
            )
        case .unavailable:
            return Catalog(
                placeholder: fallbackPlaceholder,
                summary: String(localized: "settings_provider_model_catalog_unavailable")
            )
        }
    }

    var body: some View {
        let catalog = catalog
        let visible = repository.visibleModelIDs(for: draft.profile, from: catalog.models)
        List {
            if catalog.isReady {
                modelListSection(catalog)
            } else {
                Section {
                    HStack(spacing: 10) {
                        if catalog.isLoading { ProgressView() }
                        Text(catalog.summary).font(.footnote).foregroundStyle(t.text3)
                    }
                }
                .listRowBackground(t.surface)
            }
            customModelSection(catalog)
            pickerSection(catalog, visible: visible)
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(t.windowBg)
        .navigationTitle(String(localized: "settings_provider_model_id"))
        .navigationBarTitleDisplayMode(.inline)
        .sheet(item: $selectedModelDetails) { details in
            ModelDetailsSheet(details: details, accent: Accents.color(for: draft.profile.presetID))
        }
    }

    private func modelListSection(_ catalog: Catalog) -> some View {
        Section {
            ForEach(catalog.models, id: \.self) { model in
                let details = catalog.details[model]
                HStack(spacing: 8) {
                    Button { draft.profile.modelID = model } label: {
                        HStack(spacing: 8) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(details?.preferredDisplayName ?? ModelDisplay.modelName(for: model))
                                    .foregroundStyle(t.text)
                                Text(model)
                                    .font(.footnote.monospaced())
                                    .foregroundStyle(t.text4)
                                    .lineLimit(1)
                                if let details, !details.summaryItems.isEmpty {
                                    Text(details.summaryItems.joined(separator: " · "))
                                        .font(.caption)
                                        .foregroundStyle(t.text4)
                                        .lineLimit(2)
                                }
                            }
                            Spacer(minLength: 8)
                            if draft.profile.modelID == model {
                                Image(systemName: "checkmark").fontWeight(.semibold).foregroundStyle(t.accent)
                            }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    if let details {
                        Button { selectedModelDetails = details } label: {
                            Image(systemName: "info.circle")
                                .foregroundStyle(t.accent)
                                .frame(width: 32, height: 32)
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("provider.model.info.\(model)")
                    }
                }
            }
        }
        .listRowBackground(t.surface)
    }

    private func customModelSection(_ catalog: Catalog) -> some View {
        Section {
            TextField(catalog.placeholder, text: Binding(
                get: { draft.profile.modelID },
                set: { draft.profile.modelID = $0 }
            ))
            .font(.callout.monospaced())
            .foregroundStyle(t.text)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
        } header: {
            Text(String(localized: "settings_provider_custom_model_id"))
        } footer: {
            Text(String(localized: "settings_provider_custom_model_hint"))
        }
        .listRowBackground(t.surface)
    }

    @ViewBuilder
    private func pickerSection(_ catalog: Catalog, visible: [String]) -> some View {
        Section {
            Toggle(isOn: Binding(
                get: { draft.profile.showInModelPicker },
                set: { draft.profile.showInModelPicker = $0 }
            )) {
                Text(String(localized: "settings_provider_show_in_picker")).foregroundStyle(t.text)
            }
            .tint(t.accent)
            if catalog.isReady, draft.profile.showInModelPicker {
                ForEach(catalog.models, id: \.self) { model in
                    Toggle(isOn: Binding(
                        get: { visible.contains(model) },
                        set: { _ in toggleVisibleModel(model, from: catalog.models) }
                    )) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(catalog.details[model]?.preferredDisplayName ?? ModelDisplay.modelName(for: model))
                                .foregroundStyle(t.text)
                            Text(model)
                                .font(.footnote.monospaced())
                                .foregroundStyle(t.text4)
                                .lineLimit(1)
                        }
                    }
                    .tint(t.accent)
                }
            }
        } footer: {
            if catalog.isReady, draft.profile.showInModelPicker, visible.isEmpty {
                Text(String(localized: "settings_provider_picker_hidden_empty"))
                    .foregroundStyle(t.statusTesting)
            } else {
                Text(catalog.summary)
            }
        }
        .listRowBackground(t.surface)
    }

    private func toggleVisibleModel(_ modelID: String, from candidates: [String]) {
        var allowlist = draft.profile.visibleModelIDs ?? candidates
        if allowlist.contains(modelID) {
            allowlist.removeAll { $0 == modelID }
        } else {
            allowlist.append(modelID)
        }
        draft.profile.visibleModelIDs = Array(NSOrderedSet(array: allowlist)).compactMap { $0 as? String }
    }
}

struct ProviderEditPage: View {
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let kind: ProviderKind
    let providerId: String
    @State private var repository = ProviderRepository.shared
    var body: some View {
        if kind == .llm, let draft = repository.makeDraft(for: providerId) {
            ProviderEditorForm(draft: draft) { host.pop() }
        } else {
            Text(String(localized: "settings_provider_edit_unavailable"))
        }
    }
}
