import SwiftUI
import Observation

// MARK: - Settings domain models + mock data (verbatim from prototype)

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
}

struct MCPServer: Identifiable, Equatable {
    let id: String
    var name: String
    var url: String
    let tools: Int
    var status: ConnStatus
    var enabled: Bool
    let transport: String
    var auth: String? = nil
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
    static let llm: [ProviderPreset] = [
        .init(id: "anthropic",  name: "Anthropic",  sub: "Claude API",     color: Color(srgb: 0.9351, 0.5079, 0.4015), defaultUrl: "https://api.anthropic.com",                       keyPrefix: "sk-ant-",  models: ["claude-sonnet-4-5", "claude-opus-4", "claude-haiku-4-5"]),
        .init(id: "openai",     name: "OpenAI",     sub: "ChatGPT API",    color: Color(srgb: 0.1326, 0.7261, 0.5350), defaultUrl: "https://api.openai.com/v1",                       keyPrefix: "sk-proj-", models: ["gpt-4o", "gpt-4o-mini", "o1-preview"]),
        .init(id: "google",     name: "Google",     sub: "Gemini API",     color: Color(srgb: 0.3503, 0.6649, 0.9741), defaultUrl: "https://generativelanguage.googleapis.com/v1",    keyPrefix: "AIza",     models: ["gemini-2.5-pro", "gemini-2.5-flash"]),
        .init(id: "deepseek",   name: "DeepSeek",   sub: "DeepSeek API",   color: Color(srgb: 0.6451, 0.5662, 1.0000), defaultUrl: "https://api.deepseek.com",                        keyPrefix: "sk-",      models: ["deepseek-v4-flash", "deepseek-v4-pro"]),
        .init(id: "kimi",       name: "Kimi",       sub: "Moonshot AI",    color: Color(srgb: 0.4340, 0.5865, 1.0000), defaultUrl: "https://api.moonshot.cn/v1",                    keyPrefix: "sk-",      models: ["kimi-k3", "kimi-k2.7-code", "kimi-k2.7-code-highspeed", "kimi-k2.6"]),
        .init(id: "kimi-code",  name: "Kimi Code",  sub: String(localized: "settings_provider_preset_kimi_code_sub"),      color: Color(srgb: 0.2784, 0.6980, 0.9490), defaultUrl: "https://api.kimi.com/coding/v1",                 keyPrefix: "sk-",      models: ["kimi-for-coding", "k3", "k3-256k", "kimi-for-coding-highspeed"]),
        .init(id: "qwen",       name: String(localized: "settings_provider_preset_qwen_name"),     sub: "DashScope",      color: Color(srgb: 0.8826, 0.6256, 0.2074), defaultUrl: "https://dashscope.aliyuncs.com/v1",               keyPrefix: "sk-",      models: ["qwen-max", "qwen-plus", "qwen-turbo"]),
        .init(id: "openrouter", name: "OpenRouter", sub: String(localized: "settings_provider_preset_openrouter_sub"),      color: Color(srgb: 0.0000, 0.7441, 0.7802), defaultUrl: "https://openrouter.ai/api/v1",                    keyPrefix: "sk-or-",   models: ["anthropic/claude-sonnet-4.5", "openai/gpt-4o", "google/gemini-2.5-pro"]),
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

// MARK: - Settings store (the prototype's useState block in SettingsSheet)

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
    var skills: [Skill] = [
        .init(id: "sk1", name: String(localized: "settings_skill_seed_weekly_report_name"), author: "官方", desc: String(localized: "settings_skill_seed_weekly_report_desc"), triggers: [String(localized: "settings_skill_seed_weekly_report_trigger_1"), String(localized: "settings_skill_seed_weekly_report_trigger_2")], enabled: true, builtin: true),
        .init(id: "sk2", name: String(localized: "settings_skill_seed_code_review_name"), author: "官方", desc: String(localized: "settings_skill_seed_code_review_desc"), triggers: ["/review", String(localized: "settings_skill_seed_code_review_trigger_2")], enabled: true, builtin: true),
        .init(id: "sk3", name: String(localized: "settings_skill_seed_meeting_notes_name"), author: "官方", desc: String(localized: "settings_skill_seed_meeting_notes_desc"), triggers: [String(localized: "settings_skill_seed_meeting_notes_trigger_1")], enabled: true, builtin: true),
        .init(id: "sk4", name: String(localized: "settings_skill_seed_paper_reading_name"), author: "社区 · @arxiv-fan", desc: String(localized: "settings_skill_seed_paper_reading_desc"), triggers: [String(localized: "settings_skill_seed_paper_reading_trigger_1")], enabled: false, builtin: false),
        .init(id: "sk5", name: "CSS Doctor", author: "社区 · @lin", desc: String(localized: "settings_skill_seed_css_doctor_desc"), triggers: ["/css"], enabled: false, builtin: false),
        .init(id: "sk6", name: String(localized: "settings_skill_seed_polish_name"), author: "我", desc: String(localized: "settings_skill_seed_polish_desc"), triggers: ["/polish"], enabled: true, builtin: false),
    ]
    var mcpServers: [MCPServer] = [
        .init(id: "mcp1", name: "Filesystem",   url: "stdio://npx -y @modelcontextprotocol/server-filesystem", tools: 8,  status: .connected, enabled: true,  transport: "stdio"),
        .init(id: "mcp2", name: "GitHub",       url: "https://mcp.github.com",  tools: 14, status: .connected, enabled: true,  transport: "sse", auth: "oauth"),
        .init(id: "mcp3", name: "Linear",       url: "https://mcp.linear.app",  tools: 6,  status: .connected, enabled: true,  transport: "sse", auth: "oauth"),
        .init(id: "mcp4", name: "Notion",       url: "https://mcp.notion.com",  tools: 12, status: .idle,      enabled: false, transport: "sse", auth: "oauth"),
        .init(id: "mcp5", name: String(localized: "settings_mcp_seed_postgres_local_name"), url: "stdio://uvx mcp-server-postgres", tools: 4, status: .error, enabled: true, transport: "stdio"),
    ]
    var dream = DreamConfig()
    var language = "zh-CN"
    var notifs = NotifConfig()
    var bioLock = true
    var telemetry = false
    var autoUpdate = true

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
