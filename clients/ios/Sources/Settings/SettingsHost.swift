import SwiftUI

// MARK: - Settings page identity (the prototype's `stack` of `{id, ...}`)
enum SettingsPage: Hashable {
    case main, account
    case providerList(ProviderKindBox)
    case providerPicker(ProviderKindBox)
    case providerEdit(ProviderKindBox, String)
    case voice, linuxRuntime, knowledge, memory, workflows
    case appearance, language, notifications, input, appIntegration, privacy
    case skills, skillDetail(String)
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
    /// The conversation model — its `mcpServers` carry the engine's REAL MCP
    /// listing (out-of-band). When populated we mirror it into the settings store
    /// so the MCP page renders real servers; empty keeps the mock list.
    @ObservedObject var convo: ConversationModel
    /// Pull the real MCP listing from the engine (`RefreshListings(.mcp)`).
    var onRefreshMcp: () -> Void = {}
    /// Promote the runtime's PTY surface to the app-owned full-screen route.
    var openTerminal: () -> Void = {}
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
        .onAppear { onRefreshMcp() }
        .onChange(of: convo.mcpServers) { _, servers in
            if !servers.isEmpty { store.mcpServers = servers }
        }
        .accessibilityIdentifier("settings.root")
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
                    } else {
                        Button("settings_done") { reset() }
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(t.accent)
                    }
                }
            }
    }

    private func pageKey(for page: SettingsPage) -> String {
        "\(navigation.settingsPath.count)-\(title(of: page))"
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
        case .skills: return "Skills"
        case .skillDetail(let id): return store.skills.first(where: { $0.id == id })?.name ?? "Skill"
        case .mcpList: return String(localized: "settings_mcp_servers")
        case .mcpEdit(let id): return store.mcpServers.first(where: { $0.id == id })?.name ?? "MCP"
        case .dream: return String(localized: "settings_dream_mode")
        }
    }
}
