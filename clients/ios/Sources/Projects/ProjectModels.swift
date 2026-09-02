import Foundation

enum ProjectStorageKind: String, Codable, Equatable, Sendable {
    case `internal`
    case externalBookmarkMirror = "external-bookmark-mirror"
}

enum ProjectSyncState: String, Codable, Equatable, Sendable {
    case localOnly
    case synced
    case changesPending
    case conflict
    case authorizationLost
    case syncing
    case error

    var label: String {
        switch self {
        case .localOnly: return String(localized: "project_sync_state_local")
        case .synced: return String(localized: "project_sync_state_synced")
        case .changesPending: return String(localized: "project_sync_state_pending")
        case .conflict: return String(localized: "project_sync_state_conflict")
        case .authorizationLost: return String(localized: "project_sync_state_auth_lost")
        case .syncing: return String(localized: "project_sync_state_syncing")
        case .error: return String(localized: "project_sync_state_error")
        }
    }
}

struct ProjectExternalBookmark: Codable, Equatable, Sendable {
    var data: Data
    var displayName: String?
    var pathHint: String?
    var isStale: Bool = false
}

struct ProjectRecord: Identifiable, Codable, Equatable, Sendable {
    var id: String
    var name: String
    var storageKind: ProjectStorageKind
    var createdAt: Date
    var updatedAt: Date
    var sourceBookmark: ProjectExternalBookmark? = nil
    var lastSyncAt: Date? = nil
    var lastActiveSessionId: String? = nil
    var syncState: ProjectSyncState

    init(
        id: String,
        name: String,
        storageKind: ProjectStorageKind,
        createdAt: Date,
        updatedAt: Date,
        sourceBookmark: ProjectExternalBookmark? = nil,
        lastSyncAt: Date? = nil,
        lastActiveSessionId: String? = nil,
        syncState: ProjectSyncState? = nil
    ) {
        self.id = id
        self.name = name
        self.storageKind = storageKind
        self.createdAt = createdAt
        self.updatedAt = updatedAt
        self.sourceBookmark = sourceBookmark
        self.lastSyncAt = lastSyncAt
        self.lastActiveSessionId = lastActiveSessionId
        self.syncState = syncState ?? (storageKind == .internal ? .localOnly : .changesPending)
    }
}

struct ProjectWorkspace: Equatable, Sendable {
    let projectId: String
    let hostURL: URL
    let guestPath: String

    init(projectId: String, hostURL: URL) {
        self.projectId = projectId
        self.hostURL = hostURL
        self.guestPath = LXISHGuestPaths.workspace(projectId)
    }
}

struct ProjectSessionSummary: Identifiable, Codable, Equatable, Sendable {
    var sessionId: String
    var title: String
    var messageCount: Int
    var relativeTime: String
    var updatedAt: Date
    var mode: SessionMode

    init(
        sessionId: String,
        title: String,
        messageCount: Int,
        relativeTime: String,
        updatedAt: Date,
        mode: SessionMode = .code
    ) {
        self.sessionId = sessionId
        self.title = title
        self.messageCount = messageCount
        self.relativeTime = relativeTime
        self.updatedAt = updatedAt
        self.mode = mode
    }

    enum CodingKeys: String, CodingKey {
        case sessionId
        case title
        case messageCount
        case relativeTime
        case updatedAt
        case mode
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        sessionId = try container.decode(String.self, forKey: .sessionId)
        title = try container.decode(String.self, forKey: .title)
        messageCount = try container.decode(Int.self, forKey: .messageCount)
        relativeTime = try container.decode(String.self, forKey: .relativeTime)
        updatedAt = try container.decode(Date.self, forKey: .updatedAt)
        mode = try container.decodeIfPresent(SessionMode.self, forKey: .mode) ?? .code
    }

    func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(sessionId, forKey: .sessionId)
        try container.encode(title, forKey: .title)
        try container.encode(messageCount, forKey: .messageCount)
        try container.encode(relativeTime, forKey: .relativeTime)
        try container.encode(updatedAt, forKey: .updatedAt)
        try container.encode(mode, forKey: .mode)
    }

    var id: String { sessionId }
    var modifiedAt: Date { updatedAt }
}

struct ProjectSyncFile: Codable, Equatable, Sendable {
    var relativePath: String
    var sha256: String
    var sizeBytes: Int64
}

struct ProjectSyncBaseline: Codable, Equatable, Sendable {
    var files: [String: ProjectSyncFile] = [:]
}

struct ProjectSyncConflict: Identifiable, Equatable, Sendable {
    var projectId: String
    var relativePath: String
    var internalSha256: String?
    var externalSha256: String?
    var baselineSha256: String?

    var id: String { "\(projectId):\(relativePath)" }
}

struct ProjectSnapshot: Identifiable, Equatable, Sendable {
    var record: ProjectRecord
    var workspace: ProjectWorkspace
    var sessions: [ProjectSessionSummary]

    var id: String { record.id }
}

enum ProjectOperationKind: String, Equatable, Sendable {
    case create
    case `import`
    case `switch`
    case refreshSessions
    case reimport
    case export
    case resolveConflicts
}

struct ProjectOperation: Equatable, Sendable {
    var kind: ProjectOperationKind
    var projectId: String? = nil
    var message: String
}

struct ProjectRepositoryState: Equatable, Sendable {
    var projects: [ProjectSnapshot] = []
    var activeProjectId: String? = nil
    var globalSessions: [ProjectSessionSummary] = []
    var loading: Bool = false
    var errorMessage: String? = nil

    var activeProject: ProjectSnapshot? {
        projects.first(where: { $0.record.id == activeProjectId })
    }
}

struct ProjectActiveSelectionRollback: Equatable, Sendable {
    var previousProjectId: String?
}

enum ProjectConflictResolution: String, Equatable, Sendable {
    case keepInternal
    case keepExternal
}

struct ProjectSyncResult: Equatable, Sendable {
    var project: ProjectSnapshot
    var conflicts: [ProjectSyncConflict]
    var copiedFiles: Int
    var skippedFiles: Int
}

enum ProjectSyncDirection: Sendable {
    case externalToInternal
    case internalToExternal
}

enum ProjectSyncAction: Sendable {
    case conflict
    case copyExternal
    case copyInternal
    case deleteExternal
    case deleteInternal
    case updateBaseline
    case removeBaseline
    case skip
}

func decideProjectSyncAction(
    direction: ProjectSyncDirection,
    resolution: ProjectConflictResolution?,
    baselineSha256: String?,
    externalSha256: String?,
    internalSha256: String?
) -> ProjectSyncAction {
    let hasBaseline = baselineSha256 != nil
    let externalChanged = externalSha256 != baselineSha256
    let internalChanged = internalSha256 != baselineSha256
    let bothChangedConflict =
        externalSha256 != nil &&
        internalSha256 != nil &&
        externalChanged &&
        internalChanged &&
        externalSha256 != internalSha256
    let deleteConflict =
        hasBaseline &&
        ((externalSha256 == nil && internalSha256 != nil && internalChanged) ||
         (internalSha256 == nil && externalSha256 != nil && externalChanged))

    if bothChangedConflict || deleteConflict {
        return switch (direction, resolution) {
        case (.externalToInternal, .some(.keepExternal)):
            externalSha256 == nil ? .deleteInternal : .copyExternal
        case (.internalToExternal, .some(.keepInternal)):
            internalSha256 == nil ? .deleteExternal : .copyInternal
        case (_, .none): .conflict
        default: .skip
        }
    }

    if externalSha256 != nil, externalSha256 == internalSha256 {
        return .updateBaseline
    }

    switch direction {
    case .externalToInternal:
        switch (externalSha256, internalSha256, baselineSha256, externalChanged, internalChanged) {
        case (.none, .none, .some, _, _):
            return .removeBaseline
        case (.none, .some, .some, _, false):
            return .deleteInternal
        case (.none, .some, .none, _, _):
            return .skip
        case (.none, .none, .none, _, _):
            return .skip
        case (.some, .none, _, _, _):
            return .copyExternal
        case (.some, .some, _, true, false):
            return .copyExternal
        case (.some, .some, _, false, true):
            return .skip
        default:
            return externalSha256 == internalSha256 ? .updateBaseline : .skip
        }
    case .internalToExternal:
        switch (internalSha256, externalSha256, baselineSha256, internalChanged, externalChanged) {
        case (.none, .none, .some, _, _):
            return .removeBaseline
        case (.none, .some, .some, _, false):
            return .deleteExternal
        case (.none, .some, .none, _, _):
            return .skip
        case (.none, .none, .none, _, _):
            return .skip
        case (.some, .none, _, _, _):
            return .copyInternal
        case (.some, .some, _, true, false):
            return .copyInternal
        case (.some, .some, _, false, true):
            return .skip
        default:
            return externalSha256 == internalSha256 ? .updateBaseline : .skip
        }
    }
}
