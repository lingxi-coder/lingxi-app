import Foundation

enum SessionMode: String, CaseIterable, Codable, Hashable, Sendable {
    case chat
    case code
}

#if canImport(harness_runtimeFFI)
    extension SessionMode {
        init(dto: SessionModeDto) {
            switch dto {
            case .chat: self = .chat
            case .code: self = .code
            }
        }

        var dto: SessionModeDto {
            switch self {
            case .chat: .chat
            case .code: .code
            }
        }
    }
#endif

/// The workspace a conversation runs in. Global and managed projects predate
/// this type; `.localApp` is v3's third scope — each local app is a
/// conversation scope of its own whose workspace directory is the session cwd.
enum ConversationScope: Equatable, Hashable, Sendable {
    case global
    case scheduled
    case project(String)
    case localApp(String)

    /// The legacy optional-projectID spelling (`nil` == global). `.localApp`
    /// deliberately has no projectID: an app conversation must never write
    /// into a project's session index or preferences.
    init(projectID: String?) {
        self = projectID.map(ConversationScope.project) ?? .global
    }

    init?(workspaceKey: String) {
        if workspaceKey == "global" {
            self = .global
        } else if workspaceKey == "scheduled" {
            self = .scheduled
        } else if workspaceKey.hasPrefix("project.") {
            let id = String(workspaceKey.dropFirst("project.".count))
            guard !id.isEmpty else { return nil }
            self = .project(id)
        } else if workspaceKey.hasPrefix("app.") {
            let id = String(workspaceKey.dropFirst("app.".count))
            guard !id.isEmpty else { return nil }
            self = .localApp(id)
        } else {
            return nil
        }
    }

    var projectID: String? {
        if case let .project(id) = self { return id }
        return nil
    }

    var appID: String? {
        if case let .localApp(id) = self { return id }
        return nil
    }

    var isLocalApp: Bool { appID != nil }

    var workspaceKey: String {
        switch self {
        case .global: "global"
        case .scheduled: "scheduled"
        case let .project(id): "project.\(id)"
        case let .localApp(id): "app.\(id)"
        }
    }

    /// The middle segment of a `ProjectScopedPreferences` key. `.global` and
    /// `.project` MUST keep producing exactly the strings the pre-scope code
    /// produced ("global" / the lowercased project id) so existing user state
    /// survives; `.localApp` only ADDS the `app.<id>` namespace.
    var preferenceScope: String {
        switch self {
        case .global: "global"
        case .scheduled: "scheduled"
        case let .project(id): id.lowercased()
        case let .localApp(id): "app.\(id.lowercased())"
        }
    }
}
