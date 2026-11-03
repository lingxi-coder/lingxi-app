import Foundation

enum LocalAppRequestPurpose: Sendable {
    case ordinary
    case development(SessionMode)
    case marketInstallation
}

enum LocalAppDevelopmentAdmission {
    static func requireCode(_ mode: SessionMode?) throws {
        guard mode == .code else {
            throw LocalAppServiceFailure("requiresCode", String(localized: "local_app_development_requires_code"))
        }
    }

    static func requireServiceRequest(_ fields: [String: LocalAppJSON], purpose: LocalAppRequestPurpose) throws {
        let operation = fields["op"]?.text ?? ""
        guard ["import", "publish", "prepareAuthoring"].contains(operation) else { return }
        switch purpose {
        case .development(let mode): try requireCode(mode)
        case .marketInstallation where operation != "prepareAuthoring" && fields["marketOrigin"] != nil: break
        default: try requireCode(nil)
        }
    }

    static func requiresCode(forAgentHostRequest request: String) -> Bool {
        guard let data = request.data(using: .utf8),
              let fields = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let op = fields["op"] as? String else { return false }
        return ["createAuthoring", "prepareAuthoringSource", "bindAuthoringSource", "registerAuthoring",
                "getAuthoringGrant", "restoreAuthoringSource", "remixAuthoringSource", "importRemixSource"].contains(op)
    }

    static func requireAgentHostRequest(_ request: String, mode: SessionMode?) throws {
        if requiresCode(forAgentHostRequest: request) { try requireCode(mode) }
    }
}

enum LocalAppToolFailurePresentation {
    private static let requiresCodeKey = "local_app_development_requires_code"

    static func requiresCode(tool: String, failed: Bool, headline: String?, body: String?) -> Bool {
        func hasCode(_ text: String?) -> Bool {
            guard var text = text?.trimmingCharacters(in: .whitespacesAndNewlines) else { return false }
            if text.hasPrefix("Error: ") { text.removeFirst("Error: ".count) }
            return text.hasPrefix(requiresCodeKey + ":")
        }
        return failed && tool.caseInsensitiveCompare("LocalApp") == .orderedSame
            && (hasCode(headline) || hasCode(body))
    }

    static var requiresCodeMessage: String { String(localized: "local_app_development_requires_code") }

    static func visibleText(tool: String, failed: Bool, headline: String?, body: String?, fallback: String?) -> String? {
        requiresCode(tool: tool, failed: failed, headline: headline, body: body) ? requiresCodeMessage : fallback
    }
}

/// Replay the durable native receipt first; only a proven missing archive may
/// trigger re-fetch of the same immutable Market release.
@MainActor
enum LocalAppRemixSubmission {
    static func run<T>(mode: SessionMode, submit: () async throws -> T,
                       restoreArchive: () async throws -> Void) async throws -> T {
        try LocalAppDevelopmentAdmission.requireCode(mode)
        do { return try await submit() }
        catch let failure as LocalAppServiceFailure where failure.code == "archiveMissing" {
            try await restoreArchive()
            return try await submit()
        }
    }
}

/// A confirmation always applies to the value the user selected, even if a
/// background refresh changes the page before they respond.
struct LocalAppRemixSelection<Value> {
    private var selected: Value?

    mutating func capture(_ value: Value) { selected = value }
    mutating func take() -> Value? {
        defer { selected = nil }
        return selected
    }
    mutating func clear() { selected = nil }
}

@MainActor
struct LocalAppActionPresentation {
    let mayPresent: () -> Bool
    let reportOwnerFailure: (String) -> Void

    func deliverFailure(_ message: String, showOnPage: (String) -> Void) {
        if mayPresent() { showOnPage(message) }
        else { reportOwnerFailure(message) }
    }
}

/// Values crossing the standalone service boundary. They never contain engine
/// handles, account credentials, or a currently selected conversation.
indirect enum LocalAppJSON: Codable, Equatable, Sendable {
    case object([String: LocalAppJSON]), array([LocalAppJSON]), string(String), number(Double), bool(Bool), null
    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode([String: LocalAppJSON].self) { self = .object(v) }
        else { self = .array(try c.decode([LocalAppJSON].self)) }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .object(let v): try c.encode(v)
        case .array(let v): try c.encode(v)
        case .string(let v): try c.encode(v)
        case .number(let v): try c.encode(v)
        case .bool(let v): try c.encode(v)
        case .null: try c.encodeNil()
        }
    }
    static func encode<T: Encodable>(_ value: T) throws -> Self {
        try JSONDecoder().decode(Self.self, from: JSONEncoder().encode(value))
    }
    var text: String? { if case .string(let value) = self { return value }; return nil }
}

struct LocalAppAuthoring: Codable, Equatable, Sendable {
    var sourceDirectory: String
    var workspaceId: String? = nil
    var projectRef: ProjectRef? = nil
    var authoringSessionId: String? = nil
    var appId: String? = nil
}

struct LocalAppAuthoringReceipt: Decodable, Sendable {
    let authoring: LocalAppAuthoring
    let sessionMode: SessionMode
    let sourceId: String
    let ownerAppId: String?

    private enum CodingKeys: String, CodingKey { case sessionMode, sourceId, ownerAppId }
    init(from decoder: Decoder) throws {
        authoring = try LocalAppAuthoring(from: decoder)
        let fields = try decoder.container(keyedBy: CodingKeys.self)
        sessionMode = try fields.decode(SessionMode.self, forKey: .sessionMode)
        sourceId = try fields.decode(String.self, forKey: .sourceId)
        guard UUID(uuidString: sourceId) != nil, canonicalSessionID(sourceId) == sourceId else {
            throw DecodingError.dataCorruptedError(forKey: .sourceId, in: fields,
                                                   debugDescription: "The source receipt has an invalid identity.")
        }
        ownerAppId = try fields.decodeIfPresent(String.self, forKey: .ownerAppId)
    }

    func matchesSourceID(_ expected: String?) -> Bool {
        guard let expected else { return true }
        return UUID(uuidString: expected) != nil && sourceId == canonicalSessionID(expected)
    }
}

/// A service-created source can be mounted once while its real session UUID
/// is being established. It is not an authoring binding until the native
/// service has persisted and subsequently confirmed the complete tuple.
struct LocalAppCreatedAuthoringSource: Decodable, Equatable, Sendable {
    let sourceId: String
    let requestId: String
    let sourceDirectory: String
    let sourcePath: String
}

struct LocalAppVerifiedAuthoringSource: Decodable, Equatable, Sendable {
    let sourceId: String
    let sourceDirectory: String
    let sourcePath: String
}

struct LocalAppBoundAuthoringSource: Decodable, Sendable {
    let requestId: String
    let sourceId: String
    let sourceDirectory: String
    let sourcePath: String
    let targetAppId: String
}

struct LocalAppAuthoringOwner: Equatable, Sendable {
    let sourceDirectory: String
    let projectID: String
    let sessionID: String
    let appID: String?

    func matches(_ binding: LocalAppAuthoring) -> Bool {
        guard UUID(uuidString: projectID) != nil, UUID(uuidString: sessionID) != nil,
              binding.sourceDirectory == sourceDirectory,
              let workspace = binding.workspaceId,
              case .mobileProject(let project)? = binding.projectRef,
              let session = binding.authoringSessionId,
              canonicalSessionID(workspace) == canonicalSessionID(projectID),
              canonicalSessionID(project) == canonicalSessionID(projectID),
              canonicalSessionID(session) == canonicalSessionID(sessionID)
        else { return false }
        // App ID is optional metadata: publishing may attach it later without
        // changing the immutable source/project/session owner receipt.
        return true
    }
}

struct LocalAppProvisionalAuthoringOwner: Equatable, Sendable {
    let sourceDirectory: String
    let projectID: String
    let appID: String?

    func matches(_ owner: LocalAppAuthoringOwner) -> Bool {
        sourceDirectory == owner.sourceDirectory
            && canonicalSessionID(projectID) == canonicalSessionID(owner.projectID)
            && appID == owner.appID
    }
}

/// Serializes current-pointer writes for one app. A newer user selection
/// invalidates an older ticket before it enters native code, while a write
/// already admitted finishes before the newer write is allowed to start.
@MainActor
final class LocalAppAuthoringRegistrationOrder {
    static let shared = LocalAppAuthoringRegistrationOrder()
    struct Ticket: Equatable, Sendable { let key: String; let revision: UInt64 }
    struct Lease: Equatable, Sendable { let key: String; let id: UUID }
    private var revisions: [String: UInt64] = [:]
    private var owners: [String: UUID] = [:]
    private var waiters: [String: [CheckedContinuation<Lease, Never>]] = [:]
    private var selectionAuthorities: [String: WorkspaceWriteAuthority] = [:]
    private var navigationSequence: UInt64 = 0
    private var projectSelections: [String: UInt64] = [:]

    private func selectionAuthority(projectID: String) -> WorkspaceWriteAuthority {
        if let existing = selectionAuthorities[projectID] { return existing }
        let created = WorkspaceWriteAuthority()
        selectionAuthorities[projectID] = created
        return created
    }

    func issueNavigation() -> UInt64 {
        navigationSequence &+= 1
        return navigationSequence
    }

    /// A slow prepare from an older navigation cannot become the most recent
    /// selection merely because it learned its project ID after a newer one.
    func reserveProjectSelection(projectID: String, sequence: UInt64) -> (WorkspaceWriteAuthority, UInt64)? {
        guard sequence >= (projectSelections[projectID] ?? 0) else { return nil }
        projectSelections[projectID] = sequence
        let authority = selectionAuthority(projectID: projectID)
        authority.invalidate()
        return (authority, authority.currentRevision())
    }

    func invalidateProjectSelection(projectID: String) {
        _ = reserveProjectSelection(projectID: projectID, sequence: issueNavigation())
    }

    func issue(key: String) -> Ticket {
        let next = (revisions[key] ?? 0) &+ 1
        revisions[key] = next
        return Ticket(key: key, revision: next)
    }

    func isCurrent(_ ticket: Ticket) -> Bool { revisions[ticket.key] == ticket.revision }

    func acquire(key: String) async -> Lease {
        if owners[key] == nil {
            let lease = Lease(key: key, id: UUID())
            owners[key] = lease.id
            return lease
        }
        return await withCheckedContinuation { waiters[key, default: []].append($0) }
    }

    func release(_ lease: Lease) {
        guard owners[lease.key] == lease.id else { return }
        if var queue = waiters[lease.key], !queue.isEmpty {
            let next = queue.removeFirst()
            waiters[lease.key] = queue.isEmpty ? nil : queue
            let nextLease = Lease(key: lease.key, id: UUID())
            owners[lease.key] = nextLease.id
            next.resume(returning: nextLease)
        } else {
            owners.removeValue(forKey: lease.key)
        }
    }
}
struct LocalAppAuthoringSessionBinding: Encodable, Sendable {
    let workspaceId: String
    let sourceDirectory: String
    let projectRef: ProjectRef
    let mode: String
}

enum LocalAppAuthoringHostRequest {
    static func importRemix(_ input: LocalAppMarketRemixInput, archivePath: String,
                            mode: SessionMode) throws -> [String: LocalAppJSON] {
        try LocalAppDevelopmentAdmission.requireCode(mode)
        guard UUID(uuidString: input.requestId) != nil, canonicalSessionID(input.requestId) == input.requestId,
              (archivePath as NSString).isAbsolutePath, archivePath == input.archivePath else {
            throw LocalAppServiceFailure("INVALID_AUTHORING", "The market source request is incomplete.")
        }
        return ["op": .string("importRemixSource"), "requestId": .string(input.requestId),
                "archivePath": .string(archivePath), "archiveSha256": .string(input.archiveSha256),
                "archiveBytes": .number(Double(input.archiveBytes)), "manifest": input.signedManifest,
                "files": input.signedFiles, "dependencies": input.signedDependencies,
                "marketOrigin": .object([
                    "appId": .string(input.marketOrigin.appId), "releaseId": .string(input.marketOrigin.releaseId),
                    "digest": .string(input.marketOrigin.digest)
                ])]
    }

    static func getGrant(sourceDirectory: String, projectID: String, sessionID: String,
                         mode: SessionMode) throws -> [String: LocalAppJSON] {
        guard !sourceDirectory.isEmpty, UUID(uuidString: projectID) != nil,
              UUID(uuidString: sessionID) != nil, mode == .code else {
            throw LocalAppServiceFailure("INVALID_AUTHORING", "The authoring owner is incomplete.")
        }
        return ["op": .string("getAuthoringGrant"), "sourceDirectory": .string(sourceDirectory),
                "projectId": .string(canonicalSessionID(projectID)),
                "authoringSessionId": .string(canonicalSessionID(sessionID)), "sessionMode": .string(mode.rawValue)]
    }

    static func register(_ authoring: LocalAppAuthoring, sourceID: String, requestID: String,
                         mode: SessionMode, setCurrent: Bool) throws -> [String: LocalAppJSON] {
        guard UUID(uuidString: sourceID) != nil, UUID(uuidString: requestID) != nil, mode == .code else {
            throw LocalAppServiceFailure("INVALID_AUTHORING", "The app source receipt is incomplete.")
        }
        guard let workspaceID = authoring.workspaceId, UUID(uuidString: workspaceID) != nil,
              case .mobileProject(let projectID)? = authoring.projectRef,
              UUID(uuidString: projectID) != nil,
              canonicalSessionID(projectID) == canonicalSessionID(workspaceID),
              let sessionID = authoring.authoringSessionId, UUID(uuidString: sessionID) != nil,
              !authoring.sourceDirectory.isEmpty else {
            throw LocalAppServiceFailure("INVALID_AUTHORING", "The app authoring workspace is incomplete.")
        }
        let normalized = LocalAppAuthoring(sourceDirectory: authoring.sourceDirectory,
            workspaceId: canonicalSessionID(workspaceID), projectRef: .mobileProject(canonicalSessionID(projectID)),
            authoringSessionId: canonicalSessionID(sessionID), appId: authoring.appId)
        return [
            "op": .string("registerAuthoring"),
            "sourceId": .string(canonicalSessionID(sourceID)),
            "requestId": .string(canonicalSessionID(requestID)),
            "sessionMode": .string(mode.rawValue),
            "setCurrent": .bool(setCurrent),
            "authoring": try .encode(normalized)
        ]
    }
}

enum ProjectRef: Codable, Equatable, Sendable {
    case desktopPath(String)
    case mobileProject(String)

    private enum CodingKeys: String, CodingKey { case kind, path, projectId }
    private enum Kind: String, Codable { case desktopPath, mobileProject }
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        switch try values.decode(Kind.self, forKey: .kind) {
        case .desktopPath: self = .desktopPath(try values.decode(String.self, forKey: .path))
        case .mobileProject: self = .mobileProject(try values.decode(String.self, forKey: .projectId))
        }
    }
    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .desktopPath(let path):
            try values.encode(Kind.desktopPath, forKey: .kind)
            try values.encode(path, forKey: .path)
        case .mobileProject(let projectID):
            try values.encode(Kind.mobileProject, forKey: .kind)
            try values.encode(projectID, forKey: .projectId)
        }
    }
}
struct LocalAppAgentWakeRequest: Equatable, Sendable {
    let appID: String
    let revision: UInt64
    let runID: String
}
struct LocalAppAgentTaskProgress: Decodable, Equatable, Sendable {
    let completedModelTurns: UInt64
    let turnLimit: UInt64
}
struct LocalAppAgentTaskSummary: Decodable, Identifiable, Equatable, Sendable {
    /// Host task identity used for task operations such as cancellation.
    let runId: String
    /// Public AG-UI request/run identifier supplied by the browser.
    let requestId: String
    let state: String
    let sequence: UInt64
    let createdAt: UInt64
    let phase: String?
    let progress: LocalAppAgentTaskProgress?
    let limits: Limits
    let result: Result?
    let error: Failure?
    struct Limits: Decodable, Equatable, Sendable {
        let timeoutMs: UInt64
        let maxTurns: UInt64
    }
    struct Result: Decodable, Equatable, Sendable {
        let text: String
        let output: LocalAppJSON?
        let attachments: [LocalAppJSON]
        let chatId: String?
        let messageId: String?
    }
    struct Failure: Decodable, Equatable, Sendable {
        let code: String
        let message: String
    }
    var id: String { runId }
    var isTerminal: Bool { ["succeeded", "failed", "cancelled", "interrupted"].contains(state) }
}
struct LocalAppAgentTaskListResponse: Decodable, Sendable {
    let appId: String
    let revision: UInt64
    let tasks: [LocalAppAgentTaskSummary]
}
struct LocalAppWakeReservation: Decodable, Sendable {
    let appId: String
    let revision: UInt64
    let runId: String
    let expiresAtMs: UInt64
    let created: Bool
    let reservationId: String
}
struct LocalAppMarketOrigin: Codable, Equatable, Sendable {
    let appId: String
    let releaseId: String
    let digest: String
    var remixedFrom: String?
}

/// The selected immutable release, without download URLs or credentials.
/// The corresponding ZIP is kept under the request's protected staging directory.
struct LocalAppMarketRemixInput: Codable, Equatable, Sendable {
    struct File: Codable, Equatable, Sendable {
        let path: String
        let sizeBytes: Int
        let sha256: String
        let executable: Bool
    }
    let requestId: String
    let archivePath: String
    let archiveSha256: String
    let archiveBytes: Int
    let manifest: UnifiedAppManifest
    let files: [File]
    let dependencies: [File]
    let signedManifest: LocalAppJSON
    let signedFiles: LocalAppJSON
    let signedDependencies: LocalAppJSON
    let marketOrigin: LocalAppMarketOrigin
}

struct LocalAppMarketRemixResult: Decodable, Sendable {
    let requestId: String
    let sourceId: String
    let sourceDirectory: String
    let sourcePath: String
    let record: UnifiedAppRecord
    let release: UnifiedAppRelease
}
struct UnifiedAppRecord: Codable, Equatable, Identifiable, Sendable {
    var id: String { appId }
    let appId: String
    let name: String
    let summary: String
    let currentRevision: UInt64
    let latestRevision: UInt64
    let authoring: LocalAppAuthoring?
    let marketOrigin: LocalAppMarketOrigin?
    let originPort: UInt16
    let dataSchemaVersion: UInt32
    let executionGrants: [String]
    let createdAtMs: UInt64
    let updatedAtMs: UInt64
    let removed: Bool
}
struct UnifiedAppManifest: Codable, Equatable, Sendable {
    struct Entry: Codable, Equatable, Sendable { let type: String; var path: String?; var process: String? }
    struct Target: Codable, Equatable, Sendable {
        var platform: String
        var environment: String
        var architecture: String?
        var runtimeVersion: String?
        var runtimeAbi: String?
        var channels: [String]?
        var capabilities: [String]?
    }
    struct Remix: Codable, Equatable, Sendable { let allowed: Bool; let license: String? }
    let schema: Int
    let name: String
    let summary: String?
    let entry: Entry
    let install: [LocalAppJSON]?
    let build: [LocalAppJSON]?
    let processes: [LocalAppJSON]?
    let capabilities: [String]?
    let targets: [Target]
    let dataSchema: LocalAppJSON?
    let remix: Remix?
    let package: LocalAppJSON?
    let agent: LocalAppJSON?
    var executesCode: Bool { (capabilities ?? []).contains("process") || entry.type == "server" || !(install ?? []).isEmpty || !(build ?? []).isEmpty || !(processes ?? []).isEmpty }
}
struct UnifiedAppRelease: Codable, Equatable, Identifiable, Sendable {
    struct File: Codable, Equatable, Sendable { let path: String; let sha256: String; let bytes: UInt64; let executable: Bool }
    var id: UInt64 { revision }
    let appId: String
    let revision: UInt64
    let digest: String
    let manifest: UnifiedAppManifest
    let files: [File]
    let createdAtMs: UInt64
    let marketOrigin: LocalAppMarketOrigin?
}
struct UnifiedAppRun: Codable, Equatable, Identifiable, Sendable {
    struct Service: Codable, Equatable, Sendable { let name: String; let port: UInt16 }
    var id: String { runId }
    let appId: String
    let runId: String
    let revision: UInt64
    let releaseDigest: String
    let phase: String
    let services: [Service]
    let error: String?
    let startedAtMs: UInt64
    let updatedAtMs: UInt64
    var isLive: Bool { ["preparing", "starting", "running", "stopping"].contains(phase) }
}
struct LocalAppLaunch: Decodable, Sendable { let run: UnifiedAppRun; let url: URL; let origin: String }
struct LocalAppRuntimeStatus: Decodable, Sendable { let run: UnifiedAppRun?; let preparingRun: UnifiedAppRun? }
struct LocalAppPublishResult: Decodable, Sendable { let record: UnifiedAppRecord; let release: UnifiedAppRelease }
struct LocalAppAuthoringResolution: Decodable, Sendable {
    let action: String
    let sourceId: String?
    let sourceDirectory: String
    let sourcePath: String?
    let authoring: LocalAppAuthoring?
    let authoringSessionId: String?
}
struct LocalAppRestoredAuthoringSource: Decodable, Sendable {
    let record: UnifiedAppRecord
    let sourceId: String
    let requestId: String
    let sourceDirectory: String
    let sourcePath: String
}
struct LocalAppLayout: Decodable, Sendable { let sourcePath: String; let dataPath: String; let releasesPath: String; let runsPath: String }
struct LocalAppServiceFailure: Error, LocalizedError, Codable, Sendable {
    let code: String
    let message: String
    var errorDescription: String? { "\(message) (\(code))" }
    init(_ code: String, _ message: String) { self.code = code; self.message = message }
}
struct LocalAppServiceEnvelope<T: Decodable>: Decodable {
    let ok: Bool
    let value: T?
    let error: LocalAppServiceFailure?
    private enum CodingKeys: String, CodingKey { case ok, value, error }
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        ok = try container.decode(Bool.self, forKey: .ok)
        error = try container.decodeIfPresent(LocalAppServiceFailure.self, forKey: .error)
        // JSON null is a valid LocalAppJSON success (configure/cancel/stop).
        // decodeIfPresent would erase it and reject the successful operation.
        value = ok && container.contains(.value) ? try container.decode(T.self, forKey: .value) : nil
    }
    func result() throws -> T {
        guard ok, let value else { throw error ?? LocalAppServiceFailure("INVALID_RESPONSE", "The app service returned an invalid response.") }
        return value
    }
}
