import SwiftUI

struct DesktopSettingsEntry: Identifiable {
    let id: String
    let titleKey: String
    var title: String { String(localized: String.LocalizationValue(titleKey)) }
    let group: String
    let page: SettingsPage
    var availableOnMobile: Bool { id != "fusion" }
    var layered = false
    var keys: [String] = []
    var icon: LXIconName {
        switch id {
        case "account", "provider-credentials": .pin
        case "appearance": .sun
        case "voice": .mic
        case "archived-chats": .clock
        case "projects": .folder
        case "custom-providers", "mcp", "plugins": .plug
        case "fusion": .sparkle
        case "permissions": .check
        case "tools-agent": .wrench
        case "skills": .skill
        case "hooks": .workflow
        case "diagnostics": .listChecks
        case "about": .book
        default: .cog
        }
    }
    static let groups = ["个人", "模型与服务", "编码", "高级"]
    static let all: [Self] = [
        .init(id: "general", titleKey: "settings_parity_general", group: "个人", page: .general, keys: ["general", "language", "notifications", "input"]),
        .init(id: "account", titleKey: "settings_title_account", group: "个人", page: .account, keys: ["account", "login", "auth"]),
        .init(id: "appearance", titleKey: "settings_appearance", group: "个人", page: .appearance, keys: ["appearance", "theme", "dark", "light"]),
        .init(id: "voice", titleKey: "settings_parity_voice", group: "个人", page: .voice, keys: ["voice", "tts", "stt"]),
        .init(id: "archived-chats", titleKey: "settings_parity_archived", group: "个人", page: .archivedChats, keys: ["archived", "restore"]),
        .init(id: "projects", titleKey: "settings_parity_projects_trust", group: "个人", page: .projectsTrust, keys: ["project", "trustedDirectories"]),
        .init(id: "provider-credentials", titleKey: "settings_parity_credentials", group: "模型与服务", page: .providerList(.init(.llm)), keys: ["credential", "API key", "keychain"]),
        .init(id: "custom-providers", titleKey: "settings_parity_custom_providers", group: "模型与服务", page: .customProviders, layered: true, keys: ["providers", "routing", "models", "aliases", "fallback", "retry"]),
        .init(id: "fusion", titleKey: "settings_parity_fusion", group: "模型与服务", page: .fusion, layered: true, keys: ["fusion", "panelModels", "analystModel"]),
        .init(id: "permissions", titleKey: "settings_parity_permissions", group: "编码", page: .permissions, layered: true, keys: ["permissions", "allow", "deny", "ask"]),
        .init(id: "tools-agent", titleKey: "settings_parity_tools_agent", group: "编码", page: .toolsAgent, layered: true, keys: ["enabledTools", "outputStyle", "modelOverrides", "alwaysThinkingEnabled", "showThinkingSummaries", "visionDelegationEnabled", "disableAllHooks", "skipWebFetchPreflight"]),
        .init(id: "skills", titleKey: "settings_title_skills", group: "编码", page: .skills, layered: true, keys: ["skills", "syncClaudeAiSkills", "reload"]),
        .init(id: "mcp", titleKey: "settings_mcp_servers", group: "编码", page: .mcpList, keys: ["mcp", "server"]),
        .init(id: "hooks", titleKey: "settings_parity_hooks", group: "编码", page: .hooks, layered: true, keys: ["hooks", "ConfigChange", "PreToolUse", "PostToolUse"]),
        .init(id: "plugins", titleKey: "settings_parity_plugins", group: "编码", page: .plugins, layered: true, keys: ["enabledPlugins", "pluginConfigs", "marketplace"]),
        .init(id: "diagnostics", titleKey: "settings_parity_diagnostics", group: "高级", page: .diagnostics, keys: ["diagnostics", "log", "json", "settings.json"]),
        .init(id: "about", titleKey: "settings_section_about", group: "高级", page: .about, keys: ["about", "version"]),
    ]
    static func search(_ query: String) -> [Self] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return all.filter(\.availableOnMobile) }
        return all.filter(\.availableOnMobile).filter { entry in
            ([entry.title, entry.id] + entry.keys).contains { $0.localizedStandardContains(query) }
        }
    }
}

struct MainSettingsPage: View {
    @Environment(\.theme) private var theme
    @Bindable var store: SettingsStore
    let host: SettingsHost
    @State private var query = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            TextField("settings_parity_search", text: $query)
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("settings.search")
            let matches = DesktopSettingsEntry.search(query)
            ForEach(DesktopSettingsEntry.groups, id: \.self) { group in
                let entries = matches.filter { $0.group == group }
                if !entries.isEmpty {
                    SettingsSection(label: String(localized: String.LocalizationValue(["个人": "settings_parity_personal", "模型与服务": "settings_parity_models_services", "编码": "settings_parity_coding", "高级": "settings_section_advanced"][group] ?? group))) {
                        ForEach(entries) { entry in
                            SettingsRow(icon: entry.icon, label: entry.title, isLast: entry.id == entries.last?.id,
                                        onTap: { host.push(entry.page) })
                                .accessibilityIdentifier(entry.id == "provider-credentials" ? "settings.provider.llm" : "settings.page.\(entry.id)")
                        }
                    }
                }
            }
            if matches.isEmpty { ContentUnavailableView.search(text: query) }
        }
    }
}
