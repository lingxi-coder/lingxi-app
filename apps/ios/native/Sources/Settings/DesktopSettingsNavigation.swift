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
    @Environment(\.theme) private var theme
    @Bindable var store: SettingsStore
    let host: SettingsHost
    @State private var query = ""

    private var searching: Bool {
        !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            TextField("settings_parity_search", text: $query)
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("settings.search")
            if searching {
                results
            } else {
                SettingsAccountCard { host.push(.account) }
                SettingsSection {
                    ForEach(SettingsGroup.allCases) { group in
                        SettingsRow(
                            icon: group.icon,
                            label: group.title,
                            sub: group.summary,
                            isLast: group == SettingsGroup.allCases.last,
                            onTap: { host.push(group.directPage ?? .group(group)) }
                        )
                        .accessibilityIdentifier("settings.group.\(group.rawValue)")
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var results: some View {
        let matches = DesktopSettingsEntry.search(query)
        if matches.isEmpty {
            ContentUnavailableView.search(text: query)
        } else {
            SettingsSection {
                ForEach(matches) { entry in
                    SettingsRow(
                        icon: entry.icon,
                        label: entry.title,
                        sub: entry.mobileGroup?.title,
                        isLast: entry.id == matches.last?.id,
                        onTap: { host.push(entry.page) }
                    )
                    .accessibilityIdentifier(entry.accessibilityID)
                }
            }
        }
    }
}

/// One group's pages, each opening the existing settings page unchanged.
struct SettingsGroupPage: View {
    let group: SettingsGroup
    let host: SettingsHost

    private var entries: [DesktopSettingsEntry] {
        DesktopSettingsEntry.all.filter { $0.availableOnMobile && $0.mobileGroup == group }
    }

    var body: some View {
        SettingsSection {
            ForEach(entries) { entry in
                SettingsRow(
                    icon: entry.icon,
                    label: entry.title,
                    isLast: entry.id == entries.last?.id,
                    onTap: { host.push(entry.page) }
                )
                .accessibilityIdentifier(entry.accessibilityID)
            }
        }
        .accessibilityIdentifier("settings.group.page.\(group.rawValue)")
    }
}

/// The signed-in profile (or a sign-in prompt) — tapping opens the account page.
private struct SettingsAccountCard: View {
    @Environment(\.theme) private var t
    let onOpen: () -> Void

    var body: some View {
        let user = FirebaseAccount.shared.user
        let name = user.flatMap { $0.displayName?.trimmingCharacters(in: .whitespaces).isEmpty == false ? $0.displayName : $0.email }
        let subtitle = user == nil
            ? String(localized: "drawer_account_signed_out_subtitle")
            : (user?.displayName?.isEmpty == false ? user?.email : nil)
        Button(action: onOpen) {
            HStack(spacing: 12) {
                Circle()
                    .fill(LinearGradient(colors: [t.accent, t.accent2], startPoint: .topLeading, endPoint: .bottomTrailing))
                    .frame(width: 44, height: 44)
                    .overlay {
                        if let initial = name?.first {
                            Text(String(initial).uppercased())
                                .font(.system(size: 17, weight: .semibold))
                                .foregroundColor(.white)
                        } else {
                            LXIcon(name: .sparkle, size: 18, color: .white, stroke: 1.8)
                        }
                    }
                VStack(alignment: .leading, spacing: 2) {
                    Text(name ?? String(localized: "drawer_account_signed_out_title"))
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(t.text)
                        .lineLimit(1)
                    if let subtitle {
                        Text(subtitle).font(.system(size: 12)).foregroundColor(t.text4).lineLimit(1)
                    }
                }
                Spacer(minLength: 8)
                LXIcon(name: .chevronR, size: 13, color: t.text4, stroke: 1.7)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .background(t.surface, in: RoundedRectangle(cornerRadius: 16))
            .overlay(RoundedRectangle(cornerRadius: 16).stroke(t.border, lineWidth: 0.5))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("settings.page.account")
    }
}
