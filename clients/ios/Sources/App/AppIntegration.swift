import AppIntents
import Foundation

enum LingxiAppAction: Codable, Equatable, Sendable {
    case openApp
    case newConversation
    case ask(String)
    case openTerminal(sessionID: String, initialCommand: String?)
}

enum LingxiDeepLink {
    static let maximumCommandLength = 32_768

    /// Matches Android's public `lingxi://open_terminal` route. Unknown hosts,
    /// non-Lingxi schemes and oversized commands are rejected fail-closed.
    static func action(from url: URL) -> LingxiAppAction? {
        guard
            url.scheme?.lowercased() == "lingxi",
            url.host?.lowercased() == "open_terminal",
            let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        else { return nil }

        let values = Dictionary(
            components.queryItems?.map { ($0.name, $0.value ?? "") } ?? [],
            uniquingKeysWith: { _, newest in newest }
        )
        let sessionID = values["sessionId"]?
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .prefix(256)
        let command = values["initCommand"]?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard command?.count ?? 0 <= maximumCommandLength else { return nil }
        return .openTerminal(
            sessionID: sessionID.map(String.init).flatMap { $0.isEmpty ? nil : $0 } ?? "interactive",
            initialCommand: command.flatMap { $0.isEmpty ? nil : $0 }
        )
    }
}

extension Notification.Name {
    static let lingxiAppActionPending = Notification.Name("LingxiAppActionPending")
}

/// Durable hand-off between an App Intent and the SwiftUI scene. App Intents may
/// execute before the scene is active, so an in-memory notification alone is not
/// sufficient. The queue survives that process/scene boundary and drains once.
actor LingxiAppActionStore {
    static let shared = LingxiAppActionStore()

    private let defaults: UserDefaults
    private let storageKey = "app-integration.pending-actions.v1"

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func enqueue(_ action: LingxiAppAction) {
        var actions = load()
        actions.append(action)
        if let data = try? JSONEncoder().encode(actions) {
            defaults.set(data, forKey: storageKey)
        }
        Task { @MainActor in
            NotificationCenter.default.post(name: .lingxiAppActionPending, object: nil)
        }
    }

    func drain() -> [LingxiAppAction] {
        let actions = load()
        defaults.removeObject(forKey: storageKey)
        return actions
    }

    private func load() -> [LingxiAppAction] {
        guard
            let data = defaults.data(forKey: storageKey),
            let actions = try? JSONDecoder().decode([LingxiAppAction].self, from: data)
        else { return [] }
        return actions
    }
}

struct OpenLingxiIntent: AppIntent {
    static let title: LocalizedStringResource = "打开灵犀"
    static let description = IntentDescription("打开灵犀并回到当前会话。")
    static let openAppWhenRun = true

    func perform() async throws -> some IntentResult {
        await LingxiAppActionStore.shared.enqueue(.openApp)
        return .result()
    }
}

struct NewLingxiConversationIntent: AppIntent {
    static let title: LocalizedStringResource = "新建灵犀对话"
    static let description = IntentDescription("在当前项目中新建一个灵犀对话。")
    static let openAppWhenRun = true

    func perform() async throws -> some IntentResult {
        await LingxiAppActionStore.shared.enqueue(.newConversation)
        return .result()
    }
}

struct AskLingxiIntent: AppIntent {
    static let title: LocalizedStringResource = "向灵犀提问"
    static let description = IntentDescription("把来自 Siri、快捷指令或其他 App 的文本带入新对话。")
    static let openAppWhenRun = true

    @Parameter(title: "问题")
    var question: String

    static var parameterSummary: some ParameterSummary {
        Summary("向灵犀提问 \(\.$question)")
    }

    func perform() async throws -> some IntentResult {
        await LingxiAppActionStore.shared.enqueue(.ask(question))
        return .result()
    }
}

struct OpenLingxiTerminalIntent: AppIntent {
    static let title: LocalizedStringResource = "打开灵犀终端"
    static let description = IntentDescription("打开当前项目的终端，可选择预填一条命令。")
    static let openAppWhenRun = true

    @Parameter(title: "预填命令", default: "")
    var initialCommand: String

    static var parameterSummary: some ParameterSummary {
        Summary("打开灵犀终端，预填 \(\.$initialCommand)")
    }

    func perform() async throws -> some IntentResult {
        let trimmed = initialCommand.trimmingCharacters(in: .whitespacesAndNewlines)
        let bounded = String(trimmed.prefix(LingxiDeepLink.maximumCommandLength))
        await LingxiAppActionStore.shared.enqueue(
            .openTerminal(
                sessionID: "interactive",
                initialCommand: bounded.isEmpty ? nil : bounded
            )
        )
        return .result()
    }
}

struct LingxiAppShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: OpenLingxiIntent(),
            phrases: ["打开 \(.applicationName)"],
            shortTitle: "打开灵犀",
            systemImageName: "sparkles"
        )
        AppShortcut(
            intent: NewLingxiConversationIntent(),
            phrases: ["在 \(.applicationName) 新建对话"],
            shortTitle: "新建对话",
            systemImageName: "square.and.pencil"
        )
        AppShortcut(
            intent: AskLingxiIntent(),
            phrases: ["用 \(.applicationName) 提问", "让 \(.applicationName) 回答"],
            shortTitle: "向灵犀提问",
            systemImageName: "text.bubble"
        )
        AppShortcut(
            intent: OpenLingxiTerminalIntent(),
            phrases: ["打开 \(.applicationName) 终端"],
            shortTitle: "打开终端",
            systemImageName: "terminal"
        )
    }

    static let shortcutTileColor: ShortcutTileColor = .purple
}
