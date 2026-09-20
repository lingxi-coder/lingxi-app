import SwiftUI

// MARK: - Domain models + mock data (verbatim from lingxi-iphone.html)

struct Workspace: Identifiable, Equatable {
    let id: String
    let name: String
    let icon: String
    let color: Color
}

struct Chat: Identifiable, Equatable {
    let id: String
    let wsId: String
    let title: String
    let group: String
    let preview: String
    let activity: String
}

struct ProjectSession: Identifiable, Equatable {
    let id: String
    let title: String
    let activity: String
    let preview: String
    var pinned: Bool = false
    let msgs: Int
}

struct Project: Identifiable, Equatable {
    let id: String
    let wsId: String
    let name: String
    let icon: String
    let color: Color
    let desc: String
    let sessions: [ProjectSession]
}

struct Cron: Identifiable, Equatable {
    let id: String
    let wsId: String
    let title: String
    let cron: String
    let next: String
    let desc: String
    let enabled: Bool
}

struct ModelOption: Identifiable, Equatable {
    let id: String
    let name: String
    let desc: String
    let tag: String
    let color: Color
    /// Name with the "Lingxi-" prefix stripped (composer chip label).
    var shortName: String { name.replacingOccurrences(of: "Lingxi-", with: "") }
}

struct ModelRuntimeDetails: Identifiable, Equatable {
    let reference: String
    let providerId: String
    let providerLabel: String
    let displayName: String
    let modelId: String
    let description: String?
    let family: String?
    let status: String?
    let releaseDate: String?
    let lastUpdated: String?
    let knowledgeCutoff: String?
    let inputModalities: [String]
    let outputModalities: [String]
    let contextWindowTokens: UInt64?
    let maxInputTokens: UInt64?
    let maxOutputTokens: UInt64?
    let openWeights: Bool?
    let attachments: Bool?
    let temperatureControl: Bool?
    let pricing: ModelPricingDto?
    let capabilities: ModelCapabilitiesDto
    let reasoning: ReasoningControlSpecDto
    var supportsFastMode: Bool = false

    var id: String { reference }

    static func from(_ dto: ModelDetailsDto) -> ModelRuntimeDetails {
        ModelRuntimeDetails(
            reference: dto.reference,
            providerId: dto.providerId,
            providerLabel: dto.providerLabel,
            displayName: dto.displayName,
            modelId: dto.modelId,
            description: dto.description?.isEmpty == false ? dto.description : nil,
            family: dto.family?.isEmpty == false ? dto.family : nil,
            status: dto.status?.isEmpty == false ? dto.status : nil,
            releaseDate: dto.releaseDate?.isEmpty == false ? dto.releaseDate : nil,
            lastUpdated: dto.lastUpdated?.isEmpty == false ? dto.lastUpdated : nil,
            knowledgeCutoff: dto.knowledgeCutoff?.isEmpty == false ? dto.knowledgeCutoff : nil,
            inputModalities: dto.inputModalities,
            outputModalities: dto.outputModalities,
            contextWindowTokens: dto.contextWindowTokens,
            maxInputTokens: dto.maxInputTokens,
            maxOutputTokens: dto.maxOutputTokens,
            openWeights: dto.openWeights,
            attachments: dto.attachments,
            temperatureControl: dto.temperatureControl,
            pricing: dto.pricing,
            capabilities: dto.capabilities,
            reasoning: dto.reasoning,
            supportsFastMode: dto.supportsFastMode
        )
    }

    var preferredDisplayName: String? {
        displayName.isEmpty ? nil : displayName
    }

    var preferredProviderLabel: String? {
        providerLabel.isEmpty ? nil : providerLabel
    }

    var summaryItems: [String] {
        var summary: [String] = []
        if capabilities.reasoning {
            let labels = reasoning.options.map { ModelDetailFormat.reasoningSelectionLabel($0.selection) }
            if !labels.isEmpty {
                summary.append("Thinking " + labels.joined(separator: "/"))
            }
        }
        let modalityBits = [
            capabilities.vision ? "图片" : nil,
            capabilities.documents ? "文档" : nil,
            capabilities.tools ? "工具" : nil,
            capabilities.structuredOutput ? "结构化输出" : nil,
        ].compactMap { $0 }
        if !modalityBits.isEmpty {
            summary.append(modalityBits.joined(separator: " · "))
        }
        if let contextWindowTokens {
            summary.append("\(ModelDetailFormat.tokenCount(contextWindowTokens)) 上下文")
        }
        if let maxOutputTokens {
            summary.append("\(ModelDetailFormat.tokenCount(maxOutputTokens)) 输出")
        }
        summary.append(ModelDetailFormat.pricingSummary(pricing))
        if let status, !status.isEmpty {
            summary.append(status.capitalized)
        }
        return summary.filter { !$0.isEmpty }
    }

    var searchableText: String {
        (
            [
                reference,
                providerId,
                providerLabel,
                displayName,
                modelId,
                description,
                family,
                status,
                releaseDate,
                lastUpdated,
                knowledgeCutoff,
                ModelDetailFormat.pricingSummary(pricing),
            ]
            + inputModalities
            + outputModalities
            + summaryItems
            + reasoning.options.map { ModelDetailFormat.reasoningSelectionLabel($0.selection) }
        )
            .compactMap { $0 }
            .joined(separator: " ")
    }
}

enum ModelDetailFormat {
    static func tokenCount(_ value: UInt64) -> String {
        let n = Double(value)
        if n >= 1_000_000 {
            return "\(trimDecimal(n / 1_000_000))M"
        }
        if n >= 1_000 {
            return "\(trimDecimal(n / 1_000))K"
        }
        return "\(value)"
    }

    static func price(_ value: Double) -> String {
        trimDecimal(value)
    }

    static func billingModeLabel(_ mode: ModelBillingModeDto) -> String {
        switch mode {
        case .perToken: return "按量计费"
        case .subscription: return "套餐/订阅内"
        case .free: return "免费"
        case .unknown: return "价格未提供"
        }
    }

    static func pricingSummary(_ pricing: ModelPricingDto?) -> String {
        guard let pricing else { return "" }
        switch pricing.billingMode {
        case .subscription, .free, .unknown:
            return billingModeLabel(pricing.billingMode)
        case .perToken:
            let parts = [
                pricing.inputPerMillion.map { "$\(price($0)) in" },
                pricing.outputPerMillion.map { "$\(price($0)) out" },
            ].compactMap { $0 }
            return parts.isEmpty ? "价格未提供" : parts.joined(separator: " · ")
        }
    }

    static func reasoningSelectionLabel(_ selection: ReasoningSelectionDto) -> String {
        switch selection {
        case .automatic: return "自动"
        case .disabled: return "关闭"
        case .enabled: return "开启"
        case let .level(id): return id
        case let .tokenBudget(tokens): return "\(tokenCount(tokens)) 预算"
        }
    }

    static func yesNo(_ value: Bool?) -> String? {
        guard let value else { return nil }
        return value ? "是" : "否"
    }

    private static func trimDecimal(_ value: Double) -> String {
        let rounded = (value * 100).rounded() / 100
        if rounded == rounded.rounded(.down) {
            return String(Int(rounded))
        }
        return String(rounded)
    }
}

/// One engine-provided model reference prepared for display.
///
/// `reference` remains byte-for-byte identical to `ModelList.models`, so picking
/// a row always submits the provider-qualified route (`provider/model`) rather
/// than losing provider identity. `modelId` is only the display-side suffix.
struct ModelCatalogItem: Identifiable, Equatable {
    let reference: String
    let providerId: String
    let modelId: String
    let details: ModelRuntimeDetails?

    var id: String { reference }
    var name: String { details?.preferredDisplayName ?? ModelDisplay.modelName(for: modelId) }
    var shortName: String { ModelDisplay.shortModelName(for: modelId) }
    var color: Color { ModelDisplay.providerColor(for: providerId) }
}

/// A stable Provider section derived only from the curated references supplied
/// by the engine. The UI never expands this with a Provider's full model catalog.
struct ModelProviderSection: Identifiable, Equatable {
    let providerId: String
    let name: String
    let models: [ModelCatalogItem]

    var id: String { providerId }
}

/// Friendly display for REAL, provider-qualified engine model references.
///
/// The engine owns the curated "latest and commonly used" shortlist. These
/// helpers only group and label that input; they never invent or append models.
enum ModelDisplay {
    private static let unqualifiedProviderId = "other"

    /// Group the exact engine input by provider while preserving its order.
    /// Duplicate references are ignored so every SwiftUI row has stable identity.
    static func sections(
        for references: [String],
        detailsByReference: [String: ModelRuntimeDetails] = [:]
    ) -> [ModelProviderSection] {
        var providerOrder: [String] = []
        var itemsByProvider: [String: [ModelCatalogItem]] = [:]
        var seenReferences = Set<String>()

        for reference in references where !reference.isEmpty {
            guard seenReferences.insert(reference).inserted else { continue }
            let item = item(for: reference, detailsByReference: detailsByReference)
            if itemsByProvider[item.providerId] == nil {
                providerOrder.append(item.providerId)
            }
            itemsByProvider[item.providerId, default: []].append(item)
        }

        return providerOrder.map { providerId in
            ModelProviderSection(
                providerId: providerId,
                name: providerName(for: providerId),
                models: itemsByProvider[providerId, default: []])
        }
    }

    /// Narrow the engine's references to those matching `query`, preserving the
    /// engine's order so `sections(for:)` still groups them exactly as before.
    ///
    /// The mobile sibling of Android's `EngineModelCatalog.filter`
    /// (`clients/android/.../model/Models.kt`): a blank query matches
    /// everything, otherwise it is a case-insensitive substring test against the
    /// friendly name, the wire id, the provider's display name, and the full
    /// qualified reference. Android also searches a per-model `desc` and a
    /// capability blurb; iOS's `ModelCatalogItem` carries neither, and inventing
    /// a metadata table just to match that list would be a far larger change
    /// than the search box justifies.
    static func filter(
        _ references: [String],
        matching query: String,
        detailsByReference: [String: ModelRuntimeDetails] = [:]
    ) -> [String] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !needle.isEmpty else { return references }
        return references.filter { reference in
            let item = item(for: reference, detailsByReference: detailsByReference)
            return [
                item.name,
                item.modelId,
                item.details?.preferredProviderLabel ?? providerName(for: item.providerId),
                reference,
                item.details?.searchableText,
            ].compactMap { $0 }.contains { $0.lowercased().contains(needle) }
        }
    }

    /// Parse only the first slash: aggregator model ids may themselves contain
    /// slashes (`openrouter/openai/gpt-5.5`).
    static func item(
        for reference: String,
        detailsByReference: [String: ModelRuntimeDetails] = [:]
    ) -> ModelCatalogItem {
        guard let slash = reference.firstIndex(of: "/"),
              slash != reference.startIndex,
              reference.index(after: slash) != reference.endIndex else {
            return ModelCatalogItem(
                reference: reference,
                providerId: unqualifiedProviderId,
                modelId: reference,
                details: detailsByReference[reference])
        }
        return ModelCatalogItem(
            reference: reference,
            providerId: String(reference[..<slash]),
            modelId: String(reference[reference.index(after: slash)...]),
            details: detailsByReference[reference])
    }

    static func providerName(for providerId: String) -> String {
        switch providerId.lowercased() {
        case "anthropic": return "Anthropic"
        case "builtin": return "Anthropic (Built-in)"
        case "openai": return "OpenAI"
        case "openai-chatgpt": return "OpenAI (ChatGPT)"
        case "deepseek": return "DeepSeek"
        case "kimi": return "Kimi"
        case "kimi-code": return "Kimi Code"
        case "gemini": return "Google Gemini"
        case "github-copilot": return "GitHub Copilot"
        case "zai": return "Z.AI"
        case "glm-coding": return "GLM Coding Plan"
        case "openrouter": return "OpenRouter"
        case unqualifiedProviderId: return "其他"
        default:
            return providerId
                .split(separator: "-", omittingEmptySubsequences: true)
                .map { $0.prefix(1).uppercased() + String($0.dropFirst()) }
                .joined(separator: " ")
        }
    }

    /// A human-friendly fallback name when the engine did not provide one.
    static func modelName(for modelId: String) -> String {
        let displayId = modelId.split(separator: "/").last.map(String.init) ?? modelId
        if displayId.lowercased().hasPrefix("gpt-") {
            let remainder = String(displayId.dropFirst(4))
            return "GPT-" + remainder
                .split(whereSeparator: { $0 == "_" || $0 == "-" })
                .enumerated()
                .map { index, segment in
                    if index == 0 { return String(segment) }
                    return segment.prefix(1).uppercased() + String(segment.dropFirst())
                }
                .joined(separator: " ")
        }
        return displayId
            .split(whereSeparator: { $0 == "-" || $0 == "_" })
            .map { segment in
                switch segment.lowercased() {
                case "gpt": return "GPT"
                case "glm": return "GLM"
                case "deepseek": return "DeepSeek"
                case "kimi": return "Kimi"
                case "gemini": return "Gemini"
                case "claude": return "Claude"
                case "openai": return "OpenAI"
                default:
                    if segment.allSatisfy({ $0.isNumber || $0 == "." }) {
                        return String(segment)
                    }
                    return segment.prefix(1).uppercased() + String(segment.dropFirst())
                }
            }
            .joined(separator: " ")
    }

    /// A human-friendly full name for a qualified reference.
    static func name(for reference: String) -> String {
        item(for: reference).name
    }

    /// A compact chip label. Parsing the reference first prevents the provider
    /// prefix from leaking into the chip while retaining the full route in state.
    static func shortName(for reference: String) -> String {
        item(for: reference).shortName
    }

    fileprivate static func shortModelName(for modelId: String) -> String {
        let displayId = modelId.split(separator: "/").last.map(String.init) ?? modelId
        let l = displayId.lowercased()
        if l.contains("opus") { return "Opus" }
        if l.contains("sonnet") { return "Sonnet" }
        if l.contains("haiku") { return "Haiku" }
        if l.hasPrefix("gpt-") { return modelName(for: displayId) }
        if l.hasPrefix("gemini-") { return modelName(for: displayId).replacingOccurrences(of: "Gemini ", with: "") }
        if l.hasPrefix("deepseek-") { return modelName(for: displayId).replacingOccurrences(of: "DeepSeek ", with: "") }
        if l.hasPrefix("kimi-") { return modelName(for: displayId).replacingOccurrences(of: "Kimi ", with: "") }
        if l.hasPrefix("glm-") { return modelName(for: displayId) }
        return displayId.isEmpty ? "默认" : displayId
    }

    /// A deterministic Provider color (Swift's `hashValue` is process-randomized,
    /// so it is unsuitable for stable cross-launch UI).
    static func providerColor(for providerId: String) -> Color {
        switch providerId.lowercased() {
        case "anthropic", "builtin":
            return Color(srgb: 0.9351, 0.5079, 0.4015)
        case "openai", "openai-chatgpt":
            return Color(srgb: 0.1326, 0.7261, 0.5350)
        case "deepseek":
            return Color(srgb: 0.6451, 0.5662, 1.0000)
        case "kimi":
            return Color(srgb: 0.4340, 0.5865, 1.0000)
        case "kimi-code":
            return Color(srgb: 0.2784, 0.6980, 0.9490)
        case "gemini":
            return Color(srgb: 0.3503, 0.6649, 0.9741)
        case "github-copilot":
            return Color(srgb: 0.5728, 0.6177, 0.7466)
        case "zai", "glm-coding":
            return Color(srgb: 0.0000, 0.7601, 0.7664)
        default:
            return Color(srgb: 0.4340, 0.5865, 1.0000)
        }
    }

    static func color(for reference: String) -> Color {
        item(for: reference).color
    }
}

enum Role { case user, ai }

struct MessageImage: Identifiable, Equatable {
    let id: UUID
    let mediaType: String
    let url: String

    init(id: UUID = UUID(), mediaType: String, url: String) {
        self.id = id
        self.mediaType = mediaType
        self.url = url
    }
}

struct Message: Identifiable, Equatable {
    let id: UUID
    let role: Role
    var tag: String? = nil
    let text: String
    let images: [MessageImage]

    var loopWakeupStreak: UInt32? = nil
    var loopFoldedItemIDs: Set<String> = []

    init(id: UUID = UUID(), role: Role, tag: String? = nil, text: String, images: [MessageImage] = []) {
        self.id = id
        self.role = role
        self.tag = tag
        self.text = text
        self.images = images
    }
}

/// A unified "session" reference used by ChatView's title bar.
struct SessionRef: Identifiable, Equatable {
    let id: String
    let title: String
}

/// One REAL resumable session from the engine — the UI projection of the
/// protocol `SessionRowDto` (`client-protocol/src/listings.rs`). The engine
/// enumerates `~/.claude` JSONL sessions and lowers each to a row; the
/// conversation source maps `SessionRowDto` → this model on the out-of-band
/// `SessionList` event path (the analog of how `ModelList` rides a separate
/// state path, NOT a per-turn delta).
///
/// `id` is the session UUID (also the `ResumeSession` target). `relativeTime`
/// is derived from the row's RFC 3339 `modified` so the drawer shows "2 小时前"
/// rather than a raw timestamp.
struct EngineSession: Identifiable, Equatable {
    /// The session UUID — `SessionRowDto.uuid`; the `ResumeSession` target id.
    let id: String
    /// The shared catalog title (≤ 200 Unicode characters); the drawer may
    /// truncate it visually without changing the UUID-backed identity.
    let title: String
    /// Session capability profile used for Chat/Code workspace filtering.
    let mode: SessionMode
    /// Number of visible user/assistant messages (`SessionRowDto.message_count`).
    let messageCount: Int
    /// Absolute modified time used for deterministic workspace/session sorting.
    let modifiedAt: Date?
    /// A short, relative "time ago" string derived from `modified_rfc3339`.
    let relativeTime: String

    /// A `SessionRef` for ChatView's title bar (so a resumed session shows its
    /// real title even though the transcript itself is an engine follow-up).
    var ref: SessionRef { SessionRef(id: id, title: title) }
}

/// Formats an RFC 3339 timestamp into a short, relative Chinese "time ago"
/// string (e.g. "刚刚", "2 小时前", "昨天") for the drawer's session rows. A
/// parse failure falls back to the raw string so a row is never blank.
enum RelativeTime {
    private static let parser: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
    private static let parserNoFraction: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()

    /// `rfc3339` → a short relative label. Unparseable input returns the raw
    /// string (trimmed) so the row degrades gracefully rather than going blank.
    static func format(_ rfc3339: String, now: Date = Date()) -> String {
        let date = parse(rfc3339)
        guard let date else {
            return rfc3339.isEmpty ? "—" : rfc3339
        }
        let secs = now.timeIntervalSince(date)
        if secs < 60 { return "刚刚" }
        if secs < 3600 { return "\(Int(secs / 60)) 分钟前" }
        if secs < 86_400 { return "\(Int(secs / 3600)) 小时前" }
        let days = Int(secs / 86_400)
        if days == 1 { return "昨天" }
        if days < 7 { return "\(days) 天前" }
        let fmt = DateFormatter()
        fmt.locale = Locale(identifier: "zh_CN")
        fmt.dateFormat = "M月d日"
        return fmt.string(from: date)
    }

    static func parse(_ rfc3339: String) -> Date? {
        parser.date(from: rfc3339) ?? parserNoFraction.date(from: rfc3339)
    }
}

// MARK: - Mock data ---------------------------------------------------------

enum MockData {
    static let workspaces: [Workspace] = [
        .init(id: "personal", name: "个人", icon: "◐", color: Color(srgb: 0.4340, 0.5865, 1.0000)), // oklch(70% 0.18 268)
        .init(id: "work",     name: "工作", icon: "◑", color: Color(srgb: 0.0000, 0.7601, 0.7664)), // oklch(70% 0.16 195)
        .init(id: "research", name: "研究", icon: "◒", color: Color(srgb: 0.8090, 0.4552, 0.8891)), // oklch(72% 0.18 320)
        .init(id: "creative", name: "创作", icon: "◓", color: Color(srgb: 0.8696, 0.5765, 0.0000)), // oklch(72% 0.16 75)
    ]

    static let chats: [Chat] = [
        .init(id: "c1", wsId: "work", title: "重装 Claude Code", group: "今天", preview: "nvm 残留清理完成", activity: "2 小时前"),
        .init(id: "c2", wsId: "work", title: "客户邮件回复模板", group: "昨天", preview: "已生成 4 套话术", activity: "昨天"),
        .init(id: "c3", wsId: "work", title: "上海差旅规划", group: "本周", preview: "机酒路线", activity: "周二"),
    ]

    static let projects: [Project] = [
        .init(id: "p1", wsId: "work", name: "灵犀 OS 设计", icon: "◑",
              color: Color(srgb: 0.4340, 0.5865, 1.0000), // oklch(70% 0.18 268)
              desc: "多端 UI · 14 文件 · 8 记忆",
              sessions: [
                .init(id: "s1",   title: "设计灵犀 iPhone 版", activity: "刚刚",   preview: "类 Claude 移动端布局", pinned: true, msgs: 8),
                .init(id: "p1s2", title: "iPad 横屏推演",      activity: "昨天",   preview: "Pencil 标注入口", msgs: 14),
                .init(id: "p1s3", title: "深色色板校准",        activity: "5月3日", preview: "oklch 节点对齐", msgs: 22),
              ]),
        .init(id: "p2", wsId: "work", name: "Q2 OKR & 周报", icon: "◐",
              color: Color(srgb: 0.0000, 0.7601, 0.7664), // oklch(72% 0.16 195)
              desc: "目标对齐 · 6 文件 · 3 记忆",
              sessions: [
                .init(id: "p2s1", title: "整理 Q2 OKR 草案", activity: "5 小时前", preview: "已对齐三方", msgs: 24),
                .init(id: "p2s2", title: "周报自动化模板",   activity: "昨天",     preview: "从多源聚合", msgs: 6),
              ]),
        .init(id: "p3", wsId: "work", name: "Code & 工程", icon: "◇",
              color: Color(srgb: 0.2085, 0.7571, 0.4656), // oklch(72% 0.16 155)
              desc: "Bug 排查 · 23 文件 · 12 记忆",
              sessions: [
                .init(id: "p3s1", title: "WebSocket 重连排查", activity: "昨天", preview: "指数退避方案", msgs: 31),
                .init(id: "p3s2", title: "PRD v2 评审反馈",   activity: "周一", preview: "12 评论 4 待办", msgs: 18),
              ]),
    ]

    static let crons: [Cron] = [
        .init(id: "cr1", wsId: "work", title: "每日晨报",     cron: "工作日 08:30", next: "明早 08:30",  desc: "聚合 Linear/GitHub/邮件 → 早会摘要", enabled: true),
        .init(id: "cr2", wsId: "work", title: "周报自动生成", cron: "每周五 17:00", next: "周五 17:00",  desc: "git 提交 + 日历 → 周报草稿", enabled: true),
        .init(id: "cr3", wsId: "work", title: "客户反馈周聚合", cron: "每周一 09:00", next: "下周一 09:00", desc: "7 天工单聚类 + 情感分析", enabled: true),
        .init(id: "cr4", wsId: "work", title: "凌晨日志巡检", cron: "每日 03:00",   next: "— 已暂停",     desc: "错误日志分类 + 告警", enabled: false),
    ]

    static let models: [ModelOption] = [
        .init(id: "lx-72b",   name: "Lingxi-72B",   desc: "主力", tag: "默认", color: Color(srgb: 0.4340, 0.5865, 1.0000)),
        .init(id: "lx-72b-r", name: "Lingxi-72B-R", desc: "推理", tag: "慢",   color: Color(srgb: 0.8090, 0.4552, 0.8891)),
        .init(id: "lx-32b",   name: "Lingxi-32B",   desc: "高速", tag: "快",   color: Color(srgb: 0.0000, 0.7601, 0.7664)),
        .init(id: "lx-code",  name: "Lingxi-Code",  desc: "代码", tag: "编程", color: Color(srgb: 0.2085, 0.7571, 0.4656)),
    ]

    static let messagesDefault: [Message] = [
        .init(role: .user, text: "帮我做一版手机上的灵犀 AI 助手，参考 Claude iOS 应用的极简风格，但要保留多 workspace 和工作流的能力。"),
        .init(role: .ai, tag: "思考了 48 秒", text: "已完成。\n\n**iPhone 版本设计要点**：\n\n1. **主界面 = 对话**。打开即进入最近会话，没有冗余首页。\n2. **左滑/汉堡 → 抽屉**，包含 workspace pill、session 列表、知识库、设置。\n3. **顶部 chip 显示工作流进度**，一行可滑动，与 Mac/iPad 一致。\n4. **底部胶囊 composer**，按住录音、点附件出 sheet。\n\n点击左上角菜单试试抽屉。"),
        .init(role: .user, text: "能不能加个语音\"心流\"模式？随时按住屏幕说话，松开发送。"),
        .init(role: .ai, tag: "思考了 12 秒", text: "已加。**按住屏幕任意位置 0.6 秒**会进入沉浸录音态：背景虚化，中央波形脉动，松开立即发送给当前模型。键盘/composer 临时隐藏。\n\n再次按住录音时，AI 的上一条回复会变为半透明，提示\"上下文已记入\"。"),
    ]

    /// Flattened session lookup (chats + every project session).
    static var allSessions: [SessionRef] {
        var refs = chats.map { SessionRef(id: $0.id, title: $0.title) }
        for p in projects {
            refs.append(contentsOf: p.sessions.map { SessionRef(id: $0.id, title: $0.title) })
        }
        return refs
    }

    static func session(_ id: String) -> SessionRef {
        allSessions.first(where: { $0.id == id }) ?? allSessions[0]
    }

    /// The default active session id — the FIRST available session, derived from
    /// real data rather than a hardcoded literal. RootView seeds `activeSession`
    /// with this (and falls back to it via `session(_:)` for any unknown id).
    static var defaultSessionId: String { allSessions.first?.id ?? "" }
}
