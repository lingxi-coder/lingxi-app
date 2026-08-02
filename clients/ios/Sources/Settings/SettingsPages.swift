import SwiftUI

// MARK: - Page dispatcher
struct SettingsPages: View {
    @Bindable var store: SettingsStore
    let host: SettingsHost
    let page: SettingsPage

    var body: some View {
        switch page {
        case .main:                       MainSettingsPage(store: store, host: host)
        case .account:                    AccountPage()
        case .providerList(let b):        ProviderListPage(store: store, host: host, kind: b.kind)
        case .providerPicker(let b):      ProviderPickerPage(store: store, host: host, kind: b.kind)
        case .providerEdit(let b, let id): ProviderEditPage(store: store, host: host, kind: b.kind, providerId: id)
        case .voice:                      VoicePage(store: store)
        case .linuxRuntime:               LinuxRuntimePage(store: store, onOpenTerminal: host.openTerminal)
        case .knowledge:                  KnowledgePage()
        case .memory:                     MemoryPage()
        case .workflows:                  WorkflowsPage()
        case .appearance:                 AppearancePage()
        case .language:                   LanguagePage(store: store)
        case .notifications:              NotificationsPage(store: store)
        case .input:                      InputPage()
        case .appIntegration:             AppIntegrationPage()
        case .privacy:                    PrivacyPage()
        case .skills:                     SkillsPage(store: store, host: host)
        case .skillDetail(let id):        SkillDetailPage(store: store, host: host, skillId: id)
        case .mcpList:                    MCPListPage(store: store, host: host)
        case .mcpEdit(let id):            MCPEditPage(store: store, host: host, mcpId: id)
        case .dream:                      DreamPage(store: store)
        }
    }
}

// MARK: - Main settings list
struct MainSettingsPage: View {
    @Environment(AppState.self) private var app
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    let host: SettingsHost

    private let langMap = ["zh-CN": "简体中文", "zh-TW": "繁體中文", "en-US": "English", "ja-JP": "日本語"]

    var body: some View {
        VStack(spacing: 0) {
            accountCard

            SettingsSection(label: "智能") {
                let dl = store.llmProviders.first(where: { $0.isDefault }) ?? store.llmProviders.first
                let ds = store.searchProviders.first(where: { $0.isDefault }) ?? store.searchProviders.first
                let df = store.fetchProviders.first(where: { $0.isDefault }) ?? store.fetchProviders.first
                SettingsRow(icon: .sparkle, iconColor: Accents.color(for: "oklch(70% 0.18 268)"), label: "LLM 提供商",
                            sub: "默认: \(dl?.name ?? "未配置")", value: "\(store.llmProviders.filter{$0.enabled}.count) 个启用",
                            onTap: { host.push(.providerList(.init(.llm))) })
                    .accessibilityIdentifier("settings.provider.llm")
                SettingsRow(icon: .search, iconColor: Color(srgb: 0,0.7151,0.7672), label: "联网搜索",
                            sub: ds != nil ? "默认: \(ds!.name)" : "未配置", value: "\(store.searchProviders.filter{$0.enabled}.count) 个启用",
                            onTap: { host.push(.providerList(.init(.search))) })
                SettingsRow(icon: .link, iconColor: Color(srgb: 0.8713,0.58,0), label: "网页抓取",
                            sub: df != nil ? "默认: \(df!.name)" : "未配置", value: "\(store.fetchProviders.filter{$0.enabled}.count) 个启用",
                            onTap: { host.push(.providerList(.init(.fetch))) })
                SettingsRow(icon: .mic, iconColor: Color(srgb: 0.8018,0.4038,0.8909), label: "语音 TTS",
                            sub: Presets.voice.first(where: { $0.id == store.voice.preset })?.name,
                            value: store.voice.preset == "system" ? "免费" : "已配置", isLast: true,
                            onTap: { host.push(.voice) })
            }

            SettingsSection(label: "能力扩展",
                            footer: "Skills 是可复用的 AI 行为包；MCP 是接入外部工具的标准协议；Dream 让灵犀在你休息时主动整理与规划。") {
                SettingsRow(icon: .skill, iconColor: Color(srgb: 0,0.7601,0.7664), label: "Skills",
                            sub: "技能包 · 提示词 · 操作流", value: "\(store.skills.filter{$0.enabled}.count) / \(store.skills.count) 启用",
                            onTap: { host.push(.skills) })
                SettingsRow(icon: .plug, iconColor: Color(srgb: 0,0.78,0.55), label: "MCP 服务器",
                            sub: "Model Context Protocol", value: "\(store.mcpServers.filter{$0.enabled}.count) 连接",
                            onTap: { host.push(.mcpList) })
                SettingsRow(icon: .workflow, iconColor: Color(srgb: 0.3503,0.6649,0.9741), label: "Linux 运行时",
                            sub: store.linuxRuntime.summary, value: store.linuxRuntime.badge,
                            onTap: { host.push(.linuxRuntime) })
                SettingsRow(icon: .dream, iconColor: Color(srgb: 0.809,0.4552,0.8891), label: "Dream 模式",
                            sub: "后台离线思考与整理", value: store.dream.enabled ? "开启" : "关闭", isLast: true,
                            onTap: { host.push(.dream) })
            }

            SettingsSection(label: "记忆与知识") {
                SettingsRow(icon: .book, iconColor: Color(srgb: 0,0.7601,0.7664), label: "知识库", value: "24 项",
                            onTap: { host.push(.knowledge) })
                SettingsRow(icon: .brain, iconColor: Color(srgb: 0.809,0.4552,0.8891), label: "记忆",
                            sub: "灵犀记住的关于你的事实", value: "42 条", onTap: { host.push(.memory) })
                SettingsRow(icon: .workflow, iconColor: Color(srgb: 0,0.78,0.55), label: "工作流与自动化",
                            value: "3 启用", isLast: true, onTap: { host.push(.workflows) })
            }

            SettingsSection(label: "应用") {
                SettingsRow(icon: .sun, iconColor: Color(srgb: 0.896,0.6013,0), label: "外观",
                            value: app.isDark ? "深色" : "浅色", onTap: { host.push(.appearance) })
                SettingsRow(icon: .message, iconColor: Color(srgb: 0.3503,0.6649,0.9741), label: "语言",
                            value: langMap[store.language], onTap: { host.push(.language) })
                SettingsRow(icon: .cog, iconColor: t.text3, label: "通知",
                            value: "\(store.notifs.enabledCount) 项开启", onTap: { host.push(.notifications) })
                SettingsRow(icon: .workflow, iconColor: t.accent, label: "应用接入",
                            sub: "Siri · 快捷指令 · 自动化", value: "3 个动作",
                            onTap: { host.push(.appIntegration) })
                    .accessibilityIdentifier("settings.appIntegration")
                SettingsRow(icon: .paperclip, iconColor: Color(srgb: 0.9351,0.5079,0.4015), label: "键盘与输入",
                            sub: "语音输入 · 候选词", isLast: true, onTap: { host.push(.input) })
            }

            SettingsSection(label: "隐私与安全") {
                SettingsRow(icon: .pin, iconColor: t.ok, label: "生物识别锁", chevron: false) { LXToggle(isOn: $store.bioLock) }
                SettingsRow(icon: .brain, iconColor: t.text3, label: "数据与隐私",
                            sub: "导出 · 删除 · 透明度报告", onTap: { host.push(.privacy) })
                SettingsRow(icon: .sparkle, iconColor: t.text3, label: "使用诊断", chevron: false) { LXToggle(isOn: $store.telemetry) }
                SettingsRow(icon: .check, iconColor: t.text3, label: "自动更新", chevron: false, isLast: true) { LXToggle(isOn: $store.autoUpdate) }
            }

            SettingsSection(label: "关于") {
                SettingsRow(icon: .sparkle, label: "灵犀", value: "2.4.1 (build 8721)", chevron: false)
                SettingsRow(icon: .play, label: "重新观看引导", sub: "再过一遍首次设置向导", onTap: {
                    app.setupDone = false
                    host.onClose()
                })
                SettingsRow(icon: .book, label: "帮助中心", onTap: {})
                SettingsRow(icon: .message, label: "反馈与建议", onTap: {})
                SettingsRow(icon: .link, label: "开源许可", isLast: true, onTap: {})
            }

            Text("© 2026 灵犀 AI · 用户偏好仅在本地")
                .font(.system(size: 11)).foregroundColor(t.text4)
                .multilineTextAlignment(.center).lineSpacing(5)
                .frame(maxWidth: .infinity).padding(.top, 8).padding(.bottom, 4)
        }
    }

    private var accountCard: some View {
        HStack(spacing: 12) {
            Circle().fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
                .frame(width: 46, height: 46)
                .overlay(Text("Y").font(.system(size: 17, weight: .semibold)).foregroundColor(.white))
            VStack(alignment: .leading, spacing: 2) {
                Text("Yuxin Yang").font(.system(size: 15.5, weight: .semibold)).foregroundColor(t.text)
                Text("yuxin@axielix.com · Pro").font(.system(size: 12)).foregroundColor(t.text4)
            }
            Spacer()
            Button { host.push(.account) } label: {
                Text("账户").font(.system(size: 12, weight: .medium)).foregroundColor(t.text2)
                    .padding(.horizontal, 11).padding(.vertical, 6)
                    .background(t.windowBg).clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).stroke(t.border, lineWidth: 0.5))
            }
        }
        .padding(14)
        .background(t.surface)
        .clipShape(RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(t.border, lineWidth: 0.5))
        .padding(.bottom, 22)
    }
}
