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
        case .llm:    return "灵犀本身不调用云端 LLM — 由你添加的提供商完成推理。密钥仅本机加密。"
        case .search: return "让 AI 接入实时网页搜索 — 选择一个搜索提供商以启用\"联网\"模式。"
        case .fetch:  return "抓取网页正文用于阅读、摘要、引用 — 推荐 Jina Reader（免费）。"
        }
    }

    var body: some View {
        let arr = kind == .llm ? repository.legacyProviders() : [GenericProvider]()
        VStack(spacing: 0) {
            Text(blurb).font(.system(size: 11.5)).foregroundColor(t.text3).lineSpacing(4)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.bottom, 14)

            SettingsSection(label: "已添加 · \(arr.count)") {
                if arr.isEmpty {
                    Text("尚未添加任何提供商").font(.system(size: 13)).foregroundColor(t.text4)
                        .frame(maxWidth: .infinity).padding(.vertical, 24)
                }
                ForEach(Array(arr.enumerated()), id: \.element.id) { i, p in
                    let preset = kind == .llm ? repository.preset(for: p.preset) : store.preset(of: p, kind: kind)
                    SettingsRow(
                        labelView: AnyView(HStack(spacing: 6) {
                            Text(p.name).font(.system(size: 14, weight: .medium)).foregroundColor(t.text)
                            if p.isDefault { badge("默认", t.accent) }
                            if !p.enabled { badge("停用", t.text4) }
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
                DashedAddButton(title: "添加\(kind.title)") { host.push(.providerPicker(.init(kind))) }
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
            label: "路由与重试",
            footer: "仅对默认模型生效。最大重试次数限制为 \(ProviderRoutingSettings.minRetryMaxAttempts)-\(ProviderRoutingSettings.maxRetryMaxAttempts)，退避范围为 \(ProviderRoutingSettings.minRetryBackoffMs)-\(ProviderRoutingSettings.maxRetryBackoffMs) ms。"
        ) {
            VStack(alignment: .leading, spacing: 12) {
                routeField(
                    label: "默认模型",
                    text: .constant(snapshot.defaultModelID ?? "未配置默认模型"),
                    editable: false
                )
                routeField(
                    label: "最大重试次数",
                    text: $retryMaxAttemptsText
                )
                routeField(
                    label: "退避毫秒",
                    text: $retryBackoffMsText
                )
                VStack(alignment: .leading, spacing: 8) {
                    Text("默认模型 fallback 顺序")
                        .font(.system(size: 12.5, weight: .medium))
                        .foregroundColor(t.text2)
                    if candidates.isEmpty {
                        Text("启用至少一个非默认 Provider 并设置模型后，才能为默认模型配置 fallback。")
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
                    Text("应用路由并重连")
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
            Text("尚未接入真实配置仓储")
                .font(.system(size: 13.5, weight: .semibold))
                .foregroundColor(t.text)
            Text("iOS 当前版本只移除了原型卡片；\(kind.title) 会显示真实空状态，待对应后端与会话重连链路接入后再开放新增与编辑。")
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
        Text(text).font(.system(size: 10, weight: text == "默认" ? .semibold : .medium))
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
                Text("此类别尚未开放新增。")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(t.text)
                Text("联网搜索与网页抓取的真实 Provider 仓储还未在 iOS 侧接通，因此这里只保留空状态，不再提供原型预设卡片。")
                    .font(.system(size: 11.5))
                    .foregroundColor(t.text4)
                    .lineSpacing(4)
            }
        } else {
            VStack(spacing: 0) {
                Text("选择一个预设 — 灵犀会自动预填 API 地址、Key 前缀和可用模型。")
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
                                        Text(p.sub + (p.models.isEmpty ? "" : " · \(p.models.count) 模型"))
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

    @ViewBuilder
    var body: some View {
        if kind != .llm {
            VStack(alignment: .leading, spacing: 10) {
                Text("此类别尚未开放编辑。")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(t.text)
                Text("为了移除模拟卡片，这里不再读取任何预置 Provider 状态。等待对应真实后端接入后再开放编辑。")
                    .font(.system(size: 11.5))
                    .foregroundColor(t.text4)
                    .lineSpacing(4)
            }
        } else if let editing = repository.state(for: providerId) {
            let preset = repository.preset(for: editing.profile.presetID)
            VStack(spacing: 0) {
                statusBanner(editing)

            FieldLabel(text: "显示名称")
            SettingsField(text: binding(\.name), mono: false).padding(.bottom, 14)

            FieldLabel(text: "API 地址")
            SettingsField(text: binding(\.baseURL), placeholder: preset.defaultUrl)
            FieldHint("默认 `\(preset.defaultUrl.isEmpty ? "—" : preset.defaultUrl)` · 可填代理 / 镜像")

            FieldLabel(text: "API Key")
            keyField(editing, preset)
            FieldHint(editing.maskedCredentialSummary + " · 密钥永不写入普通偏好。")

            if !preset.models.isEmpty { modelPicker(editing, preset) }
            FieldLabel(text: preset.models.isEmpty ? "模型 ID" : "自定义模型 ID")
            SettingsField(
                text: binding(\.modelID),
                placeholder: preset.models.first ?? "llama-3.3-70b"
            )
            .padding(.bottom, 6)
            if !preset.models.isEmpty {
                FieldHint("可直接输入未列出的模型 ID；上方预设选择器会写回这个字段。")
            }

            SettingsSection {
                SettingsRow(label: "启用", chevron: false) {
                    LXToggle(isOn: Binding(get: { editing.profile.enabled },
                                           set: { v in repository.updateProfile(providerId) { $0.enabled = v } }))
                }
                SettingsRow(label: "设为默认", chevron: false, isLast: true,
                            onTap: { repository.setDefaultProfile(providerId) }) {
                    if editing.profile.isDefault { LXIcon(name: .check, size: 16, color: t.accent, stroke: 2.2) }
                    else { Text("未启用").font(.system(size: 12)).foregroundColor(t.text4) }
                }
            }

            Button {
                Task {
                    await repository.removeProfile(providerId)
                    store.llmProviders = repository.legacyProviders()
                    host.pop()
                }
            } label: {
                Text("移除此提供商").font(.system(size: 13.5, weight: .medium)).foregroundColor(t.danger)
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
                }
            } label: {
                Text("应用并重连")
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
                    Text("测试连接").font(.system(size: 12, weight: .medium)).foregroundColor(t.text2)
                        .padding(.horizontal, 10).padding(.vertical, 6)
                        .background(t.windowBg).clipShape(RoundedRectangle(cornerRadius: 7))
                        .overlay(RoundedRectangle(cornerRadius: 7).stroke(t.border, lineWidth: 0.5))
                }
                .disabled(p.operationInFlight || p.connectionState == .testing)
                .buttonStyle(.plain)

                if p.clearCredentialOnApply {
                    Button("撤销清除") {
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

            keyFieldDivider

            Button { showKey.toggle() } label: {
                Text(showKey ? "隐藏" : "显示")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundColor(t.text3)
                    .frame(minWidth: 44, minHeight: 44)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(showKey ? "隐藏 API Key" : "显示 API Key")
            .accessibilityIdentifier("provider.api-key.visibility")

            keyFieldDivider

            Button {
                if p.clearCredentialOnApply {
                    repository.cancelCredentialClear(for: providerId)
                } else {
                    repository.clearCredentialRequest(for: providerId)
                }
            } label: {
                Text(p.clearCredentialOnApply ? "保留" : "清除")
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

    private var pendingSecretBinding: Binding<String> {
        Binding(
            get: { repository.state(for: providerId)?.pendingSecret ?? "" },
            set: { repository.stageSecret($0, for: providerId) }
        )
    }

    private func keyPlaceholder(_ p: ProviderProfileState, _ preset: ProviderPreset) -> String {
        if p.clearCredentialOnApply { return "将在应用后删除" }
        if p.hasStoredCredential { return "已保存于安全存储" }
        return preset.keyPrefix + "..."
    }

    private var keyFieldDivider: some View {
        Rectangle()
            .fill(t.border)
            .frame(width: 0.5, height: 24)
    }

    private func modelPicker(_ p: ProviderProfileState, _ preset: ProviderPreset) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            FieldLabel(text: "默认模型")
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
