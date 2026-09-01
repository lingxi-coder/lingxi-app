import SwiftUI

// MARK: - Settings page identity (the prototype's `stack` of `{id, ...}`)
enum SettingsPage: Hashable {
    case main, account
    case providerList(ProviderKindBox)
    case providerPicker(ProviderKindBox)
    case providerEdit(ProviderKindBox, String)
    case voice, linuxRuntime, knowledge, memory, workflows
    case appearance, language, notifications, input, appIntegration, privacy, permissionMode, typescriptLsp
    case skills, skillDetail(String)
    case localAppPlugin
    case mcpList, mcpEdit(String)
    case dream
}

/// Equatable/Hashable wrapper so ProviderKind can live inside the enum.
struct ProviderKindBox: Equatable, Hashable {
    let kind: ProviderKind
    init(_ k: ProviderKind) { kind = k }
    static func == (l: ProviderKindBox, r: ProviderKindBox) -> Bool {
        String(describing: l.kind) == String(describing: r.kind)
    }
    func hash(into h: inout Hasher) { h.combine(String(describing: kind)) }
}

// MARK: - Settings sheet host
struct SettingsHost: View {
    @Environment(AppState.self) private var app
    @Environment(\.theme) private var t
    @Bindable var store: SettingsStore
    /// The conversation model — its `mcpServers` carry the engine's real MCP
    /// listing (out-of-band). The settings store starts with persisted config
    /// and is replaced by the engine's authoritative live listing on refresh.
    @ObservedObject var convo: ConversationModel
    @Bindable var localAppsStore: LocalAppsStore
    /// Active project root used to read/write the engine's project `.mcp.json`.
    var projectCwd: String? = nil
    @State private var providerRepository = ProviderRepository.shared
    /// Pull the real MCP listing from the engine (`RefreshListings(.mcp)`).
    var onRefreshMcp: () -> Void = {}
    /// Pull the engine's live slash-command/skill catalog.
    var onRefreshSkills: () -> Void = {}
    let mcpRepository: MCPConfigurationRepository = .shared
    /// Promote the runtime's PTY surface to the app-owned full-screen route.
    var openTerminal: () -> Void = {}
    var onPermissionModeChanged: (String) async throws -> Void = { _ in }
    var onTypescriptLspModeChanged: (String) async throws -> Void = { _ in }
    let onClose: () -> Void

    /// Settings deep links and in-sheet navigation share the app-owned typed
    /// navigation model instead of copying a View that owns a Binding.
    @Bindable var navigation: AppNavigationModel

    func push(_ page: SettingsPage) {
        withAnimation(.easeOut(duration: 0.22)) { navigation.pushSettings(page) }
    }
    func pop() {
        withAnimation(.easeOut(duration: 0.22)) { navigation.popSettings() }
    }
    func reset() {
        withAnimation(.easeOut(duration: 0.22)) { navigation.resetSettings() }
    }
    /// Replaces the top two pages (used by the picker → edit flow).
    func replaceTopTwo(with pages: [SettingsPage]) {
        navigation.replaceSettingsTail(removing: 2, with: pages)
    }

    func applyPermissionMode(_ mode: String) {
        let previous = store.permissionMode
        let previousEffective = store.effectivePermissionMode
        guard store.setPermissionMode(mode) else { return }
        Task { @MainActor in
            do {
                try await onPermissionModeChanged(mode)
            } catch {
                store.restorePermissionMode(previous, effectiveMode: previousEffective, error: error.localizedDescription)
            }
        }
    }

    func applyTypescriptLspMode(_ mode: String) {
        let previous = store.typescriptLspMode
        let previousEffective = store.effectiveTypescriptLspMode
        let previousAvailable = store.typescriptLspAvailable
        guard store.setTypescriptLspMode(mode) else { return }
        Task { @MainActor in
            do {
                try await onTypescriptLspModeChanged(mode)
            } catch {
                store.restoreTypescriptLspMode(
                    previous,
                    effectiveMode: previousEffective,
                    available: previousAvailable,
                    error: error.localizedDescription
                )
            }
        }
    }

    var body: some View {
        NavigationStack(path: $navigation.settingsPath) {
            navigationPage(for: .main)
                .navigationDestination(for: SettingsPage.self) { page in
                    navigationPage(for: page)
                }
        }
        .background(t.windowBg)
        // Settings controls draw their own cards and fills. Avoid the automatic
        // tinted capsule that newer iOS versions add around custom labels.
        .buttonStyle(.plain)
        // Pull the real MCP listing when the sheet opens; mirror it into the store
        // (so the existing MCP page renders real servers) once it arrives.
        .onAppear {
            // Configured rows are useful before the first engine event; the
            // event itself remains authoritative for connection health.
            if convo.mcpServersLoaded {
                syncMcpServers(convo.mcpServers)
            } else if store.mcpServers.isEmpty {
                store.mcpServers = mcpRepository.loadServers(projectCwd: projectCwd)
            }
            store.skills = convo.skills
            store.skillsLoaded = convo.skillsLoaded
            onRefreshMcp()
            onRefreshSkills()
            store.permissionMode = convo.requestedPermissionMode
            store.effectivePermissionMode = convo.effectivePermissionMode
            store.permissionModeError = nil
            store.typescriptLspMode = convo.requestedTypescriptLspMode
            store.effectiveTypescriptLspMode = convo.effectiveTypescriptLspMode
            store.typescriptLspAvailable = convo.typescriptLspAvailable
            store.typescriptLspError = nil
            Task { await localAppsStore.refreshBuiltinPluginStatus() }
        }
        .onChange(of: convo.requestedPermissionMode) { _, mode in
            store.permissionMode = mode
        }
        .onChange(of: convo.effectivePermissionMode) { _, mode in
            store.effectivePermissionMode = mode
        }
        .onChange(of: convo.requestedTypescriptLspMode) { _, mode in
            store.typescriptLspMode = mode
        }
        .onChange(of: convo.effectiveTypescriptLspMode) { _, mode in
            store.effectiveTypescriptLspMode = mode
        }
        .onChange(of: convo.typescriptLspAvailable) { _, available in
            store.typescriptLspAvailable = available
        }
        .onChange(of: convo.mcpServers) { _, servers in
            syncMcpServers(servers)
        }
        .onChange(of: convo.mcpServersLoaded) { _, _ in
            syncMcpServers(convo.mcpServers)
        }
        .onChange(of: convo.skills) { _, skills in
            store.skills = skills
        }
        .onChange(of: convo.skillsLoaded) { _, loaded in
            store.skillsLoaded = loaded
        }
        .alert(
            String(localized: "mcp_configuration_error_title"),
            isPresented: Binding(
                get: { store.mcpConfigurationError != nil },
                set: { if !$0 { store.mcpConfigurationError = nil } }
            )
        ) {
            Button("settings_done") { store.mcpConfigurationError = nil }
        } message: {
            Text(store.mcpConfigurationError ?? String(localized: "mcp_configuration_save_failed"))
        }
        .accessibilityIdentifier("settings.root")
    }

    private func syncMcpServers(_ servers: [MCPServer]) {
        let configured = Dictionary(
            uniqueKeysWithValues: mcpRepository.loadServers(projectCwd: projectCwd).map { ($0.id, $0) }
        )
        store.mcpServers = servers.map { live in
            guard let saved = configured[live.id] else { return live }
            var merged = live
            merged.url = saved.url
            merged.command = saved.command
            merged.args = saved.args
            merged.env = saved.env
            merged.headers = saved.headers
            merged.enabled = saved.enabled
            merged.auth = saved.auth
            merged.scope = saved.scope
            return merged
        }
        store.mcpListingLoaded = convo.mcpServersLoaded
    }

    private func pageContent(for page: SettingsPage) -> some View {
        ScrollView(showsIndicators: false) {
            SettingsPages(store: store, host: self, page: page)
                .padding(.horizontal, 16).padding(.top, 16).padding(.bottom, 28)
        }
        .id(pageKey(for: page))
        .transition(.opacity)
    }

    private func navigationPage(for page: SettingsPage) -> some View {
        pageContent(for: page)
            .navigationTitle(title(of: page))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    if page == .main {
                        Button(action: onClose) {
                            LXIcon(name: .x, size: 18, color: t.text3, stroke: 1.8)
                                .frame(width: 34, height: 34)
                        }
                        .accessibilityLabel(String(localized: "settings_close_accessibility"))
                    } else if case .providerEdit(let box, _) = page,
                              box.kind == .llm {
                        // The provider editor is now a self-contained draft
                        // sheet and owns its Save/Cancel actions.
                        EmptyView()
                    } else if case .mcpEdit(let serverID) = page {
                        if managedMcpInventory(serverName: serverID) == nil {
                            Button("settings_save") { saveMcpAndClose(serverID) }
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundColor(t.accent)
                        } else {
                            Button("settings_done") { handleDone(for: page) }
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundColor(t.accent)
                        }
                    } else {
                        Button("settings_done") { handleDone(for: page) }
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(t.accent)
                    }
                }
            }
    }

    private func pageKey(for page: SettingsPage) -> String {
        "\(navigation.settingsPath.count)-\(title(of: page))"
    }

    private func handleDone(for page: SettingsPage) {
        reset()
    }

    func addMcpServer() {
        let id = "new-" + UUID().uuidString.lowercased()
        store.mcpServers.append(
            MCPServer(
                id: id,
                name: "",
                url: nil,
                command: "",
                args: [],
                env: [:],
                headers: [:],
                tools: nil,
                status: .idle,
                enabled: true,
                transport: "http"
            )
        )
        push(.mcpEdit(id))
    }

    func refreshMcp() {
        onRefreshMcp()
    }

    func refreshSkills() {
        onRefreshSkills()
    }

    @discardableResult
    func saveMcpServer(_ server: MCPServer) -> Bool {
        if managedMcpInventory(serverName: server.id) != nil { return false }
        do {
            try mcpRepository.save(server, projectCwd: projectCwd)
            onRefreshMcp()
            return true
        } catch {
            store.mcpConfigurationError = String(localized: "mcp_configuration_save_failed")
            return false
        }
    }

    func removeMcpServer(_ server: MCPServer) {
        if managedMcpInventory(serverName: server.id) != nil { return }
        if !server.id.hasPrefix("new-") {
            do {
                try mcpRepository.delete(server, projectCwd: projectCwd)
            } catch {
                store.mcpConfigurationError = String(localized: "mcp_configuration_save_failed")
                return
            }
        }
        store.mcpServers.removeAll { $0.id == server.id }
        onRefreshMcp()
        pop()
    }

    private func saveProviderAndClose(_ providerID: String) {
        Task {
            await providerRepository.applyChanges(providerID)
            store.llmProviders = providerRepository.legacyProviders()
            guard providerRepository.state(for: providerID)?.connectionState != .failed else {
                return
            }
            reset()
        }
    }

    private func saveMcpAndClose(_ serverID: String) {
        if managedMcpInventory(serverName: serverID) != nil {
            reset()
            return
        }
        guard let server = store.mcpServers.first(where: { $0.id == serverID }) else { return }
        guard !server.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              server.isConfigured else {
            store.mcpConfigurationError = String(localized: "mcp_configuration_invalid")
            return
        }
        guard saveMcpServer(server) else { return }
        reset()
    }

    // MARK: titles
    func title(of page: SettingsPage) -> String {
        switch page {
        case .main: return String(localized: "settings_title_main")
        case .account: return String(localized: "settings_title_account")
        case .providerList(let b): return b.kind.title
        case .providerPicker(let b):
            return b.kind == .llm ? String(localized: "settings_title_add_llm")
                : b.kind == .search ? String(localized: "settings_title_add_search")
                : String(localized: "settings_title_add_fetch")
        case .providerEdit(let b, let id):
            if b.kind == .llm {
                return providerRepository.state(for: id)?.profile.name ?? String(localized: "settings_title_edit")
            }
            return store.providers(b.kind).first(where: { $0.id == id })?.name ?? String(localized: "settings_title_edit")
        case .voice: return String(localized: "settings_voice_tts")
        case .linuxRuntime: return String(localized: "settings_linux_runtime")
        case .knowledge: return String(localized: "settings_knowledge")
        case .memory: return String(localized: "settings_memory")
        case .workflows: return String(localized: "settings_title_workflows")
        case .appearance: return String(localized: "settings_appearance")
        case .language: return String(localized: "settings_language_title")
        case .notifications: return String(localized: "settings_notifications")
        case .input: return String(localized: "settings_keyboard_input")
        case .appIntegration: return String(localized: "settings_app_integration")
        case .privacy: return String(localized: "settings_data_privacy")
        case .permissionMode: return "权限模式"
        case .typescriptLsp: return "TypeScript LSP"
        case .skills: return "Skills"
        case .skillDetail(let id): return store.skills.first(where: { $0.id == id })?.name ?? "Skill"
        case .localAppPlugin: return String(localized: "local_apps_plugin_title")
        case .mcpList: return String(localized: "settings_mcp_servers")
        case .mcpEdit(let id): return store.mcpServers.first(where: { $0.id == id })?.name ?? "MCP"
        case .dream: return String(localized: "settings_dream_mode")
        }
    }

    func managedMcpInventory(serverName: String) -> LocalAppManagedMcpInventory? {
        localAppsStore.managedMcpInventory(serverName: serverName)
    }
}
