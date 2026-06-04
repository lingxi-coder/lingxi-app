import SwiftUI

// MARK: - Provider list
struct ProviderListPage: View {
    @Environment(\.theme) private var t
    @ObservedObject var store: SettingsStore
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
        let arr = store.providers(kind)
        VStack(spacing: 0) {
            Text(blurb).font(.system(size: 11.5)).foregroundColor(t.text3).lineSpacing(4)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.bottom, 14)

            SettingsSection(label: "已添加 · \(arr.count)") {
                if arr.isEmpty {
                    Text("尚未添加任何提供商").font(.system(size: 13)).foregroundColor(t.text4)
                        .frame(maxWidth: .infinity).padding(.vertical, 24)
                }
                ForEach(Array(arr.enumerated()), id: \.element.id) { i, p in
                    let preset = store.preset(of: p, kind: kind)
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

            DashedAddButton(title: "添加\(kind.title)") { host.push(.providerPicker(.init(kind))) }
                .padding(.bottom, 22)

            if kind == .llm {
                EngineCredentialsSection()

                SettingsSection(label: "高级",
                                footer: "智能路由：根据任务类型自动选择最合适的模型（推理→Opus / 速度→Mini / 代码→Code）。流式响应：边生成边显示。") {
                    SettingsRow(icon: .sparkle, iconColor: t.accent, label: "智能路由",
                                sub: "自动在已启用提供商间调度", chevron: false) { LXToggle(isOn: $store.smartRouting) }
                    SettingsRow(icon: .workflow, label: "流式响应", chevron: false, isLast: true) { LXToggle(isOn: $store.streamingDefault) }
                }
            }
        }
    }

    private func badge(_ text: String, _ color: Color) -> some View {
        Text(text).font(.system(size: 10, weight: text == "默认" ? .semibold : .medium))
            .foregroundColor(color)
            .padding(.horizontal, 6).padding(.vertical, 1)
            .background(color.tint(0.18)).clipShape(RoundedRectangle(cornerRadius: 4))
    }
}

// MARK: - Provider picker (preset list)
struct ProviderPickerPage: View {
    @Environment(\.theme) private var t
    @ObservedObject var store: SettingsStore
    let host: SettingsHost
    let kind: ProviderKind

    var body: some View {
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

    private func add(_ presetId: String) {
        let id = store.addProvider(kind, presetId: presetId)
        host.replaceTopTwo(with: [.providerList(.init(kind)), .providerEdit(.init(kind), id)])
    }
}

// MARK: - Provider edit
struct ProviderEditPage: View {
    @Environment(\.theme) private var t
    @ObservedObject var store: SettingsStore
    let host: SettingsHost
    let kind: ProviderKind
    let providerId: String

    @State private var showKey = false

    var body: some View {
        guard let editing = store.providers(kind).first(where: { $0.id == providerId }) else {
            DispatchQueue.main.async { host.pop() }
            return AnyView(EmptyView())
        }
        let preset = store.preset(of: editing, kind: kind)
        return AnyView(VStack(spacing: 0) {
            statusBanner(editing)

            FieldLabel(text: "显示名称")
            SettingsField(text: bind(\.name), mono: false).padding(.bottom, 14)

            FieldLabel(text: "API 地址")
            SettingsField(text: bind(\.url), placeholder: preset.defaultUrl)
            FieldHint("默认 `\(preset.defaultUrl.isEmpty ? "—" : preset.defaultUrl)` · 可填代理 / 镜像")

            FieldLabel(text: "API Key")
            keyField(editing, preset)
            FieldHint("密钥仅本地加密存储 · 从不上传到灵犀服务器")

            if preset.needsCx {
                FieldLabel(text: "Custom Search Engine ID (cx)")
                SettingsField(text: bind(\.cx), placeholder: "0123456789abcdef:ghi")
                FieldHint("在 Google Programmable Search 创建并粘贴搜索引擎 ID")
            }

            if kind == .llm && !preset.models.isEmpty { modelPicker(editing, preset) }
            if kind == .llm && preset.models.isEmpty {
                FieldLabel(text: "模型 ID")
                SettingsField(text: bind(\.model), placeholder: "llama-3.3-70b").padding(.bottom, 14)
            }

            SettingsSection {
                SettingsRow(label: "启用", chevron: false) {
                    LXToggle(isOn: Binding(get: { editing.enabled },
                                           set: { v in store.update(kind, id: providerId) { $0.enabled = v } }))
                }
                SettingsRow(label: "设为默认", chevron: false, isLast: true,
                            onTap: { store.setDefault(kind, id: providerId) }) {
                    if editing.isDefault { LXIcon(name: .check, size: 16, color: t.accent, stroke: 2.2) }
                    else { Text("未启用").font(.system(size: 12)).foregroundColor(t.text4) }
                }
            }

            Button { store.remove(kind, id: providerId); host.pop() } label: {
                Text("移除此提供商").font(.system(size: 13.5, weight: .medium)).foregroundColor(t.danger)
                    .frame(maxWidth: .infinity).padding(12)
                    .overlay(RoundedRectangle(cornerRadius: 11).stroke(t.border, lineWidth: 0.5))
            }
            .padding(.top, 8)
        })
    }

    // bind a provider field as a Binding<String>
    private func bind(_ keyPath: WritableKeyPath<GenericProvider, String>) -> Binding<String> {
        Binding(
            get: { store.providers(kind).first(where: { $0.id == providerId })?[keyPath: keyPath] ?? "" },
            set: { v in store.update(kind, id: providerId) { $0[keyPath: keyPath] = v } })
    }

    private func statusBanner(_ p: GenericProvider) -> some View {
        let dot = p.status.dot(t)
        return HStack(spacing: 8) {
            Circle().fill(dot).frame(width: 8, height: 8)
                .overlay(Circle().stroke(dot.tint(0.22), lineWidth: 3).scaleEffect(1.75))
            Text(p.status.label).font(.system(size: 12.5, weight: .medium)).foregroundColor(t.text2)
            Spacer()
            Button {
                store.update(kind, id: providerId) { $0.status = .testing }
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.1) {
                    store.update(kind, id: providerId) { $0.status = .connected }
                }
            } label: {
                Text("测试连接").font(.system(size: 12, weight: .medium)).foregroundColor(t.text2)
                    .padding(.horizontal, 10).padding(.vertical, 6)
                    .background(t.windowBg).clipShape(RoundedRectangle(cornerRadius: 7))
                    .overlay(RoundedRectangle(cornerRadius: 7).stroke(t.border, lineWidth: 0.5))
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .background(dot.mix(with: t.surface, amount: 0.08))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(dot.tint(0.24), lineWidth: 0.5))
        .padding(.bottom, 18)
    }

    private func keyField(_ p: GenericProvider, _ preset: ProviderPreset) -> some View {
        ZStack(alignment: .trailing) {
            SettingsField(text: bind(\.key), placeholder: preset.keyPrefix + "...",
                          secure: !showKey, trailingPadding: 76)
            HStack(spacing: 2) {
                Button { showKey.toggle() } label: {
                    Text(showKey ? "隐藏" : "显示").font(.system(size: 11, weight: .medium)).foregroundColor(t.text3)
                        .padding(.horizontal, 7).padding(.vertical, 5)
                }
                Button {} label: { LXIcon(name: .copy, size: 13, color: t.text3, stroke: 1.8).padding(.horizontal, 5) }
            }
            .padding(.trailing, 6)
        }
    }

    private func modelPicker(_ p: GenericProvider, _ preset: ProviderPreset) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            FieldLabel(text: "默认模型")
            VStack(spacing: 0) {
                ForEach(Array(preset.models.enumerated()), id: \.element) { i, m in
                    let sel = p.model == m
                    Button { store.update(kind, id: providerId) { $0.model = m } } label: {
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

// MARK: - Engine credentials (SHIP-BLOCKER #1)

/// The in-process engine's actual API credentials, persisted in the iOS Keychain
/// (see `Keychain`). This is distinct from the visual "provider" cards above
/// (which are prototype/display state): THIS is the key the engine reads at
/// `EngineConfig.fromEnvironment` time to opt in and authenticate turns.
///
/// A non-empty key here is what flips the app from the mock conversation source
/// to the real engine on next launch. A blank key clears the Keychain item and
/// falls back to the mock. The base URL is optional (proxy / mirror); blank ⇒
/// the Anthropic default. Both load from / save to the Keychain — never to the
/// settings store, UserDefaults, or any log.
struct EngineCredentialsSection: View {
    @Environment(\.theme) private var t

    /// Local editing buffers, seeded from the Keychain on appear and flushed back
    /// on commit / disappear so a backgrounding mid-edit still persists.
    @State private var apiKey: String = ""
    @State private var apiBase: String = ""
    @State private var showKey = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            FieldLabel(text: "引擎 API Key")
            keyField()
            FieldHint("驱动设备端引擎的真实密钥 · 仅存于本机钥匙串(Keychain)，从不上传 · 填入后下次启动即启用真实引擎，留空则回退到演示模式")

            FieldLabel(text: "API 地址（可选）")
            SettingsField(text: $apiBase, placeholder: "https://api.anthropic.com")
                .onSubmit(persistBase)
            FieldHint("默认 `https://api.anthropic.com` · 可填代理 / 镜像端点")
        }
        .padding(.bottom, 22)
        .onAppear {
            apiKey = Keychain.get(.apiKey) ?? ""
            apiBase = Keychain.get(.apiBase) ?? ""
        }
        .onDisappear {
            persistKey()
            persistBase()
        }
    }

    private func keyField() -> some View {
        ZStack(alignment: .trailing) {
            SettingsField(text: $apiKey, placeholder: "sk-ant-...",
                          secure: !showKey, trailingPadding: 76)
                .onSubmit(persistKey)
            HStack(spacing: 2) {
                Button { showKey.toggle() } label: {
                    Text(showKey ? "隐藏" : "显示")
                        .font(.system(size: 11, weight: .medium)).foregroundColor(t.text3)
                        .padding(.horizontal, 7).padding(.vertical, 5)
                }
                Button { persistKey(); apiKey = ""; Keychain.clear(.apiKey) } label: {
                    Text("清除").font(.system(size: 11, weight: .medium)).foregroundColor(t.danger)
                        .padding(.horizontal, 7).padding(.vertical, 5)
                }
            }
            .padding(.trailing, 6)
        }
    }

    /// Persist the key buffer to the Keychain. A blank value clears the item.
    private func persistKey() { Keychain.set(.apiKey, apiKey) }
    /// Persist the base-URL buffer. A blank value clears the override.
    private func persistBase() { Keychain.set(.apiBase, apiBase) }
}
