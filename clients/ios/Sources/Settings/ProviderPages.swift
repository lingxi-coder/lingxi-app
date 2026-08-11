import SwiftUI

// MARK: - Provider list
struct ProviderListPage: View {
    @Environment(\.theme) private var t
    @State private var repository = ProviderRepository.shared
    @State private var retryMaxAttemptsText = ""
    @State private var retryBackoffMsText = ""
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let kind: ProviderKind

    private var blurb: String {
        switch kind {
        case .llm:    return String(localized: "provider_llm_blurb")
        case .search: return String(localized: "provider_search_blurb")
        case .fetch:  return String(localized: "provider_fetch_blurb")
        }
    }

    var body: some View {
        let arr = kind == .llm ? repository.legacyProviders() : [GenericProvider]()
        VStack(spacing: 0) {
            Text(blurb).font(.system(size: 11.5)).foregroundColor(t.text3).lineSpacing(4)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.bottom, 14)

            SettingsSection(label: String(localized: "settings_provider_section_added_count \(arr.count)")) {
                if arr.isEmpty {
                    Text("provider_no_providers_added").font(.system(size: 13)).foregroundColor(t.text4)
                        .frame(maxWidth: .infinity).padding(.vertical, 24)
                }
                ForEach(Array(arr.enumerated()), id: \.element.id) { i, p in
                    let preset = kind == .llm ? repository.preset(for: p.preset) : store.preset(of: p, kind: kind)
                    SettingsRow(
                        labelView: AnyView(HStack(spacing: 6) {
                            Text(p.name).font(.system(size: 14, weight: .medium)).foregroundColor(t.text)
                            if p.isDefault { badge(String(localized: "settings_badge_default"), t.accent) }
                            if !p.enabled { badge(String(localized: "settings_badge_disabled"), t.text4) }
                        }),
                        subView: AnyView(HStack(spacing: 5) {
                            Circle().fill(p.status.dot(t)).frame(width: 6, height: 6)
                            Text("\(p.status.label) · \(p.model.isEmpty ? "—" : p.model)")
                                .font(.system(size: 11.5, design: .monospaced)).foregroundColor(t.text4)
                        }),
                        chevron: true, isLast: i == arr.count - 1,
                        onTap: { host.push(.providerEdit(.init(kind), p.id)) }
                    ) {
                        Text(String(p.name.prefix(1)))
                            .font(.system(size: 13, weight: .bold)).foregroundColor(preset.color)
                            .frame(width: 28, height: 28)
                            .background(preset.color.mix(with: t.surface, amount: 0.18))
                            .clipShape(RoundedRectangle(cornerRadius: 7))
                            .overlay(RoundedRectangle(cornerRadius: 7).stroke(preset.color.tint(0.28), lineWidth: 0.5))
                    }
                }
            }

            if kind == .llm {
                DashedAddButton(title: String(localized: "provider_add_kind \(kind.title)")) {
                    host.push(.providerPicker(.init(kind)))
                }
                .accessibilityIdentifier("provider.add")
                .padding(.bottom, 22)
            } else {
                unsupportedProviderNotice
                    .padding(.bottom, 22)
            }

            if kind == .llm {
                routingSection
            }
        }
        .onChange(of: repository.syncRevision) { _, _ in
            if kind == .llm {
                store.llmProviders = repository.legacyProviders()
            } else if kind == .search {
                store.searchProviders = []
            } else {
                store.fetchProviders = []
            }
        }
        .task {
            guard kind == .llm else { return }
            retryMaxAttemptsText = String(repository.routingSettings.retryMaxAttempts)
            retryBackoffMsText = String(repository.routingSettings.retryBackoffMs)
            store.llmProviders = repository.legacyProviders()
            await repository.refreshCredentialStatus()
        }
    }

    @ViewBuilder
    private var routingSection: some View {
        let snapshot = repository.makeLaunchSnapshot()
        let candidates = repository.fallbackCandidates()
        SettingsSection(
            label: String(localized: "settings_provider_section_routing"),
            footer: String(localized: "settings_provider_routing_footer \(ProviderRoutingSettings.minRetryMaxAttempts) \(ProviderRoutingSettings.maxRetryMaxAttempts) \(ProviderRoutingSettings.minRetryBackoffMs) \(ProviderRoutingSettings.maxRetryBackoffMs)")
        ) {
            VStack(alignment: .leading, spacing: 12) {
                routeField(
                    label: String(localized: "settings_provider_default_model"),
                    text: .constant(snapshot.defaultModelID ?? String(localized: "settings_provider_unconfigured")),
                    editable: false
                )
                routeField(
                    label: String(localized: "settings_provider_max_retries"),
                    text: $retryMaxAttemptsText
                )
                routeField(
                    label: String(localized: "settings_provider_backoff_ms"),
                    text: $retryBackoffMsText
                )
                VStack(alignment: .leading, spacing: 8) {
                    Text("settings_provider_fallback_order")
                        .font(.system(size: 12.5, weight: .medium))
                        .foregroundColor(t.text2)
                    if candidates.isEmpty {
                        Text("settings_provider_fallback_hint")
                            .font(.system(size: 11.5))
                            .foregroundColor(t.text4)
                    } else {
                        ForEach(candidates) { candidate in
                            fallbackRow(candidate)
                        }
                    }
                }
                if let message = repository.routingMessage {
                    Text(message)
                        .font(.system(size: 11.5))
                        .foregroundColor(repository.routingDirty ? t.text4 : t.text2)
                }
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
                    Text("settings_provider_apply_routing")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(.white)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 11)
                        .background(hasPendingRoutingChanges ? t.accent : t.text4)
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                }
                .disabled(!hasPendingRoutingChanges)
                .buttonStyle(.plain)
            }
            .padding(.vertical, 4)
        }
    }

    private var hasPendingRoutingChanges: Bool {
        repository.routingDirty ||
            retryMaxAttemptsText.trimmingCharacters(in: .whitespacesAndNewlines)
                != String(repository.routingSettings.retryMaxAttempts) ||
            retryBackoffMsText.trimmingCharacters(in: .whitespacesAndNewlines)
                != String(repository.routingSettings.retryBackoffMs)
    }

    private var unsupportedProviderNotice: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("settings_provider_not_connected")
                .font(.system(size: 13.5, weight: .semibold))
                .foregroundColor(t.text)
            Text("settings_provider_unsupported_notice \(kind.title)")
                .font(.system(size: 11.5))
                .foregroundColor(t.text4)
                .lineSpacing(4)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(t.border, lineWidth: 0.5))
    }

    private func badge(_ text: String, _ color: Color) -> some View {
        Text(text).font(.system(size: 10, weight: text == String(localized: "settings_badge_default") ? .semibold : .medium))
            .foregroundColor(color)
            .padding(.horizontal, 6).padding(.vertical, 1)
            .background(color.tint(0.18)).clipShape(RoundedRectangle(cornerRadius: 4))
    }

    @ViewBuilder
    private func routeField(
        label: String,
        text: Binding<String>,
        editable: Bool = true
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label)
                .font(.system(size: 12.5, weight: .medium))
                .foregroundColor(t.text2)
            if editable {
                SettingsField(text: text)
            } else {
                Text(text.wrappedValue)
                    .font(.system(size: 12.5, design: .monospaced))
                    .foregroundColor(t.text4)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 11)
                    .background(t.surface)
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                    .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
            }
        }
    }

    private func fallbackRow(_ candidate: ProviderFallbackCandidate) -> some View {
        HStack(spacing: 10) {
            Button {
                repository.toggleFallbackProfile(candidate.profileID)
            } label: {
                Circle()
                    .strokeBorder(candidate.selected ? t.accent : t.border, lineWidth: 1.5)
                    .frame(width: 18, height: 18)
                    .overlay(candidate.selected ? Circle().fill(t.accent).frame(width: 9, height: 9) : nil)
            }
            .buttonStyle(.plain)
            VStack(alignment: .leading, spacing: 2) {
                Text(candidate.name)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(t.text)
                Text(candidate.modelID)
                    .font(.system(size: 11.5, design: .monospaced))
                    .foregroundColor(t.text4)
            }
            Spacer()
            if candidate.selected {
                HStack(spacing: 6) {
                    Button {
                        repository.moveFallbackProfile(candidate.profileID, by: -1)
                    } label: {
                        LXIcon(name: .chevronR, size: 12, color: t.text3, stroke: 1.7)
                            .rotationEffect(.degrees(-90))
                            .frame(width: 24, height: 24)
                    }
                    .buttonStyle(.plain)
                    .disabled((candidate.order ?? 0) == 0)
                    Button {
                        repository.moveFallbackProfile(candidate.profileID, by: 1)
                    } label: {
                        LXIcon(name: .chevronR, size: 12, color: t.text3, stroke: 1.7)
                            .rotationEffect(.degrees(90))
                            .frame(width: 24, height: 24)
                    }
                    .buttonStyle(.plain)
                    .disabled((candidate.order ?? 0) == repository.routingSettings.fallbackProfileIDs.count - 1)
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
    }
}

// MARK: - Provider picker (preset list)
struct ProviderPickerPage: View {
    @Environment(\.theme) private var t
    @State private var repository = ProviderRepository.shared
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let kind: ProviderKind

    @ViewBuilder
    var body: some View {
        if kind != .llm {
            VStack(alignment: .leading, spacing: 10) {
                Text("settings_provider_picker_unavailable")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(t.text)
                Text("settings_provider_picker_unavailable_detail")
                    .font(.system(size: 11.5))
                    .foregroundColor(t.text4)
                    .lineSpacing(4)
            }
        } else {
            VStack(spacing: 0) {
                Text("settings_provider_picker_intro")
                    .font(.system(size: 11.5)).foregroundColor(t.text3).lineSpacing(4)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.bottom, 14)
                SettingsSection {
                    ForEach(Array(kind.presets.enumerated()), id: \.element.id) { i, p in
                        Button { add(p.id) } label: {
                            VStack(spacing: 0) {
                                HStack(spacing: 12) {
                                    Text(String(p.name.prefix(1)))
                                        .font(.system(size: 14, weight: .bold)).foregroundColor(p.color)
                                        .frame(width: 32, height: 32)
                                        .background(p.color.mix(with: t.surface, amount: 0.18))
                                        .clipShape(RoundedRectangle(cornerRadius: 8))
                                        .overlay(RoundedRectangle(cornerRadius: 8).stroke(p.color.tint(0.28), lineWidth: 0.5))
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(p.name).font(.system(size: 14, weight: .semibold)).foregroundColor(t.text)
                                        Text(p.sub + (p.models.isEmpty ? "" : String(localized: "settings_provider_models_count \(p.models.count)")))
                                            .font(.system(size: 11.5)).foregroundColor(t.text4)
                                    }
                                    Spacer()
                                    LXIcon(name: .plus, size: 15, color: t.text4, stroke: 2)
                                }
                                .padding(.horizontal, 14).padding(.vertical, 12)
                                if i < kind.presets.count - 1 { Rectangle().fill(t.border).frame(height: 0.5) }
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
    }

    private func add(_ presetId: String) {
        let id: String
        if kind == .llm {
            id = repository.addProfile(presetID: presetId)
            store.llmProviders = repository.legacyProviders()
        } else {
            id = store.addProvider(kind, presetId: presetId)
        }
        host.replaceTopTwo(with: [.providerList(.init(kind)), .providerEdit(.init(kind), id)])
    }
}

// MARK: - Provider edit
struct ProviderEditPage: View {
    @Environment(\.theme) private var t
    @State private var repository = ProviderRepository.shared
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let kind: ProviderKind
    let providerId: String

    @State private var showKey = false
    @State private var editingStoredCredential = false

    @ViewBuilder
    var body: some View {
        if kind != .llm {
            VStack(alignment: .leading, spacing: 10) {
                Text("settings_provider_edit_unavailable")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(t.text)
                Text("settings_provider_edit_unavailable_detail")
                    .font(.system(size: 11.5))
                    .foregroundColor(t.text4)
                    .lineSpacing(4)
            }
        } else if let editing = repository.state(for: providerId) {
            let preset = repository.preset(for: editing.profile.presetID)
            VStack(spacing: 0) {
                statusBanner(editing)

            FieldLabel(text: String(localized: "settings_display_name"))
            SettingsField(text: binding(\.name), mono: false).padding(.bottom, 14)

            FieldLabel(text: String(localized: "settings_api_url"))
            SettingsField(text: binding(\.baseURL), placeholder: preset.defaultUrl)
                .disabled(repository.oauthProvider(for: editing.profile.presetID) != nil)
            FieldHint(String(localized: "settings_provider_url_hint \(preset.defaultUrl.isEmpty ? "—" : preset.defaultUrl)"))

            if editing.profile.presetID != "openai-chatgpt" {
                FieldLabel(text: "API Key")
                keyField(editing, preset)
                FieldHint(editing.maskedCredentialSummary + String(localized: "settings_provider_key_hint_suffix"))
            }

            if repository.oauthProvider(for: editing.profile.presetID) != nil {
                oauthSection(editing)
            }

            if !preset.models.isEmpty { modelPicker(editing, preset) }
            FieldLabel(text: preset.models.isEmpty
                ? String(localized: "settings_provider_model_id")
                : String(localized: "settings_provider_custom_model_id"))
            SettingsField(
                text: binding(\.modelID),
                placeholder: preset.models.first ?? "llama-3.3-70b"
            )
            .padding(.bottom, 6)
            if !preset.models.isEmpty {
                FieldHint(String(localized: "settings_provider_custom_model_hint"))
            }

            SettingsSection {
                SettingsRow(label: String(localized: "settings_provider_enable"), chevron: false) {
                    LXToggle(isOn: Binding(get: { editing.profile.enabled },
                                           set: { v in repository.updateProfile(providerId) { $0.enabled = v } }))
                }
                SettingsRow(label: String(localized: "settings_provider_set_default"), chevron: false, isLast: true,
                            onTap: { repository.setDefaultProfile(providerId) }) {
                    if editing.profile.isDefault { LXIcon(name: .check, size: 16, color: t.accent, stroke: 2.2) }
                    else { Text("settings_status_not_enabled").font(.system(size: 12)).foregroundColor(t.text4) }
                }
            }

            Button {
                Task {
                    await repository.removeProfile(providerId)
                    store.llmProviders = repository.legacyProviders()
                    host.pop()
                }
            } label: {
                Text("settings_provider_remove").font(.system(size: 13.5, weight: .medium)).foregroundColor(t.danger)
                    .frame(maxWidth: .infinity).padding(12)
                    .overlay(RoundedRectangle(cornerRadius: 11).stroke(t.border, lineWidth: 0.5))
            }
            .padding(.top, 8)
            .disabled(editing.operationInFlight || editing.connectionState == .testing)
            .buttonStyle(.plain)

            Button {
                Task {
                    await repository.applyChanges(providerId)
                    store.llmProviders = repository.legacyProviders()
                    editingStoredCredential = false
                    showKey = false
                }
            } label: {
                Text("settings_provider_apply_reconnect")
                    .font(.system(size: 13.5, weight: .semibold))
                    .foregroundColor(.white)
                    .frame(maxWidth: .infinity)
                    .padding(12)
                    .background(t.accent)
                    .clipShape(RoundedRectangle(cornerRadius: 11))
            }
            .padding(.top, 10)
            .disabled(editing.operationInFlight || editing.connectionState == .testing)
            .buttonStyle(.plain)

            }
            .task(id: repository.syncRevision) {
                store.llmProviders = repository.legacyProviders()
            }
        } else {
            EmptyView()
                .task { host.pop() }
        }
    }

    private func binding(_ keyPath: WritableKeyPath<ProviderStoredProfile, String>) -> Binding<String> {
        Binding(
            get: { repository.state(for: providerId)?.profile[keyPath: keyPath] ?? "" },
            set: { value in
                repository.updateProfile(providerId) { $0[keyPath: keyPath] = value }
            }
        )
    }

    private func statusBanner(_ p: ProviderProfileState) -> some View {
        let dot = p.legacyStatus.dot(t)
        return HStack(spacing: 8) {
            Circle().fill(dot).frame(width: 8, height: 8)
                .overlay(Circle().stroke(dot.tint(0.22), lineWidth: 3).scaleEffect(1.75))
            VStack(alignment: .leading, spacing: 2) {
                Text(p.statusLabel).font(.system(size: 12.5, weight: .medium)).foregroundColor(t.text2)
                if let detail = p.validationMessage ?? p.detailMessage {
                    Text(detail).font(.system(size: 11.5)).foregroundColor(t.text4).lineLimit(2)
                }
            }
            Spacer()
            HStack(spacing: 8) {
                Button {
                    Task {
                        await repository.testConnection(providerId)
                        store.llmProviders = repository.legacyProviders()
                    }
                } label: {
                    Text("settings_provider_test_connection").font(.system(size: 12, weight: .medium)).foregroundColor(t.text2)
                        .padding(.horizontal, 10).padding(.vertical, 6)
                        .background(t.windowBg).clipShape(RoundedRectangle(cornerRadius: 7))
                        .overlay(RoundedRectangle(cornerRadius: 7).stroke(t.border, lineWidth: 0.5))
                }
                .disabled(p.operationInFlight || p.connectionState == .testing)
                .buttonStyle(.plain)

                if p.clearCredentialOnApply {
                    Button("settings_provider_undo_clear") {
                        repository.cancelCredentialClear(for: providerId)
                    }
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundColor(t.text3)
                    .buttonStyle(.plain)
                }
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .background(dot.mix(with: t.surface, amount: 0.08))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(dot.tint(0.24), lineWidth: 0.5))
        .padding(.bottom, 18)
    }

    private func keyField(_ p: ProviderProfileState, _ preset: ProviderPreset) -> some View {
        HStack(spacing: 0) {
            if let mask = p.credentialFieldMask, !editingStoredCredential {
                HStack(spacing: 8) {
                    Text(mask)
                        .font(.system(size: 13.5, design: .monospaced))
                        .foregroundColor(t.text)
                    Text("settings_provider_key_stored_securely")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundColor(t.ok)
                    Spacer(minLength: 4)
                }
                .padding(.leading, 12)
                .padding(.trailing, 8)
                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("provider.api-key.stored")

                keyFieldDivider

                Button {
                    editingStoredCredential = true
                } label: {
                    Text("settings_provider_replace_key")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundColor(t.text3)
                        .frame(minWidth: 44, minHeight: 44)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("provider.api-key.replace")

                keyFieldDivider
            } else {
                Group {
                    if showKey {
                        TextField(keyPlaceholder(p, preset), text: pendingSecretBinding)
                    } else {
                        SecureField(keyPlaceholder(p, preset), text: pendingSecretBinding)
                    }
                }
                .font(.system(size: 13.5, design: .monospaced))
                .foregroundColor(t.text)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .lineLimit(1)
                .padding(.vertical, 11)
                .padding(.leading, 12)
                .padding(.trailing, 8)
                .frame(maxWidth: .infinity)
                .layoutPriority(1)
                .accessibilityIdentifier("provider.api-key")

                if p.canRevealCredential {
                    keyFieldDivider

                    Button { showKey.toggle() } label: {
                        Text(showKey ? String(localized: "settings_provider_hide_key") : String(localized: "settings_provider_show_key"))
                            .font(.system(size: 11, weight: .medium))
                            .foregroundColor(t.text3)
                            .frame(minWidth: 44, minHeight: 44)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(showKey ? String(localized: "settings_provider_hide_api_key_accessibility") : String(localized: "settings_provider_show_api_key_accessibility"))
                    .accessibilityIdentifier("provider.api-key.visibility")
                }

                keyFieldDivider
            }

            Button {
                if p.clearCredentialOnApply {
                    repository.cancelCredentialClear(for: providerId)
                } else {
                    repository.clearCredentialRequest(for: providerId)
                    editingStoredCredential = false
                    showKey = false
                }
            } label: {
                Text(p.clearCredentialOnApply ? String(localized: "settings_provider_keep_key") : String(localized: "settings_provider_clear_key"))
                    .font(.system(size: 11, weight: .medium))
                    .foregroundColor(p.clearCredentialOnApply ? t.text3 : t.danger)
                    .frame(minWidth: 44, minHeight: 44)
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("provider.api-key.clear")
        }
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
    }

    @ViewBuilder
    private func oauthSection(_ p: ProviderProfileState) -> some View {
        SettingsSection(label: "OAuth") {
            VStack(alignment: .leading, spacing: 10) {
                if let oauth = p.oauthState, oauth.signedIn {
                    HStack(spacing: 8) {
                        Circle().fill(t.ok).frame(width: 7, height: 7)
                        Text("已登录")
                            .font(.system(size: 13, weight: .medium))
                            .foregroundColor(t.text)
                        if let account = oauth.accountLabel, !account.isEmpty {
                            Text(account)
                                .font(.system(size: 11.5, design: .monospaced))
                                .foregroundColor(t.text4)
                                .lineLimit(1)
                        }
                        Spacer()
                    }
                    Text(p.hasStoredAPIKey ? "当前使用 API Key（OAuth 备用）" : "当前使用 OAuth")
                        .font(.system(size: 11.5))
                        .foregroundColor(t.text4)
                    HStack(spacing: 8) {
                        Button {
                            Task {
                                await repository.testConnection(providerId)
                                store.llmProviders = repository.legacyProviders()
                            }
                        } label: {
                            Text("测试 OAuth")
                                .font(.system(size: 12, weight: .medium))
                                .foregroundColor(t.text2)
                                .padding(.horizontal, 10).padding(.vertical, 7)
                                .background(t.windowBg)
                                .clipShape(RoundedRectangle(cornerRadius: 7))
                                .overlay(RoundedRectangle(cornerRadius: 7).stroke(t.border, lineWidth: 0.5))
                        }
                        .buttonStyle(.plain)
                        .disabled(p.operationInFlight || p.connectionState == .testing)

                        Button {
                            Task {
                                await repository.logoutOAuth(for: providerId)
                                store.llmProviders = repository.legacyProviders()
                            }
                        } label: {
                            Text("退出 OAuth")
                                .font(.system(size: 12, weight: .medium))
                                .foregroundColor(t.danger)
                        }
                        .buttonStyle(.plain)
                        .disabled(p.operationInFlight)
                    }
                } else {
                    Text("使用系统浏览器登录，不会把 refresh token 交给 Swift UI。")
                        .font(.system(size: 11.5))
                        .foregroundColor(t.text4)
                        .lineSpacing(3)
                    Button {
                        Task {
                            await repository.loginOAuth(for: providerId)
                            store.llmProviders = repository.legacyProviders()
                        }
                    } label: {
                        Text(p.profile.presetID == "openai-chatgpt" ? "登录 ChatGPT" : "登录 Anthropic")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(.white)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 10)
                            .background(p.operationInFlight ? t.text4 : t.accent)
                            .clipShape(RoundedRectangle(cornerRadius: 9))
                    }
                    .buttonStyle(.plain)
                    .disabled(p.operationInFlight)
                }
            }
            .padding(.vertical, 3)
        }
    }

    private var pendingSecretBinding: Binding<String> {
        Binding(
            get: { repository.state(for: providerId)?.pendingSecret ?? "" },
            set: { repository.stageSecret($0, for: providerId) }
        )
    }

    private func keyPlaceholder(_ p: ProviderProfileState, _ preset: ProviderPreset) -> String {
        if p.clearCredentialOnApply { return String(localized: "settings_provider_key_will_be_deleted") }
        if p.hasStoredCredential { return String(localized: "settings_provider_key_stored_securely") }
        return preset.keyPrefix + "..."
    }

    private var keyFieldDivider: some View {
        Rectangle()
            .fill(t.border)
            .frame(width: 0.5, height: 24)
    }

    private func modelPicker(_ p: ProviderProfileState, _ preset: ProviderPreset) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            FieldLabel(text: String(localized: "settings_provider_default_model"))
            VStack(spacing: 0) {
                ForEach(Array(preset.models.enumerated()), id: \.element) { i, m in
                    let sel = p.profile.modelID == m
                    Button { repository.updateProfile(providerId) { $0.modelID = m } } label: {
                        VStack(spacing: 0) {
                            HStack(spacing: 10) {
                                Circle().strokeBorder(sel ? preset.color : t.border, lineWidth: 1.5)
                                    .frame(width: 16, height: 16)
                                    .overlay(sel ? Circle().fill(preset.color).frame(width: 8, height: 8) : nil)
                                Text(m).font(.system(size: 13, design: .monospaced)).foregroundColor(t.text)
                                Spacer()
                            }
                            .padding(.horizontal, 12).padding(.vertical, 11)
                            if i < preset.models.count - 1 { Rectangle().fill(t.border).frame(height: 0.5) }
                        }
                    }
                    .buttonStyle(.plain)
                }
            }
            .background(t.surface).clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(t.border, lineWidth: 0.5))
            .padding(.bottom, 14)
        }
    }
}
