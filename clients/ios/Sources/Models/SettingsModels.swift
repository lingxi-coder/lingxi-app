import SwiftUI

// MARK: - Settings domain models + mock data (verbatim from prototype)

enum ConnStatus: String {
    case connected, idle, testing, error

    var label: String {
        switch self {
        case .connected: return "已连接"
        case .idle:      return "未验证"
        case .testing:   return "检测中…"
        case .error:     return "连接失败"
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

struct VoiceConfig: Equatable {
    var preset: String = "system"
    var voiceId: String = "zh-CN-XiaoxiaoNeural"
    var speed: Double = 1.0
    var autoPlay: Bool = false
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
    let lastRun: String = "今早 03:24 · 整理 7 条记忆 / 草拟今日计划"
}

struct NotifConfig: Equatable {
    var workflows = true
    var mentions = true
    var crons = true
    var marketing = false
    var enabledCount: Int { [workflows, mentions, crons, marketing].filter { $0 }.count }
}

// MARK: - Presets data

enum Presets {
    static let llm: [ProviderPreset] = [
        .init(id: "anthropic",  name: "Anthropic",  sub: "Claude API",     color: Color(srgb: 0.9351, 0.5079, 0.4015), defaultUrl: "https://api.anthropic.com",                       keyPrefix: "sk-ant-",  models: ["claude-sonnet-4-5", "claude-opus-4", "claude-haiku-4-5"]),
        .init(id: "openai",     name: "OpenAI",     sub: "ChatGPT API",    color: Color(srgb: 0.1326, 0.7261, 0.5350), defaultUrl: "https://api.openai.com/v1",                       keyPrefix: "sk-proj-", models: ["gpt-4o", "gpt-4o-mini", "o1-preview"]),
        .init(id: "google",     name: "Google",     sub: "Gemini API",     color: Color(srgb: 0.3503, 0.6649, 0.9741), defaultUrl: "https://generativelanguage.googleapis.com/v1",    keyPrefix: "AIza",     models: ["gemini-2.5-pro", "gemini-2.5-flash"]),
        .init(id: "deepseek",   name: "DeepSeek",   sub: "DeepSeek API",   color: Color(srgb: 0.6451, 0.5662, 1.0000), defaultUrl: "https://api.deepseek.com/v1",                     keyPrefix: "sk-",      models: ["deepseek-chat", "deepseek-reasoner"]),
        .init(id: "qwen",       name: "通义千问",     sub: "DashScope",      color: Color(srgb: 0.8826, 0.6256, 0.2074), defaultUrl: "https://dashscope.aliyuncs.com/v1",               keyPrefix: "sk-",      models: ["qwen-max", "qwen-plus", "qwen-turbo"]),
        .init(id: "openrouter", name: "OpenRouter", sub: "多模型聚合",      color: Color(srgb: 0.0000, 0.7441, 0.7802), defaultUrl: "https://openrouter.ai/api/v1",                    keyPrefix: "sk-or-",   models: ["anthropic/claude-sonnet-4.5", "openai/gpt-4o", "google/gemini-2.5-pro"]),
        .init(id: "custom",     name: "自定义",       sub: "OpenAI 兼容端点", color: Color(srgb: 0.5728, 0.6177, 0.7466), defaultUrl: "https://",                                       keyPrefix: "",         models: []),
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

    static let voice: [ProviderPreset] = [
        .init(id: "elevenlabs", name: "ElevenLabs", sub: "高质量 · 多语种", color: Color(srgb: 0.8018, 0.4038, 0.8909), defaultUrl: "https://api.elevenlabs.io/v1", keyPrefix: "", models: []),
        .init(id: "openai-tts", name: "OpenAI TTS", sub: "低延迟",          color: Color(srgb: 0.1326, 0.7261, 0.5350), defaultUrl: "https://api.openai.com/v1", keyPrefix: "", models: []),
        .init(id: "system",     name: "系统语音",    sub: "设备本地 · 免费",  color: Color(srgb: 0.5728, 0.6177, 0.7466), defaultUrl: "", keyPrefix: "", models: []),
    ]
}

// MARK: - Settings store (the prototype's useState block in SettingsSheet)

enum ProviderKind { case llm, search, fetch
    var title: String { self == .llm ? "LLM 提供商" : self == .search ? "联网搜索" : "网页抓取" }
    var presets: [ProviderPreset] { self == .llm ? Presets.llm : self == .search ? Presets.search : Presets.fetch }
    var idPrefix: String { self == .llm ? "l" : self == .search ? "s" : "f" }
}

final class SettingsStore: ObservableObject {
    @Published var llmProviders: [GenericProvider] = [
        .init(id: "p_ant", preset: "anthropic", name: "Anthropic", url: "https://api.anthropic.com",  key: "sk-ant-api03-••••••••7Hq2", model: "claude-sonnet-4-5", status: .connected, isDefault: true, enabled: true),
        .init(id: "p_oai", preset: "openai",    name: "OpenAI",    url: "https://api.openai.com/v1",   key: "sk-proj-••••••••4nQ8",      model: "gpt-4o",            status: .idle,                     enabled: true),
        .init(id: "p_dsk", preset: "deepseek",  name: "DeepSeek",  url: "https://api.deepseek.com/v1", key: "",                          model: "deepseek-chat",     status: .idle,                     enabled: false),
    ]
    @Published var searchProviders: [GenericProvider] = [
        .init(id: "s_brv", preset: "brave",  name: "Brave",  url: "https://api.search.brave.com/res/v1", key: "BSA••••••a9Z", status: .connected, isDefault: true, enabled: true),
        .init(id: "s_jin", preset: "tavily", name: "Tavily", url: "https://api.tavily.com", key: "", status: .idle, enabled: false),
    ]
    @Published var fetchProviders: [GenericProvider] = [
        .init(id: "f_jin", preset: "jina", name: "Jina Reader", url: "https://r.jina.ai", key: "", status: .connected, isDefault: true, enabled: true),
    ]
    @Published var voice = VoiceConfig()
    @Published var skills: [Skill] = [
        .init(id: "sk1", name: "周报生成", author: "官方", desc: "聚合 Linear / GitHub / 日历自动出周报", triggers: ["每周五 17:00", "@周报"], enabled: true, builtin: true),
        .init(id: "sk2", name: "代码评审", author: "官方", desc: "对粘贴的 diff 给出严格 review", triggers: ["/review", "拖入 .diff"], enabled: true, builtin: true),
        .init(id: "sk3", name: "会议纪要", author: "官方", desc: "从语音/文本提取要点 + action item", triggers: ["会议结束后"], enabled: true, builtin: true),
        .init(id: "sk4", name: "论文精读", author: "社区 · @arxiv-fan", desc: "arXiv 链接 → 结构化摘要 + 批注", triggers: ["粘贴 arxiv URL"], enabled: false, builtin: false),
        .init(id: "sk5", name: "CSS Doctor", author: "社区 · @lin", desc: "诊断布局问题并给出修复", triggers: ["/css"], enabled: false, builtin: false),
        .init(id: "sk6", name: "英文润色", author: "我", desc: "中→英写作润色，保留原意", triggers: ["/polish"], enabled: true, builtin: false),
    ]
    @Published var mcpServers: [MCPServer] = [
        .init(id: "mcp1", name: "Filesystem",   url: "stdio://npx -y @modelcontextprotocol/server-filesystem", tools: 8,  status: .connected, enabled: true,  transport: "stdio"),
        .init(id: "mcp2", name: "GitHub",       url: "https://mcp.github.com",  tools: 14, status: .connected, enabled: true,  transport: "sse", auth: "oauth"),
        .init(id: "mcp3", name: "Linear",       url: "https://mcp.linear.app",  tools: 6,  status: .connected, enabled: true,  transport: "sse", auth: "oauth"),
        .init(id: "mcp4", name: "Notion",       url: "https://mcp.notion.com",  tools: 12, status: .idle,      enabled: false, transport: "sse", auth: "oauth"),
        .init(id: "mcp5", name: "Postgres (本地)", url: "stdio://uvx mcp-server-postgres", tools: 4, status: .error, enabled: true, transport: "stdio"),
    ]
    @Published var dream = DreamConfig()
    @Published var language = "zh-CN"
    @Published var notifs = NotifConfig()
    @Published var smartRouting = true
    @Published var streamingDefault = true
    @Published var bioLock = true
    @Published var telemetry = false
    @Published var autoUpdate = true

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
