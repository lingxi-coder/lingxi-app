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
                .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
            }
            defaultModelSection
            providersSection
            Section {
                NavigationLink {
                    Form {
                        routingSection
                        fallbackSection
                        servicesSection
                    }
                    .navigationTitle(String(localized: "settings_section_advanced"))
                    .navigationBarTitleDisplayMode(.inline)
                } label: {
                    Text("settings_section_advanced")
                }
                .accessibilityIdentifier("provider.advanced")
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color(uiColor: .systemGroupedBackground))
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
            && (desktop.needsReconnect || PluginSecretRepository.shared.needsReconnect)
    }

    // MARK: Default model

    private var defaultModelSection: some View {
        let summary = repository.settingsSummary
        return Section {
            LabeledContent {
                Text(summary.defaultModelID.flatMap { $0.isEmpty ? nil : $0 }
                     ?? String(localized: "settings_provider_unconfigured"))
                    .foregroundStyle(Color.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            } label: {
                Text("settings_provider_default_model")
            }
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
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
    }

    // MARK: Providers

    private var providersSection: some View {
        Section {
            if repository.profiles.isEmpty {
                Text(String(localized: "provider_no_providers_added"))
                    .font(.subheadline)
                    .foregroundStyle(Color.secondary)
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
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("provider.add")
        } header: {
            Text(String(localized: "settings_provider_section_added_count \(repository.settingsSummary.totalCount)"))
        }
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
    }

    private func providerRow(_ state: ProviderProfileState) -> some View {
        let preset = repository.preset(for: state.profile.presetID)
        return Button { host.push(.providerEdit(.init(.llm), state.id)) } label: {
            HStack(spacing: 12) {
                ProviderTile(preset: preset, name: state.profile.name)
                VStack(alignment: .leading, spacing: 4) {
                    Text(state.profile.name).font(.body).foregroundStyle(Color.primary)
                        .fixedSize(horizontal: false, vertical: true)
                    Text(state.profile.modelID.isEmpty ? "—" : state.profile.modelID)
                        .font(.subheadline).foregroundStyle(Color.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Image(systemName: state.oauthState?.signedIn == true ? "person.crop.circle" : "circle.fill")
                    .font(.caption2).foregroundStyle(state.legacyStatus.dot(t))
                    .accessibilityLabel(state.profile.enabled ? state.statusLabel : String(localized: "settings_badge_disabled"))
                if state.profile.isDefault {
                    Image(systemName: "checkmark.circle.fill").foregroundStyle(t.accent)
                        .accessibilityLabel(String(localized: "settings_badge_default"))
                }
                Image(systemName: "chevron.right")
                    .font(.footnote.weight(.semibold)).foregroundStyle(Color(uiColor: .tertiaryLabel))
                    .accessibilityHidden(true)
            }
            .padding(.vertical, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.automatic)
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
                Label(String(localized: "common_removed"), systemImage: "trash")
            }
        }
    }

    // MARK: Routing

    private var routingSection: some View {
        Section {
            Toggle(isOn: Binding(
                get: { repository.visionDelegationEnabled },
                set: { repository.setVisionDelegationEnabled($0) }
            )) {
                Text("settings_provider_vision_delegation")
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
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
    }

    private var fallbackSection: some View {
        let candidates = repository.fallbackCandidates()
        return Section {
            if candidates.isEmpty {
                Text("provider_no_providers_added").foregroundStyle(Color.secondary)
            } else {
                ForEach(candidates) { candidate in
                    Button { repository.toggleFallbackProfile(candidate.profileID) } label: {
                        HStack(spacing: 10) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(candidate.name).foregroundStyle(Color.primary)
                                Text(candidate.modelID)
                                    .font(.footnote.monospaced())
                                    .foregroundStyle(Color.secondary)
                                    .fixedSize(horizontal: false, vertical: true)
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
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
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
                .foregroundStyle(Color.secondary)
        } label: {
            Text(label).foregroundStyle(Color.primary)
        }
    }

    // MARK: Other model services

    private var servicesSection: some View {
        let entries = DesktopSettingsEntry.all.filter {
            $0.availableOnMobile && $0.mobileGroup == .models && $0.id != "provider-credentials"
        }
        return Section {
            ForEach(entries) { entry in
                Button { host.push(entry.page) } label: {
                    HStack {
                        Text(entry.title).foregroundStyle(Color.primary)
                        Spacer()
                        Image(systemName: "chevron.right")
                            .font(.footnote.weight(.semibold)).foregroundStyle(Color(uiColor: .tertiaryLabel))
                            .accessibilityHidden(true)
                    }
                    .frame(minHeight: 28)
                    .contentShape(Rectangle())
                }
                .accessibilityIdentifier(entry.accessibilityID)
            }
            NavigationLink {
                EngineCredentialsPage()
            } label: {
                Text("settings_parity_credentials")
            }
        }
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
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
            .frame(width: 36, height: 36)
            .background(preset.color.opacity(0.12))
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
        .background(Color(uiColor: .systemGroupedBackground))
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
            Text(String(localized: "settings_provider_not_connected")).font(.footnote.weight(.semibold)).foregroundStyle(Color.primary)
            Text(String(localized: "settings_provider_unsupported_notice \(kind.title)")) .font(.caption).foregroundStyle(Color.secondary).lineSpacing(4)
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
    @State private var query = ""

    private var presets: [ProviderPreset] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        return query.isEmpty ? repository.catalogPresets : repository.catalogPresets.filter {
            $0.name.localizedStandardContains(query) || $0.sub.localizedStandardContains(query)
        }
    }

    let onDone: () -> Void

    var body: some View {
        NavigationStack(path: $path) {
            List {
                Section {
                    ForEach(presets) { preset in
                        Button { path.append(preset.id) } label: {
                            HStack(spacing: 12) {
                                ProviderTile(preset: preset, name: preset.name)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(preset.name).foregroundStyle(Color.primary)
                                }
                                Spacer(minLength: 8)
                                Image(systemName: "chevron.right")
                                    .font(.footnote.weight(.semibold))
                                    .foregroundStyle(Color(uiColor: .tertiaryLabel))
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("provider.preset.\(preset.id)")
                    }
                }
                .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(Color(uiColor: .systemGroupedBackground))
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: Text("settings_parity_search"))
            .overlay {
                if presets.isEmpty { ContentUnavailableView.search(text: query) }
            }
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
            identitySection
            apiKeySection
            oauthSection
            statusSection
            modelSection
            Section {
                NavigationLink {
                    Form {
                        stateSection
                        connectionsSection
                    }
                    .navigationTitle(String(localized: "settings_section_advanced"))
                    .navigationBarTitleDisplayMode(.inline)
                } label: {
                    Text("settings_section_advanced")
                }
                .accessibilityIdentifier("provider.connections")
            }
            removeSection
        }
        .formStyle(.grouped)
        .scrollDismissesKeyboard(.interactively)
        .background(Color(uiColor: .systemGroupedBackground))
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
            Button(action: testConnection) {
                HStack {
                    Text("settings_provider_test_connection")
                    Spacer(minLength: 8)
                    if draft.connectionState == .testing {
                        ProgressView()
                    } else {
                        Text(statusLabel).font(.subheadline).foregroundStyle(statusColor)
                    }
                }
            }
            .accessibilityIdentifier("provider.test")
            .disabled(draft.operationInFlight)
        } footer: {
            if let detail = draft.validationMessage ?? draft.detailMessage ?? actionError {
                Text(detail)
                    .foregroundStyle(draft.connectionState == .failed || draft.validationMessage != nil || actionError != nil ? t.danger : t.text3)
            }
        }
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
    }

    // MARK: Name and endpoint

    private var identitySection: some View {
        Section {
            LabeledContent {
                TextField("", text: textBinding(\.name))
                    .multilineTextAlignment(.trailing)
                    .foregroundStyle(Color.secondary)
            } label: {
                Text(String(localized: "settings_display_name")).foregroundStyle(Color.primary)
            }
            if repository.isOfficialEndpoint(for: draft.profile) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(String(localized: "settings_api_url")).font(.footnote).foregroundStyle(Color.secondary)
                    Text(draft.profile.baseURL)
                        .font(.callout.monospaced())
                        .foregroundStyle(Color.secondary)
                        .textSelection(.enabled)
                }
            } else {
                fieldRow(
                    String(localized: "settings_api_url"),
                    text: textBinding(\.baseURL),
                    placeholder: repository.preset(for: draft.profile.presetID).defaultUrl
                )
                .keyboardType(.URL)
            }
        }
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
    }

    /// A caption over a full-width field: URLs and ids are too long for a
    /// trailing value.
    private func fieldRow(_ label: String, text: Binding<String>, placeholder: String = "") -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).font(.footnote).foregroundStyle(Color.secondary)
            TextField(placeholder, text: text)
                .font(.callout.monospaced())
                .foregroundStyle(Color.primary)
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
                        .foregroundStyle(Color.secondary)
                }
                .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
            }
        } else {
            Section {
                if storedKeyCollapsed {
                    LabeledContent {
                        Text("••••••••••••").font(.callout.monospaced()).foregroundStyle(Color.secondary)
                    } label: {
                        Text(String(localized: "settings_provider_api_key")).foregroundStyle(Color.primary)
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
                        Button { showKey.toggle() } label: {
                            Image(systemName: showKey ? "eye.slash" : "eye")
                                .frame(minWidth: 44, minHeight: 44)
                        }
                        .accessibilityLabel(showKey ? String(localized: "settings_provider_hide_key") : String(localized: "settings_provider_show_key"))
                        .buttonStyle(.borderless)
                        .font(.body)
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
            .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
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
                        Text(String(localized: "settings_provider_oauth")).foregroundStyle(Color.primary)
                    }
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
            .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
        }
    }

    // MARK: Model

    private var modelSection: some View {
        Section {
            NavigationLink {
                ProviderModelsPage(draft: $draft)
            } label: {
                VStack(alignment: .leading, spacing: 4) {
                    Text("session_details_model").foregroundStyle(Color.primary)
                    Text(draft.profile.modelID.isEmpty ? "—" : ModelDisplay.modelName(for: draft.profile.modelID))
                        .font(.subheadline).foregroundStyle(Color.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.vertical, 2)
            }
            .accessibilityIdentifier("provider.model")
        }
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
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
                    Text(String(localized: "settings_provider_set_default")).foregroundStyle(Color.primary)
                    Spacer(minLength: 8)
                    if draft.profile.isDefault {
                        Image(systemName: "checkmark").fontWeight(.semibold).foregroundStyle(t.accent)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
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
            if !draft.profile.connections.isEmpty {
                Text("settings_provider_connections_hint")
            }
        }
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
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
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
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
            .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
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
    @State private var query = ""
    @State private var visibilityQuery = ""
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
            customModelSection(catalog)
            Section {
                NavigationLink {
                    List { pickerSection(catalog, visible: visible) }
                        .listStyle(.insetGrouped)
                        .searchable(text: $visibilityQuery, prompt: Text("composer_search_models_placeholder"))
                        .navigationTitle("settings_provider_visible_models")
                        .navigationBarTitleDisplayMode(.inline)
                } label: {
                    LabeledContent("settings_provider_visible_models") {
                        if !draft.profile.showInModelPicker {
                            Text("settings_badge_disabled")
                                .foregroundStyle(.secondary)
                        } else if catalog.isReady {
                            Text(catalog.summary)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                .accessibilityIdentifier("provider.model.visibility")
            }
            if catalog.isReady {
                modelListSection(catalog)
            } else {
                Section {
                    HStack(spacing: 10) {
                        if catalog.isLoading { ProgressView() }
                        Text(catalog.summary).font(.footnote).foregroundStyle(Color.secondary)
                    }
                }
                .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color(uiColor: .systemGroupedBackground))
        .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: Text("composer_search_models_placeholder"))
        .scrollDismissesKeyboard(.interactively)
        .navigationTitle(String(localized: "session_details_model"))
        .navigationBarTitleDisplayMode(.inline)
        .sheet(item: $selectedModelDetails) { details in
            ModelDetailsSheet(details: details, accent: Accents.color(for: draft.profile.presetID))
        }
    }

    private func matchingModels(_ catalog: Catalog, query: String) -> [String] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        return query.isEmpty ? catalog.models : catalog.models.filter {
            $0.localizedStandardContains(query)
                || (catalog.details[$0]?.preferredDisplayName?.localizedStandardContains(query) ?? false)
        }
    }

    private func modelListSection(_ catalog: Catalog) -> some View {
        let models = matchingModels(catalog, query: query)
        return Section {
            if models.isEmpty { ContentUnavailableView.search(text: query) }
            ForEach(models, id: \.self) { model in
                let details = catalog.details[model]
                HStack(spacing: 8) {
                    Button { draft.profile.modelID = model } label: {
                        HStack(spacing: 8) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(details?.preferredDisplayName ?? ModelDisplay.modelName(for: model))
                                    .foregroundStyle(Color.primary)
                                if (details?.preferredDisplayName ?? ModelDisplay.modelName(for: model)) != model {
                                    Text(model)
                                        .font(.subheadline)
                                        .foregroundStyle(Color.secondary)
                                        .fixedSize(horizontal: false, vertical: true)
                                }
                            }
                            Spacer(minLength: 8)
                            if draft.profile.modelID == model {
                                Image(systemName: "checkmark").fontWeight(.semibold).foregroundStyle(t.accent)
                            }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.borderless)
                    .accessibilityIdentifier("provider.model.select.\(model)")
                    .accessibilityAddTraits(draft.profile.modelID == model ? [.isSelected] : [])
                    if let details {
                        Button { selectedModelDetails = details } label: {
                            Image(systemName: "info.circle")
                                .foregroundStyle(t.accent)
                                .frame(minWidth: 44, minHeight: 44)
                        }
                        .buttonStyle(.borderless)
                        .accessibilityLabel(String(localized: "composer_model_details"))
                        .accessibilityValue(details.preferredDisplayName ?? model)
                        .accessibilityIdentifier("provider.model.info.\(model)")
                    }
                }
            }
        }
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
    }

    private func customModelSection(_ catalog: Catalog) -> some View {
        Section {
            if catalog.isReady {
                LabeledContent("settings_provider_default_model") {
                    Text(draft.profile.modelID.isEmpty
                         ? String(localized: "settings_provider_unconfigured")
                         : ModelDisplay.modelName(for: draft.profile.modelID))
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.trailing)
                }
                DisclosureGroup("settings_provider_custom_model_id") {
                    customModelField(catalog.placeholder)
                    Text("settings_provider_custom_model_hint")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            } else {
                customModelField(catalog.placeholder)
            }
        } header: {
            if !catalog.isReady { Text("settings_provider_default_model") }
        } footer: {
            if !catalog.isReady { Text("settings_provider_custom_model_hint") }
        }
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
    }

    private func customModelField(_ placeholder: String) -> some View {
        TextField(placeholder, text: $draft.profile.modelID)
            .font(.body)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .accessibilityLabel(String(localized: "settings_provider_custom_model_id"))
            .accessibilityIdentifier("provider.model.custom")
    }

    @ViewBuilder
    private func pickerSection(_ catalog: Catalog, visible: [String]) -> some View {
        Section {
            Toggle(isOn: Binding(
                get: { draft.profile.showInModelPicker },
                set: { draft.profile.showInModelPicker = $0 }
            )) {
                Text(String(localized: "settings_provider_show_in_picker")).foregroundStyle(Color.primary)
            }
            .tint(t.accent)
            .accessibilityIdentifier("provider.model.visible")
            if catalog.isReady, draft.profile.showInModelPicker {
                ForEach(matchingModels(catalog, query: visibilityQuery), id: \.self) { model in
                    Toggle(isOn: Binding(
                        get: { visible.contains(model) },
                        set: { _ in toggleVisibleModel(model, from: catalog.models) }
                    )) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(catalog.details[model]?.preferredDisplayName ?? ModelDisplay.modelName(for: model))
                                .foregroundStyle(Color.primary)
                            Text(model)
                                .font(.subheadline)
                                .foregroundStyle(Color.secondary)
                                .fixedSize(horizontal: false, vertical: true)
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
        .listRowBackground(Color(uiColor: .secondarySystemGroupedBackground))
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
