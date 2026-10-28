import Foundation

enum SessionMode: String, CaseIterable, Codable, Hashable, Sendable {
    case chat
    case code
}

/// An applied event may reuse the pending request's revision. Once applied,
/// a delayed copy of that pending event must not roll the UI backwards.
func acceptsSessionModeRevision(previous: UInt64?, previousPending: SessionMode?, incoming: UInt64, incomingPending: SessionMode?) -> Bool {
    guard let previous else { return true }
    return incoming > previous || (incoming == previous && (previousPending != nil || incomingPending == nil))
}

/// Tracks the first prompt's required mode across asynchronous submissions and
/// ordered engine events. A later selection cannot be confirmed by an older
/// request's event, even when both selections name the same mode.
struct NewSessionModeReadiness {
    private struct Request {
        let id: UInt64
        var sessionID: String
        let mode: SessionMode
        var submitted = false
        var confirmed = false
    }
    private struct Observation {
        let requestID: UInt64
        let epoch: UInt64
        let sessionID: String
        let mode: SessionMode
    }
    private var nextID: UInt64 = 0
    private var required: Request?
    private var unobserved: [Observation] = []

    mutating func request(_ mode: SessionMode, sessionID: String) -> UInt64 {
        nextID &+= 1
        required = Request(id: nextID, sessionID: sessionID, mode: mode)
        return nextID
    }

    mutating func beginSubmission(_ id: UInt64, epoch: UInt64) -> Bool {
        guard let required, required.id == id else { return false }
        unobserved.append(Observation(requestID: id, epoch: epoch, sessionID: required.sessionID, mode: required.mode))
        return true
    }

    mutating func submitted(_ id: UInt64) {
        guard var request = required, request.id == id else { return }
        request.submitted = true
        required = request
    }

    /// Returns true only for the currently required request.
    mutating func failed(_ id: UInt64) -> Bool {
        unobserved.removeAll { $0.requestID == id }
        return required?.id == id
    }

    mutating func observe(sessionID: String, mode: SessionMode, pending: SessionMode?, epoch: UInt64) {
        let selected = pending ?? mode
        guard let index = unobserved.firstIndex(where: {
            $0.epoch == epoch && $0.mode == selected && ($0.sessionID.isEmpty || $0.sessionID == sessionID)
        }) else { return }
        let observed = unobserved[index]
        // A pending selection may only apply later. A superseded pending
        // selection will not apply, so it cannot consume a later snapshot.
        if pending == nil || required?.id != observed.requestID {
            unobserved.remove(at: index)
        }
        guard var request = required, request.id == observed.requestID,
              request.sessionID.isEmpty || request.sessionID == sessionID,
              request.mode == mode, pending == nil else { return }
        request.sessionID = sessionID
        request.confirmed = true
        required = request
    }

    mutating func takeReady(sessionID: String, mode: SessionMode, pending: SessionMode?) -> Bool {
        guard let required, required.submitted, required.confirmed,
              required.sessionID == sessionID, required.mode == mode, pending == nil else { return false }
        self.required = nil
        // The accepted snapshot supersedes any older pending selections.
        unobserved.removeAll()
        return true
    }

    mutating func reset() {
        required = nil
        unobserved.removeAll()
    }
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
    case remote(hostID: String, sessionID: String)

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
        case let .remote(hostID, sessionID): "remote.\(hostID).\(sessionID)"
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
        case let .remote(hostID, sessionID): "remote.\(hostID).\(sessionID)"
        }
    }
}

func reusesConversationSource(currentScope: ConversationScope, targetScope: ConversationScope,
                              currentMode: SessionMode, targetMode: SessionMode,
                              startsNew: Bool, resumesSession: Bool) -> Bool {
    currentScope == targetScope && (currentMode == targetMode || startsNew || resumesSession)
}
