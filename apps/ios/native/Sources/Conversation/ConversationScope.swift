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

/// The workspace a conversation runs in: the global shell workspace, the
/// managed no-project scheduled workspace, or a managed project.
enum ConversationScope: Equatable, Hashable, Sendable {
    case global
    case scheduled
    case project(String)

    /// The legacy optional-projectID spelling (`nil` == global).
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
        } else {
            return nil
        }
    }

    var projectID: String? {
        if case let .project(id) = self { return id }
        return nil
    }

    var workspaceKey: String {
        switch self {
        case .global: "global"
        case .scheduled: "scheduled"
        case let .project(id): "project.\(id)"
        }
    }

    /// The middle segment of a `ProjectScopedPreferences` key. `.global` and
    /// `.project` MUST keep producing exactly the strings the pre-scope code
    /// produced ("global" / the lowercased project id) so existing user state
    /// survives.
    var preferenceScope: String {
        switch self {
        case .global: "global"
        case .scheduled: "scheduled"
        case let .project(id): id.lowercased()
        }
    }
}
