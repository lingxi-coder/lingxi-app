import AppIntents
import Foundation

enum LingxiAppAction: Codable, Equatable, Sendable {
    case openApp
    case newConversation
    case ask(String)
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
    }

    static let shortcutTileColor: ShortcutTileColor = .purple
}
