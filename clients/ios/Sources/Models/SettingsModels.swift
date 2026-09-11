import SwiftUI
import Observation

// MARK: - Settings domain models

enum ConnStatus: String {
    case connected, idle, testing, error

    var label: String {
        switch self {
        case .connected: return String(localized: "settings_provider_status_connected")
        case .idle:      return String(localized: "settings_provider_status_idle")
        case .testing:   return String(localized: "settings_provider_status_testing")
        case .error:     return String(localized: "settings_provider_status_failed")
        }
    }
    /// Dot color for a given palette (idle resolves to text4).
    func dot(_ t: Palette) -> Color {
        switch self {
        case .connected: return t.statusConnected
        case .idle:      return t.text4
        case .testing:   return t.statusTesting
        case .error:     return t.statusError
        }
    }
}

// MARK: Presets

struct ProviderPreset: Identifiable {
    let id: String
    let name: String
    let sub: String
    let color: Color
    let defaultUrl: String
    let keyPrefix: String
    let models: [String]
    var needsCx: Bool = false
}

struct GenericProvider: Identifiable, Equatable {
    let id: String
    var preset: String
    var name: String
    var url: String
    var key: String
    var model: String = ""
    var cx: String = ""
    var status: ConnStatus
    var isDefault: Bool = false
    var enabled: Bool
}

struct Skill: Identifiable, Equatable {
    let id: String
    var name: String
    let author: String
    let desc: String
    let triggers: [String]
    var enabled: Bool
    let builtin: Bool
    let source: String

    var sourceLabel: String {
        switch source {
        case "builtin": return String(localized: "skills_source_builtin")
        case "bundled": return String(localized: "skills_source_bundled")
        case "user": return String(localized: "skills_source_user")
        case "project": return String(localized: "skills_source_project")
        case "local": return String(localized: "skills_source_local")
        case "plugin": return String(localized: "skills_source_plugin")
        case "managed": return String(localized: "skills_source_managed")
        case "mcp": return "MCP"
        default: return source
        }
    }
}

struct MCPServer: Identifiable, Equatable {
    let id: String
    var name: String
    var url: String?
    var command: String
    var args: [String]
    var env: [String: String]
    var headers: [String: String]
    var tools: Int?
    var status: ConnStatus
    var enabled: Bool
    var transport: String
    var auth: String? = nil
    var scope: String? = nil

    static func normalizedTransport(_ value: String) -> String {
        switch value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
        case "websocket", "ws": return "ws"
        case "streamable-http": return "http"
        default: return value.lowercased()
        }
    }

    var transportLabel: String {
        switch Self.normalizedTransport(transport) {
        case "stdio": return "stdio"
        case "sse": return "SSE"
        case "ws": return "WebSocket"
        case "http": return "HTTP"
        default: return transport
        }
    }

    var endpointSummary: String {
        if Self.normalizedTransport(transport) == "stdio", !command.isEmpty {
            return ([command] + args).joined(separator: " ")
        }
        if let url, !url.isEmpty { return url }
        return String(localized: "mcp_endpoint_not_configured")
    }

    var isConfigured: Bool {
        if Self.normalizedTransport(transport) == "stdio" {
            return !command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
        return !(url ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
}

struct DreamConfig: Equatable {
    var enabled: Bool = true
    var window: String = "night"          // night | always | custom
    var onCharging: Bool = true
    var onWifi: Bool = true
    var activities: [String: Bool] = [
        "reorganize": true, "plan": true, "recap": true, "prefetch": false, "polish": false,
    ]
    var budget: String = "medium"         // low | medium | high
    let lastRun: String = String(localized: "settings_dream_last_run_seed")
}

struct NotifConfig: Equatable {
    var workflows = true
    var mentions = true
    var crons = true
    var marketing = false
    var enabledCount: Int { [workflows, mentions, crons, marketing].filter { $0 }.count }
}

enum LinuxRuntimeMode: String, CaseIterable, Equatable {
    case legacy = "Legacy"
    case mobileLinux = "Mobile Linux"
}

enum LinuxRuntimeAction: Equatable {
    case refresh, verify, repair, reset

    var label: String {
        switch self {
        case .refresh: return String(localized: "settings_linux_refresh_button")
        case .verify: return String(localized: "settings_linux_action_verify_short")
        case .repair: return String(localized: "settings_linux_action_repair_short")
        case .reset: return String(localized: "settings_linux_action_reset_short")
        }
    }
}

enum LinuxRuntimeRootfsState: Equatable {
    case missing, installing, ready, corrupt, repairing, resetting, unsupported, blockedByLicense

    var label: String {
        switch self {
        case .missing: return String(localized: "settings_linux_not_installed")
        case .installing: return String(localized: "settings_linux_state_installing")
        case .ready: return String(localized: "settings_linux_state_ready")
        case .corrupt: return String(localized: "settings_linux_state_corrupt")
        case .repairing: return String(localized: "settings_linux_state_repairing")
        case .resetting: return String(localized: "settings_linux_state_resetting")
        case .unsupported: return String(localized: "settings_linux_badge_not_linked")
        case .blockedByLicense: return String(localized: "settings_linux_badge_blocked")
        }
    }
}

struct LinuxRuntimeState: Equatable {
    var selectedMode: LinuxRuntimeMode = .mobileLinux
    var backend: String = "ios-ish"
    var rootfsState: LinuxRuntimeRootfsState = .missing
    var version: String? = nil
    var managedRoot: String? = nil
    var installedSizeBytes: UInt64? = nil
    var available: Bool = false
    var terminalSupported: Bool = false
    var backgroundTasksSupported: Bool = false
    var verifyAllowed: Bool = false
    var repairAllowed: Bool = false
    var resetAllowed: Bool = false
    var writableGuestPaths: [String] = []
    var summary: String = String(localized: "settings_linux_summary_checking")
    var detail: String = String(localized: "settings_linux_detail_default")
    var lastAction: LinuxRuntimeAction? = nil
    var lastActionMessage: String? = nil
    var busyAction: LinuxRuntimeAction? = nil
    var tasks: [LinuxRuntimeTaskRow] = []
    var mounts: [LinuxRuntimeMountRow] = []
    var terminal = LinuxTerminalState()

    var badge: String {
        if selectedMode == .legacy { return String(localized: "settings_badge_default") }
        switch rootfsState {
        case .blockedByLicense: return String(localized: "settings_linux_badge_blocked")
        case .unsupported: return String(localized: "settings_linux_badge_not_linked")
        default: return available ? String(localized: "settings_status_available") : String(localized: "settings_linux_task_unavailable")
        }
    }

    var canOpenTerminal: Bool { terminalSupported && available }

    /// Fold a bridge PROBE result into this state. The ONLY sanctioned way to
    /// apply a whole probe: `LinuxRuntimeBridge` builds a full
    /// `LinuxRuntimeState`, which is a bigger type than the probe's data — it
    /// contains user-INPUT fields the probe cannot know. A wholesale
    /// `store.linuxRuntime = probed` clobbers those by default, and every new
    /// writer had to remember a hand-merge at its call site (the draft-command
    /// clobber shipped exactly that way). The preserve list now lives here,
    /// once, next to the type it protects.
    mutating func applyProbe(_ probe: LinuxRuntimeState) {
        let preservedTerminal = terminal
        self = probe
        terminal = preservedTerminal
    }
}

// MARK: - Presets data

enum Presets {
    /// A preset's `models` are suggestions AND the fallback the app writes into
    /// `ProviderStoredProfile.modelID` when it has no better answer (adding a
    /// provider, or migrating a legacy Anthropic Keychain whose stored model
    /// belongs to someone else). That fallback becomes the launch
    /// `defaultModelID`, so every id here must be one the engine still curates
    /// (`traits::is_curated_model`) — a stale id is registered only because it
    /// is the configured default and renders as a stray row above the real
    /// shortlist, the same wart `MobileEngineConfig::default()` was moved off
    /// `claude-sonnet-4-20250514` to avoid. Anthropic's list mirrors
    /// `traits::is_curated_model`'s "anthropic" arm, `provider_default_model`
    /// first.
    static let llm: [ProviderPreset] = [
        .init(id: "anthropic",  name: "Anthropic",  sub: "Claude API",     color: Color(srgb: 0.9351, 0.5079, 0.4015), defaultUrl: "https://api.anthropic.com",                       keyPrefix: "sk-ant-",  models: ["claude-opus-5", "claude-fable-5-1", "claude-sonnet-5", "claude-haiku-4-5"]),
        .init(id: "openai",     name: "OpenAI",     sub: "ChatGPT API",    color: Color(srgb: 0.1326, 0.7261, 0.5350), defaultUrl: "https://api.openai.com/v1",                       keyPrefix: "sk-proj-", models: ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]),
        .init(id: "openai-chatgpt", name: "ChatGPT", sub: "ChatGPT OAuth", color: Color(srgb: 0.1326, 0.7261, 0.5350), defaultUrl: "https://chatgpt.com/backend-api/codex", keyPrefix: "", models: ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]),
        .init(id: "google",     name: "Google",     sub: "Gemini API",     color: Color(srgb: 0.3503, 0.6649, 0.9741), defaultUrl: "https://generativelanguage.googleapis.com/v1",    keyPrefix: "AIza",     models: ["gemini-3.7-flash", "gemini-3.6-flash", "gemini-3.5-flash", "gemini-3.1-pro-preview"]),
        .init(id: "deepseek",   name: "DeepSeek",   sub: "DeepSeek API",   color: Color(srgb: 0.6451, 0.5662, 1.0000), defaultUrl: "https://api.deepseek.com",                        keyPrefix: "sk-",      models: ["deepseek-flash", "deepseek-v4-pro"]),
        .init(id: "kimi",       name: "Kimi",       sub: "Moonshot AI",    color: Color(srgb: 0.4340, 0.5865, 1.0000), defaultUrl: "https://api.moonshot.cn/v1",                    keyPrefix: "sk-",      models: ["kimi-k3"]),
        .init(id: "kimi-code",  name: "Kimi Code",  sub: String(localized: "settings_provider_preset_kimi_code_sub"),      color: Color(srgb: 0.2784, 0.6980, 0.9490), defaultUrl: "https://api.kimi.com/coding/v1",                 keyPrefix: "sk-",      models: ["k3"]),
        .init(id: "qwen",       name: String(localized: "settings_provider_preset_qwen_name"),     sub: "DashScope",      color: Color(srgb: 0.8826, 0.6256, 0.2074), defaultUrl: "https://dashscope.aliyuncs.com/v1",               keyPrefix: "sk-",      models: ["qwen-max", "qwen-plus", "qwen-turbo"]),
        .init(
            id: "openrouter",
            name: "OpenRouter",
            sub: String(localized: "settings_provider_preset_openrouter_sub"),
            color: Color(srgb: 0.0000, 0.7441, 0.7802),
            defaultUrl: "https://openrouter.ai/api/v1",
            keyPrefix: "sk-or-",
            models: [
                "openrouter/auto", "openrouter/free",
                "~anthropic/claude-fable-latest", "~anthropic/claude-opus-latest",
                "~anthropic/claude-sonnet-latest", "~openai/gpt-latest",
                "~openai/gpt-mini-latest", "~google/gemini-pro-latest",
                "~google/gemini-flash-latest", "~deepseek/deepseek-v4-flash-latest",
                "~moonshotai/kimi-latest", "~z-ai/glm-latest", "~z-ai/glm-flash-latest",
                "~x-ai/grok-latest",
                "nvidia/nemotron-3-ultra-550b-a55b:free", "minimax/minimax-m3:free",
                "poolside/laguna-s-2.1:free", "nvidia/nemotron-3.5-lightning:free",
                "inclusionai/ling-3.0-flash-fin:free", "cohere/north-mini-code:free",
                "z-ai/glm-5.2:free", "thinkingmachines/inkling:free",
                "thinkingmachines/inkling-small:free", "minimax/minimax-m2.7:free",
            ]
        ),
        .init(id: "custom",     name: String(localized: "settings_provider_preset_custom_name"),       sub: String(localized: "settings_provider_preset_custom_sub"), color: Color(srgb: 0.5728, 0.6177, 0.7466), defaultUrl: "https://",                                       keyPrefix: "",         models: []),
    ]

    static let search: [ProviderPreset] = [
        .init(id: "google", name: "Google", sub: "官方 Custom Search", color: Color(srgb: 0.3503, 0.6649, 0.9741), defaultUrl: "https://www.googleapis.com/customsearch/v1", keyPrefix: "", models: [], needsCx: true),
        .init(id: "brave",  name: "Brave",  sub: "独立索引 · 隐私优先", color: Color(srgb: 0.8716, 0.2418, 0.1752), defaultUrl: "https://api.search.brave.com/res/v1", keyPrefix: "", models: []),
        .init(id: "tavily", name: "Tavily", sub: "AI 优化搜索",        color: Color(srgb: 0.0000, 0.7151, 0.7672), defaultUrl: "https://api.tavily.com", keyPrefix: "", models: []),
        .init(id: "serper", name: "Serper", sub: "Google 代理",        color: Color(srgb: 0.0000, 0.7391, 0.5219), defaultUrl: "https://google.serper.dev", keyPrefix: "", models: []),
        .init(id: "bing",   name: "Bing",   sub: "Microsoft",          color: Color(srgb: 0.0000, 0.7200, 0.8810), defaultUrl: "https://api.bing.microsoft.com/v7.0", keyPrefix: "", models: []),
    ]

    static let fetch: [ProviderPreset] = [
        .init(id: "jina",        name: "Jina Reader", sub: "免费 · 推荐",      color: Color(srgb: 0.8713, 0.5800, 0.0000), defaultUrl: "https://r.jina.ai", keyPrefix: "", models: []),
        .init(id: "firecrawl",   name: "Firecrawl",   sub: "渲染 JS · 结构化", color: Color(srgb: 1.0000, 0.4030, 0.1579), defaultUrl: "https://api.firecrawl.dev/v1", keyPrefix: "", models: []),
        .init(id: "browserless", name: "Browserless", sub: "Headless Chrome",  color: Color(srgb: 0.6203, 0.5486, 0.9581), defaultUrl: "https://chrome.browserless.io", keyPrefix: "", models: []),
        .init(id: "scrapingbee", name: "ScrapingBee", sub: "反爬代理",         color: Color(srgb: 0.8960, 0.6013, 0.0000), defaultUrl: "https://app.scrapingbee.com/api/v1", keyPrefix: "", models: []),
    ]

}

// MARK: - Settings store

enum ProviderKind: Hashable { case llm, search, fetch
    var title: String { self == .llm ? String(localized: "settings_llm_providers") : self == .search ? String(localized: "settings_web_search") : String(localized: "settings_web_fetch") }
    var presets: [ProviderPreset] { self == .llm ? Presets.llm : self == .search ? Presets.search : Presets.fetch }
    var idPrefix: String { self == .llm ? "l" : self == .search ? "s" : "f" }
}

@MainActor
@Observable
final class SettingsStore {
    /// Configured rows only. Presets remain a catalog but are never presented as
    /// connected accounts until the real repository persists them.
    var llmProviders: [GenericProvider] = []
    var searchProviders: [GenericProvider] = []
    var fetchProviders: [GenericProvider] = []
    var linuxRuntime = LinuxRuntimeState()
    /// Populated only by the engine's SlashCommandCatalog; no placeholder
    /// skills are shown while the real catalog is still loading.
    var skills: [Skill] = []
    var skillsLoaded = false
    /// Populated from the engine's MCP listing and app-private config file.
    var mcpServers: [MCPServer] = []
    var mcpListingLoaded = false
    var mcpConfigurationError: String?
    var dream = DreamConfig()
    var language = "zh-CN"
    var notifs = NotifConfig()
    var bioLock = true
    var telemetry = false
    var autoUpdate = true
    var permissionMode = "auto"
    var effectivePermissionMode = "auto"
    var permissionModeError: String?
    var typescriptLspMode = "auto"
    var effectiveTypescriptLspMode = "auto"
    var typescriptLspAvailable = true
    var typescriptLspError: String?

    @discardableResult
    func setPermissionMode(_ mode: String) -> Bool {
        guard ["default", "acceptEdits", "plan", "auto", "dontAsk", "bypassPermissions"].contains(mode) else { return false }
        permissionMode = mode
        permissionModeError = nil
        return true
    }

    func restorePermissionMode(_ mode: String, effectiveMode: String? = nil, error: String? = nil) {
        permissionMode = mode
        effectivePermissionMode = effectiveMode ?? mode
        permissionModeError = error
    }

    @discardableResult
    func setTypescriptLspMode(_ mode: String) -> Bool {
        guard ["auto", "off", "on"].contains(mode) else { return false }
        typescriptLspMode = mode
        typescriptLspError = nil
        return true
    }

    func restoreTypescriptLspMode(
        _ mode: String,
        effectiveMode: String,
        available: Bool,
        error: String? = nil
    ) {
        typescriptLspMode = mode
        effectiveTypescriptLspMode = effectiveMode
        typescriptLspAvailable = available
        typescriptLspError = error
    }

    // MARK: store access by kind

    func providers(_ kind: ProviderKind) -> [GenericProvider] {
        switch kind {
        case .llm: return llmProviders
        case .search: return searchProviders
        case .fetch: return fetchProviders
        }
    }
    func setProviders(_ kind: ProviderKind, _ value: [GenericProvider]) {
        switch kind {
        case .llm: llmProviders = value
        case .search: searchProviders = value
        case .fetch: fetchProviders = value
        }
    }
    func binding(_ kind: ProviderKind) -> Binding<[GenericProvider]> {
        Binding(get: { self.providers(kind) }, set: { self.setProviders(kind, $0) })
    }

    func preset(of provider: GenericProvider, kind: ProviderKind) -> ProviderPreset {
        kind.presets.first(where: { $0.id == provider.preset }) ?? kind.presets[kind.presets.count - 1]
    }

    func update(_ kind: ProviderKind, id: String, _ mutate: (inout GenericProvider) -> Void) {
        var arr = providers(kind)
        if let i = arr.firstIndex(where: { $0.id == id }) { mutate(&arr[i]); setProviders(kind, arr) }
    }
    func setDefault(_ kind: ProviderKind, id: String) {
        var arr = providers(kind)
        for i in arr.indices { arr[i].isDefault = (arr[i].id == id) }
        setProviders(kind, arr)
    }
    func remove(_ kind: ProviderKind, id: String) {
        setProviders(kind, providers(kind).filter { $0.id != id })
    }
    /// Adds a provider from a preset; returns the new id.
    @discardableResult
    func addProvider(_ kind: ProviderKind, presetId: String) -> String {
        let preset = kind.presets.first(where: { $0.id == presetId })!
        let id = kind.idPrefix + "_" + String(UUID().uuidString.prefix(5)).lowercased()
        let next = GenericProvider(id: id, preset: presetId, name: preset.name, url: preset.defaultUrl,
                                   key: "", model: preset.models.first ?? "", status: .idle, enabled: true)
        var arr = providers(kind); arr.append(next); setProviders(kind, arr)
        return id
    }
}
