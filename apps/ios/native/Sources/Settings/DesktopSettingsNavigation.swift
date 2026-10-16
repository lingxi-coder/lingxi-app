import SwiftUI

struct DesktopSettingsEntry: Identifiable {
    let id: String
    let titleKey: String
    var title: String { String(localized: String.LocalizationValue(titleKey)) }
    let group: String
    let page: SettingsPage
    var availableOnMobile: Bool { id != "fusion" && id != "general" }
    var layered = false
    var keys: [String] = []
    /// Where the phone settings home lists this page; nil = reachable through the
    /// account card or search only.
    var mobileGroup: SettingsGroup? = nil
    /// Overrides `settings.page.<id>` for the identifiers UI tests already use.
    var accessibilityID: String { id == "provider-credentials" ? "settings.provider.llm" : id == "app-integration" ? "settings.appIntegration" : "settings.page.\(id)" }
    var icon: LXIconName {
        switch id {
        case "account", "provider-credentials": .pin
        case "appearance": .sun
        case "voice": .mic
        case "language": .globe
        case "web-search", "web-fetch": .search
        case "linux-runtime": .terminal
        case "typescript-lsp": .wrench
        case "app-integration": .link
        case "dream": .dream
        case "notifications": .message
        case "input": .pencil
        case "privacy": .check
        case "permission-mode": .check
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
        .init(id: "appearance", titleKey: "settings_appearance", group: "个人", page: .appearance, keys: ["appearance", "theme", "dark", "light"], mobileGroup: .general),
        .init(id: "voice", titleKey: "settings_parity_voice", group: "个人", page: .voice, keys: ["voice", "tts", "stt"], mobileGroup: .voice),
        .init(id: "archived-chats", titleKey: "settings_parity_archived", group: "个人", page: .archivedChats, keys: ["archived", "restore"], mobileGroup: .data),
        .init(id: "projects", titleKey: "settings_parity_projects_trust", group: "个人", page: .projectsTrust, keys: ["project", "trustedDirectories"], mobileGroup: .data),
        .init(id: "provider-credentials", titleKey: "settings_parity_credentials", group: "模型与服务", page: .providerList(.init(.llm)), keys: ["credential", "API key", "keychain"], mobileGroup: .models),
        .init(id: "custom-providers", titleKey: "settings_parity_custom_providers", group: "模型与服务", page: .customProviders, layered: true, keys: ["providers", "routing", "models", "aliases", "fallback", "retry"], mobileGroup: .models),
        .init(id: "fusion", titleKey: "settings_parity_fusion", group: "模型与服务", page: .fusion, layered: true, keys: ["fusion", "panelModels", "analystModel"], mobileGroup: .models),
        .init(id: "permissions", titleKey: "settings_parity_permissions", group: "编码", page: .permissions, layered: true, keys: ["permissions", "allow", "deny", "ask"], mobileGroup: .modes),
        .init(id: "tools-agent", titleKey: "settings_parity_tools_agent", group: "编码", page: .toolsAgent, layered: true, keys: ["promptSuggestionEnabled", "enabledTools", "outputStyle", "modelOverrides", "alwaysThinkingEnabled", "showThinkingSummaries", "visionDelegationEnabled", "disableAllHooks", "skipWebFetchPreflight"], mobileGroup: .modes),
        .init(id: "skills", titleKey: "settings_title_skills", group: "编码", page: .skills, layered: true, keys: ["skills", "syncClaudeAiSkills", "reload"], mobileGroup: .modes),
        .init(id: "mcp", titleKey: "settings_mcp_servers", group: "编码", page: .mcpList, keys: ["mcp", "server"], mobileGroup: .modes),
        .init(id: "hooks", titleKey: "settings_parity_hooks", group: "编码", page: .hooks, layered: true, keys: ["hooks", "ConfigChange", "PreToolUse", "PostToolUse"], mobileGroup: .modes),
        .init(id: "plugins", titleKey: "settings_parity_plugins", group: "编码", page: .plugins, layered: true, keys: ["enabledPlugins", "pluginConfigs", "marketplace"], mobileGroup: .modes),
        .init(id: "language", titleKey: "settings_language_title", group: "个人", page: .language, keys: ["language", "locale"], mobileGroup: .general),
        .init(id: "notifications", titleKey: "settings_notifications", group: "个人", page: .notifications, keys: ["notifications", "background"], mobileGroup: .general),
        .init(id: "input", titleKey: "settings_keyboard_input", group: "个人", page: .input, keys: ["input", "keyboard"], mobileGroup: .general),
        .init(id: "web-search", titleKey: "settings_web_search", group: "模型与服务", page: .providerList(.init(.search)), keys: ["search", "web"], mobileGroup: .models),
        .init(id: "web-fetch", titleKey: "settings_web_fetch", group: "模型与服务", page: .providerList(.init(.fetch)), keys: ["fetch", "web"], mobileGroup: .models),
        .init(id: "permission-mode", titleKey: "settings_permission_mode_title", group: "编码", page: .permissionMode, keys: ["permission mode", "ask", "plan", "bypass"], mobileGroup: .modes),
        .init(id: "dream", titleKey: "settings_dream_mode", group: "编码", page: .dream, keys: ["dream", "memory"], mobileGroup: .modes),
        .init(id: "linux-runtime", titleKey: "settings_linux_runtime", group: "高级", page: .linuxRuntime, keys: ["linux", "runtime", "terminal", "shell"], mobileGroup: .phone),
        .init(id: "typescript-lsp", titleKey: "settings_typescript_lsp_title", group: "高级", page: .typescriptLsp, keys: ["typescript", "lsp"], mobileGroup: .phone),
        .init(id: "app-integration", titleKey: "settings_app_integration", group: "高级", page: .appIntegration, keys: ["integration", "shortcuts", "siri"], mobileGroup: .phone),
        .init(id: "privacy", titleKey: "settings_data_privacy", group: "高级", page: .privacy, keys: ["privacy", "data"], mobileGroup: .data),
        .init(id: "diagnostics", titleKey: "settings_parity_diagnostics", group: "高级", page: .diagnostics, keys: ["diagnostics", "log", "json", "settings.json"], mobileGroup: .data),
        .init(id: "about", titleKey: "settings_section_about", group: "高级", page: .about, keys: ["about", "version"], mobileGroup: .about),
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
    @Bindable var store: SettingsStore
    let host: SettingsHost
    @State private var query = ""
    @State private var repository = DesktopSettingsRepository.shared
    @State private var secrets = PluginSecretRepository.shared

    private var searching: Bool { !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var body: some View {
        List {
            if searching {
                let matches = DesktopSettingsEntry.search(query)
                if matches.isEmpty {
                    ContentUnavailableView.search(text: query)
                        .listRowBackground(Color.clear)
                } else {
                    Section {
                        ForEach(matches) { entry in
                            SettingsNavigationRow(title: entry.title, symbol: entry.systemImage,
                                                  color: entry.mobileGroup?.symbolColor ?? .gray) {
                                host.push(entry.page)
                            }
                            .accessibilityIdentifier(entry.accessibilityID)
                        }
                    }
                }
            } else {
                Section {
                    SettingsAccountCard { host.push(.account) }
                }
                if repository.connected && (repository.needsReconnect || secrets.needsReconnect) {
                    Section {
                        SettingsApplyChangesBanner(reconnect: host.onReconnectAfterSecretChange)
                    }
                }
                Section {
                    ForEach(SettingsGroup.allCases) { group in
                        SettingsNavigationRow(title: group.title, symbol: group.systemImage,
                                              color: group.symbolColor) {
                            host.push(group.directPage ?? .group(group))
                        }
                        .accessibilityIdentifier("settings.group.\(group.rawValue)")
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always),
                    prompt: Text("settings_parity_search"))
        .scrollDismissesKeyboard(.interactively)
    }
}

struct SettingsGroupPage: View {
    let group: SettingsGroup
    let host: SettingsHost

    private var entries: [DesktopSettingsEntry] {
        DesktopSettingsEntry.all.filter { $0.availableOnMobile && $0.mobileGroup == group }
    }

    var body: some View {
        List {
            Section {
                ForEach(entries) { entry in
                    SettingsNavigationRow(title: entry.title, symbol: entry.systemImage, color: group.symbolColor) {
                        host.push(entry.page)
                    }
                    .accessibilityIdentifier(entry.accessibilityID)
                }
            } footer: {
                Text(group.summary)
            }
        }
        .listStyle(.insetGrouped)
    }
}

private struct SettingsNavigationRow: View {
    let title: String
    let symbol: String
    let color: Color
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Image(systemName: symbol)
                    .font(.body.weight(.medium))
                    .foregroundStyle(.white)
                    .frame(width: 29, height: 29)
                    .background(color, in: RoundedRectangle(cornerRadius: 7))
                    .accessibilityHidden(true)
                Text(title).foregroundStyle(Color.primary).frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                Image(systemName: "chevron.right")
                    .font(.footnote.weight(.semibold)).foregroundStyle(Color(uiColor: .tertiaryLabel))
                    .accessibilityHidden(true)
            }
            .frame(minHeight: 32)
            .padding(.vertical, 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.automatic)
    }
}

private struct SettingsAccountCard: View {
    let onOpen: () -> Void

    var body: some View {
        let user = FirebaseAccount.shared.user
        Button(action: onOpen) {
            HStack(spacing: 14) {
                Image(systemName: "person.crop.circle.fill")
                    .font(.system(size: 52)).foregroundStyle(Color.secondary)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 4) {
                    Text(user?.displayName ?? user?.email ?? String(localized: "account_sign_in_title"))
                        .font(.headline).foregroundStyle(Color.primary)
                        .fixedSize(horizontal: false, vertical: true)
                    Text(user?.email ?? String(localized: "account_sign_in_subtitle"))
                        .font(.subheadline).foregroundStyle(Color.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Image(systemName: "chevron.right")
                    .font(.footnote.weight(.semibold)).foregroundStyle(Color(uiColor: .tertiaryLabel))
                    .accessibilityHidden(true)
            }
            .padding(.vertical, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.automatic)
        .accessibilityIdentifier("settings.page.account")
    }
}

private extension SettingsGroup {
    var systemImage: String {
        switch self {
        case .models: "sparkles"
        case .voice: "waveform"
        case .modes: "slider.horizontal.3"
        case .phone: "iphone"
        case .general: "gearshape.fill"
        case .data: "folder.fill"
        case .about: "info.circle.fill"
        }
    }
    var symbolColor: Color {
        switch self {
        case .models: .indigo
        case .voice: .pink
        case .modes: .orange
        case .phone: .green
        case .general: .gray
        case .data: .blue
        case .about: .gray
        }
    }
}

private extension DesktopSettingsEntry {
    var systemImage: String {
        switch id {
        case "account": "person.crop.circle"
        case "appearance": "paintpalette.fill"
        case "voice": "waveform"
        case "language": "globe"
        case "notifications": "bell.fill"
        case "input": "keyboard"
        case "provider-credentials": "key.fill"
        case "custom-providers", "fusion": "sparkles"
        case "web-search": "magnifyingglass"
        case "web-fetch": "globe"
        case "permissions", "permission-mode": "checkmark.shield.fill"
        case "tools-agent": "wrench.and.screwdriver.fill"
        case "skills": "square.stack.3d.up.fill"
        case "hooks": "arrow.triangle.branch"
        case "plugins", "mcp": "puzzlepiece.extension.fill"
        case "linux-runtime": "terminal.fill"
        case "typescript-lsp": "curlybraces"
        case "app-integration": "link"
        case "dream": "moon.stars.fill"
        case "privacy": "hand.raised.fill"
        case "diagnostics": "stethoscope"
        case "archived-chats": "archivebox.fill"
        case "projects": "folder.fill"
        case "about": "info.circle.fill"
        default: "gearshape.fill"
        }
    }
}
