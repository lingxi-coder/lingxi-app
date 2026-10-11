import AppIntents
import Foundation

enum LingxiAppAction: Codable, Equatable, Sendable {
    case openApp
    case newConversation
    case ask(String)
    case openConversation(
        sessionID: String,
        turnID: UInt64?,
        workspaceKey: String?,
        mode: SessionMode
    )
    case openTerminal(sessionID: String, initialCommand: String?)
}

enum LingxiDeepLink {
    static let maximumCommandLength = 32_768

    /// Matches Android's public `lingxi://open_terminal` route. Unknown hosts,
    /// non-Lingxi schemes and oversized commands are rejected fail-closed.
    static func action(from url: URL) -> LingxiAppAction? {
        guard
            url.scheme?.lowercased() == "lingxi",
            let host = url.host?.lowercased(),
            let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        else { return nil }

        let items = components.queryItems ?? []
        let values = Dictionary(
            items.map { ($0.name, $0.value ?? "") },
            uniquingKeysWith: { _, newest in newest }
        )

        switch host {
        case "open_conversation":
            let sessionID = values["sessionId"]?
                .trimmingCharacters(in: .whitespacesAndNewlines)
                .prefix(256)
            guard let sessionID, !sessionID.isEmpty else { return nil }
            let turnID = values["turnId"].flatMap { UInt64($0.trimmingCharacters(in: .whitespacesAndNewlines)) }
            let workspaceKey = values["workspaceKey"]?
                .trimmingCharacters(in: .whitespacesAndNewlines)
                .nilIfEmpty
            let mode = values["sessionMode"].flatMap(SessionMode.init(rawValue:)) ?? .code
            return .openConversation(
                sessionID: String(sessionID),
                turnID: turnID,
                workspaceKey: workspaceKey,
                mode: mode
            )
        case "open_terminal":
            let sessionID = values["sessionId"]?
                .trimmingCharacters(in: .whitespacesAndNewlines)
                .prefix(256)
            let command = values["initCommand"]?.trimmingCharacters(in: .whitespacesAndNewlines)
            guard command?.count ?? 0 <= maximumCommandLength else { return nil }
            return .openTerminal(
                sessionID: sessionID.map(String.init).flatMap { $0.isEmpty ? nil : $0 } ?? "interactive",
                initialCommand: command.flatMap { $0.isEmpty ? nil : $0 }
            )
        default:
            return nil
        }
    }

    static func conversationURL(
        sessionID: String,
        turnID: UInt64?,
        workspaceKey: String? = nil,
        mode: SessionMode = .code
    ) -> URL? {
        ConversationDeepLink.makeURL(
            sessionID: sessionID,
            turnID: turnID,
            workspaceKey: workspaceKey,
            sessionMode: mode.rawValue
        )
    }
}

private extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}

extension Notification.Name {
    static let lingxiAppActionPending = Notification.Name("LingxiAppActionPending")
    static let lingxiConversationNotificationOpened =
        Notification.Name("LingxiConversationNotificationOpened")
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
    static let title: LocalizedStringResource = "app_intent_open_title"
    static let description = IntentDescription("app_intent_open_description")
    static let openAppWhenRun = true

    func perform() async throws -> some IntentResult {
        await LingxiAppActionStore.shared.enqueue(.openApp)
        return .result()
    }
}

struct NewLingxiConversationIntent: AppIntent {
    static let title: LocalizedStringResource = "app_intent_new_conversation_title"
    static let description = IntentDescription("app_intent_new_conversation_description")
    static let openAppWhenRun = true

    func perform() async throws -> some IntentResult {
        await LingxiAppActionStore.shared.enqueue(.newConversation)
        return .result()
    }
}

struct AskLingxiIntent: AppIntent {
    static let title: LocalizedStringResource = "app_intent_ask_title"
    static let description = IntentDescription("app_intent_ask_description")
    static let openAppWhenRun = true

    @Parameter(title: "app_intent_question_param_title")
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
    static let title: LocalizedStringResource = "app_intent_open_terminal_title"
    static let description = IntentDescription("app_intent_open_terminal_description")
    static let openAppWhenRun = true

    @Parameter(title: "app_intent_initial_command_param_title", default: "")
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
            shortTitle: "app_intent_open_title",
            systemImageName: "sparkles"
        )
        AppShortcut(
            intent: NewLingxiConversationIntent(),
            phrases: ["在 \(.applicationName) 新建对话"],
            shortTitle: "settings_app_new_conversation",
            systemImageName: "square.and.pencil"
        )
        AppShortcut(
            intent: AskLingxiIntent(),
            phrases: ["用 \(.applicationName) 提问", "让 \(.applicationName) 回答"],
            shortTitle: "app_intent_ask_title",
            systemImageName: "text.bubble"
        )
        AppShortcut(
            intent: OpenLingxiTerminalIntent(),
            phrases: ["打开 \(.applicationName) 终端"],
            shortTitle: "settings_app_open_terminal",
            systemImageName: "terminal"
        )
    }

    static let shortcutTileColor: ShortcutTileColor = .purple
}
