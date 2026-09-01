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
        case .language:                   LanguagePage()
        case .notifications:              NotificationsPage(store: store)
        case .input:                      InputPage()
        case .appIntegration:             AppIntegrationPage()
        case .privacy:                    PrivacyPage()
        case .permissionMode:             PermissionModePage(store: store, host: host)
        case .typescriptLsp:              TypeScriptLspModePage(store: store, host: host)
        case .skills:                     SkillsPage(store: store, host: host)
        case .skillDetail(let id):        SkillDetailPage(store: store, host: host, skillId: id)
        case .localAppPlugin:             LocalAppPluginPage(host: host)
        case .mcpList:                    MCPListPage(store: store, host: host)
        case .mcpEdit(let id):            MCPEditPage(store: store, host: host, mcpId: id)
        case .dream:                      DreamPage(store: store)
        }
    }
}

// MARK: - Main settings list
struct MainSettingsPage: View {
    @Environment(AppState.self) private var app
    @Environment(VoiceCapabilityModel.self) private var voiceCapability
    @Environment(LocalizationManager.self) private var localization
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    let host: SettingsHost
    @State private var providerRepository = ProviderRepository.shared

    var body: some View {
        VStack(spacing: 0) {
            accountCard

            SettingsSection(label: String(localized: "settings_section_intelligence")) {
                let llmSummary = providerRepository.settingsSummary
                let ds = store.searchProviders.first(where: { $0.isDefault }) ?? store.searchProviders.first
                let df = store.fetchProviders.first(where: { $0.isDefault }) ?? store.fetchProviders.first
                SettingsRow(icon: .sparkle, iconColor: Accents.color(for: "oklch(70% 0.18 268)"), label: String(localized: "settings_llm_providers"),
                            sub: String(localized: "settings_provider_default \(llmSummary.defaultProfile?.name ?? String(localized: "settings_provider_unconfigured"))"),
                            value: String(localized: "settings_providers_enabled_count \(llmSummary.enabledCount)"),
                            onTap: { host.push(.providerList(.init(.llm))) })
                    .accessibilityIdentifier("settings.provider.llm")
                SettingsRow(icon: .search, iconColor: Color(srgb: 0,0.7151,0.7672), label: String(localized: "settings_web_search"),
                            sub: ds != nil ? String(localized: "settings_provider_default \(ds!.name)") : String(localized: "settings_provider_unconfigured"),
                            value: String(localized: "settings_providers_enabled_count \(store.searchProviders.filter{$0.enabled}.count)"),
                            onTap: { host.push(.providerList(.init(.search))) })
                SettingsRow(icon: .link, iconColor: Color(srgb: 0.8713,0.58,0), label: String(localized: "settings_web_fetch"),
                            sub: df != nil ? String(localized: "settings_provider_default \(df!.name)") : String(localized: "settings_provider_unconfigured"),
                            value: String(localized: "settings_providers_enabled_count \(store.fetchProviders.filter{$0.enabled}.count)"),
                            onTap: { host.push(.providerList(.init(.fetch))) })
                SettingsRow(icon: .mic, iconColor: Color(srgb: 0.8018,0.4038,0.8909), label: String(localized: "settings_voice_tts"),
                            sub: voiceCapability.voiceSummaryLabel,
                            value: voiceCapability.configurationReadiness.isReadyForFlow
                                ? String(localized: "settings_status_available")
                                : String(localized: "settings_status_needs_check"),
                            isLast: true,
                            onTap: { host.push(.voice) })
            }

            SettingsSection(label: String(localized: "settings_section_capabilities"),
                            footer: String(localized: "settings_section_capabilities_footer")) {
                SettingsRow(icon: .skill, iconColor: Color(srgb: 0,0.7601,0.7664), label: "Skills",
                            sub: String(localized: "settings_skills_sub"),
                            value: store.skillsLoaded
                                ? String(localized: "settings_skills_enabled_fraction \(store.skills.filter{$0.enabled}.count) \(store.skills.count)")
                                : "—",
                            onTap: { host.push(.skills) })
                SettingsRow(icon: .plug, iconColor: Color(srgb: 0,0.78,0.55), label: String(localized: "settings_mcp_servers"),
                            sub: "Model Context Protocol",
                            value: String(localized: "settings_mcp_connections_count \(store.mcpServers.filter{$0.enabled}.count)"),
                            onTap: { host.push(.mcpList) })
                SettingsRow(icon: .workflow, iconColor: Color(srgb: 0.3503,0.6649,0.9741), label: String(localized: "settings_linux_runtime"),
                            sub: store.linuxRuntime.summary, value: store.linuxRuntime.badge,
                            onTap: { host.push(.linuxRuntime) })
                SettingsRow(icon: .dream, iconColor: Color(srgb: 0.809,0.4552,0.8891), label: String(localized: "settings_dream_mode"),
                            sub: String(localized: "settings_dream_sub"),
                            value: store.dream.enabled ? String(localized: "settings_status_on") : String(localized: "settings_status_off"),
                            isLast: true,
                            onTap: { host.push(.dream) })
            }

            SettingsSection(label: String(localized: "settings_section_memory_knowledge")) {
                SettingsRow(icon: .book, iconColor: Color(srgb: 0,0.7601,0.7664), label: String(localized: "settings_knowledge"),
                            value: String(localized: "settings_knowledge_items"),
                            onTap: { host.push(.knowledge) })
                SettingsRow(icon: .brain, iconColor: Color(srgb: 0.809,0.4552,0.8891), label: String(localized: "settings_memory"),
                            sub: String(localized: "settings_memory_sub"),
                            value: String(localized: "settings_memory_items"),
                            onTap: { host.push(.memory) })
                SettingsRow(icon: .workflow, iconColor: Color(srgb: 0,0.78,0.55), label: String(localized: "settings_workflows_automation"),
                            value: String(localized: "settings_workflows_enabled"),
                            isLast: true, onTap: { host.push(.workflows) })
            }

            SettingsSection(label: String(localized: "settings_section_app")) {
                SettingsRow(icon: .sun, iconColor: Color(srgb: 0.896,0.6013,0), label: String(localized: "settings_appearance"),
                            value: app.isDark ? String(localized: "settings_appearance_dark") : String(localized: "settings_appearance_light"),
                            onTap: { host.push(.appearance) })
                SettingsRow(icon: .message, iconColor: Color(srgb: 0.3503,0.6649,0.9741), label: String(localized: "settings_language_title"),
                            value: LocalizationManager.label(for: localization.language), onTap: { host.push(.language) })
                SettingsRow(icon: .cog, iconColor: t.text3, label: String(localized: "settings_notifications"),
                            value: String(localized: "settings_notifs_enabled_count \(store.notifs.enabledCount)"),
                            onTap: { host.push(.notifications) })
                SettingsRow(icon: .workflow, iconColor: t.accent, label: String(localized: "settings_app_integration"),
                            sub: String(localized: "settings_app_integration_sub"),
                            value: String(localized: "settings_app_integration_actions"),
                            onTap: { host.push(.appIntegration) })
                    .accessibilityIdentifier("settings.appIntegration")
                SettingsRow(icon: .paperclip, iconColor: Color(srgb: 0.9351,0.5079,0.4015), label: String(localized: "settings_keyboard_input"),
                            sub: String(localized: "settings_keyboard_input_sub"),
                            isLast: true, onTap: { host.push(.input) })
            }

            SettingsSection(label: String(localized: "settings_section_privacy_security")) {
                SettingsRow(icon: .pin, iconColor: t.ok, label: String(localized: "settings_bio_lock"), chevron: false) { LXToggle(isOn: $store.bioLock) }
                SettingsRow(icon: .brain, iconColor: t.text3, label: String(localized: "settings_data_privacy"),
                            sub: String(localized: "settings_data_privacy_sub"),
                            onTap: { host.push(.privacy) })
                SettingsRow(icon: .check, iconColor: t.accent, label: "权限模式",
                            sub: "控制工具何时需要确认", value: store.permissionMode == store.effectivePermissionMode
                                ? store.permissionMode
                                : "\(store.permissionMode) → \(store.effectivePermissionMode)",
                            onTap: { host.push(.permissionMode) })
                SettingsRow(icon: .cog, iconColor: Color(srgb: 0.3503,0.6649,0.9741), label: "TypeScript LSP",
                            sub: "JavaScript / JSX 实时代码诊断",
                            value: store.typescriptLspMode == store.effectiveTypescriptLspMode
                                ? store.typescriptLspMode
                                : "\(store.typescriptLspMode) → \(store.effectiveTypescriptLspMode)",
                            onTap: { host.push(.typescriptLsp) })
                SettingsRow(icon: .sparkle, iconColor: t.text3, label: String(localized: "settings_usage_diagnostics"), chevron: false) { LXToggle(isOn: $store.telemetry) }
                SettingsRow(icon: .check, iconColor: t.text3, label: String(localized: "settings_auto_update"), chevron: false, isLast: true) { LXToggle(isOn: $store.autoUpdate) }
            }

            SettingsSection(label: String(localized: "settings_section_about")) {
                SettingsRow(icon: .sparkle, label: String(localized: "app_name"), value: "2.4.1 (build 8721)", chevron: false)
                SettingsRow(icon: .play, label: String(localized: "settings_rewatch_onboarding"),
                            sub: String(localized: "settings_rewatch_onboarding_sub"),
                            onTap: {
                    app.setupDone = false
                    host.onClose()
                })
                SettingsRow(icon: .book, label: String(localized: "settings_help_center"), onTap: {})
                SettingsRow(icon: .message, label: String(localized: "settings_feedback"), onTap: {})
                SettingsRow(icon: .link, label: String(localized: "settings_open_source"), isLast: true, onTap: {})
            }

            Text("settings_copyright")
                .font(.system(size: 11)).foregroundColor(t.text4)
                .multilineTextAlignment(.center).lineSpacing(5)
                .frame(maxWidth: .infinity).padding(.top, 8).padding(.bottom, 4)
        }
        .task { voiceCapability.reloadFromDefaults() }
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
                Text("settings_account").font(.system(size: 12, weight: .medium)).foregroundColor(t.text2)
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
