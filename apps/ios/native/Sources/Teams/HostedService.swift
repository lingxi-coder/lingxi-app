import Foundation

// A Bot offered outside its publisher's Teams (a hosted Bot service), as a phone meets it: the person looks a service
// up by its link, agrees to its terms, and talks to its Bot in sessions of their own. A service is not a Team: nothing
// of it can be watched, so everything here is asked through the API (`/v1/services` for the service, the Team message
// route with `serviceId` for its sessions and requests). A phone hosts no Bots and offers no service.
//
// Foundation only: the rules are the ones of `packages/teams/hosted-service.ts` and are tested on macOS.

enum HostedService {
    /// The id space of a service. No Team has such an id.
    static func isServiceId(_ value: String) -> Bool {
        value.range(of: "^s_[A-Za-z0-9_-]{6,62}$", options: .regularExpression) != nil
    }
    /// The service a link or a pasted id names: `…/services/<id>` or the id itself.
    static func serviceId(from input: String) -> String? {
        let value = input.trimmingCharacters(in: .whitespacesAndNewlines)
        if isServiceId(value) { return value }
        guard let url = URLComponents(string: value), url.scheme?.isEmpty == false, url.host?.isEmpty == false else { return nil }
        var parts = url.percentEncodedPath.split(separator: "/", omittingEmptySubsequences: false).map(String.init)
        if parts.last == "" { parts.removeLast() }
        guard parts.count == 3, parts[0] == "", parts[1] == "services", isServiceId(parts[2]) else { return nil }
        return parts[2]
    }
    /// The string key for why a service would not answer now.
    static func availabilityKey(_ availability: String) -> String {
        switch availability {
        case "available": "services_state_available"
        case "disabled": "services_state_disabled"
        case "not_ready": "services_state_not_ready"
        case "host_offline": "services_state_host_offline"
        case "service_quota_exhausted": "services_state_service_quota"
        case "user_quota_exhausted": "services_state_user_quota"
        default: "services_state_withdrawn"
        }
    }
    /// The string key for a refusal. Codes a service does not have a word for fall back to the Teams table.
    static func errorKey(_ code: String) -> String {
        switch code {
        case "NOT_FOUND": "services_error_not_found"
        case "SERVICE_ENTRY_REQUIRED": "services_error_entry_required"
        case "SERVICE_ACCESS_REQUIRED": "services_error_access_required"
        case "STALE_MEMBERSHIP": "services_error_stale"
        case "SERVICE_WITHDRAWN": "services_error_withdrawn"
        case "SERVICE_DISABLED": "services_error_disabled"
        case "SERVICE_USER_BUSY": "services_error_user_busy"
        case "SERVICE_BUSY": "services_error_busy"
        case "REQUEST_QUOTA_EXCEEDED": "services_error_quota"
        case "HOST_UNAVAILABLE": "services_error_host_unavailable"
        case "SERVICE_SESSION_LIMIT": "services_error_session_limit"
        case "REPORT_LIMIT": "services_error_report_limit"
        case "SERVICE_UNAVAILABLE": "services_error_unavailable"
        case "INVALID_SERVICE_LINK": "services_lookup_invalid"
        case "OWN_SERVICE": "services_own"
        default: TeamsErrors.key(code)
        }
    }
    /// Codes after which the person is no longer in the service: its sessions are closed and the list is read again.
    static func isAccessLoss(_ code: String) -> Bool {
        ["NOT_FOUND", "SERVICE_ENTRY_REQUIRED", "SERVICE_ACCESS_REQUIRED", "STALE_MEMBERSHIP", "SERVICE_WITHDRAWN"].contains(code)
    }
    /// What became of a request that ended without an answer, as a string key; nil while it is under way or answered.
    static func endedKey(_ state: String) -> String? {
        switch state {
        case "failed": "services_task_failed"
        case "expired": "services_task_expired"
        case "interrupted": "services_task_interrupted"
        case "revoked": "services_task_revoked"
        case "cancelled": "services_task_cancelled"
        case "authorization_required": "services_task_authorization_required"
        case "needs_reconciliation": "services_task_needs_reconciliation"
        default: nil
        }
    }
    /// The string key of the line under the messages for a request that is not answered.
    static func taskKey(_ task: TeamTask) -> String? {
        switch task.state {
        case "queued": "services_task_queued"
        case "running": "services_task_running"
        case "waiting": task.wait?.kind == "tool_approval" ? "services_task_approval" : "services_task_question"
        default: endedKey(task.state)
        }
    }
    static let reportReasons = ["harmful", "deceptive", "privacy", "spam", "other"]
    /// The title of a session made by its first request: the request's first 60 characters.
    static func sessionTitle(_ text: String) -> String { String(text.prefix(60)) }
}

/// What a person who may use a service is told about it.
struct HostedServiceView: Decodable, Equatable, Identifiable, Sendable {
    struct Limits: Decodable, Equatable, Sendable {
        let userConcurrent: Int
        var userDaily: Int? = nil
        var userDailyLeft: Int? = nil
    }
    let serviceId: String
    let name: String
    let description: String
    let publisherName: String
    let sharedBotId: String
    let availability: String
    var entered: Bool
    var generation: Int? = nil
    /// What the publisher says is kept.
    let retention: String
    let limits: Limits
    var id: String { serviceId }
    var available: Bool { availability == "available" }
    var withdrawn: Bool { availability == "withdrawn" }
}

/// What looking a service up gives: the service, or the fact that it is the viewer's own (the publisher is shown what
/// they set, which a phone has no screen for).
enum HostedServiceLookup: Decodable, Equatable, Sendable {
    case mine
    case service(HostedServiceView)
    private enum Keys: String, CodingKey { case publisher }
    init(from decoder: Decoder) throws {
        if try decoder.container(keyedBy: Keys.self).contains(.publisher) { self = .mine } else { self = .service(try HostedServiceView(from: decoder)) }
    }
}
struct HostedServiceEntry: Decodable, Sendable { let generation: Int; let service: HostedServiceView }
/// The services the viewer entered. What they offer is not listed on a phone.
struct HostedServiceList: Decodable, Sendable { let entered: [HostedServiceView] }
/// A session of a service as the API lists it.
struct HostedServiceSession: Decodable, Equatable, Identifiable, Sendable { let id: String; let title: String; let lastSeq: Int; let lastChangeSeq: Int }
struct HostedServiceSessions: Decodable, Sendable { let conversations: [HostedServiceSession] }
/// One page of a session's change log, with the messages it names as they are now.
struct HostedServiceSync: Decodable, Sendable {
    struct Change: Decodable, Sendable { let seq: Int; let messageId: String }
    let changes: [Change]
    let messages: [TeamMessage]
    let hasMore: Bool
}
struct HostedServiceTasks: Decodable, Sendable { let tasks: [TeamTask] }
struct HostedServiceAck: Decodable, Sendable {}

/// An open session as far as it has been read: its messages by id, and how much of the change log they include.
struct HostedServiceLog: Equatable, Sendable {
    private(set) var after = 0
    private var byId: [String: TeamMessage] = [:]
    var messages: [TeamMessage] { byId.values.sorted { $0.seq < $1.seq } }
    /// Takes one page read from `after`. Returns whether more is to be read. A page that does not continue the log,
    /// names a message it does not carry, or carries a message of someone else is refused whole.
    mutating func apply(_ page: HostedServiceSync, uid: String, sharedBotId: String) throws -> Bool {
        var next = byId
        let carried = Dictionary(page.messages.map { ($0.id, $0) }, uniquingKeysWith: { _, last in last })
        for (index, change) in page.changes.enumerated() {
            guard change.seq == after + index + 1, let message = carried[change.messageId] else { throw TeamMessageFailure(code: "CHANGE_GAP") }
            // A session holds what its one person and the service's one Bot wrote.
            guard message.author.kind == "user" ? message.author.uid == uid : message.author.sharedBotId == sharedBotId else { throw TeamMessageFailure(code: "INVALID_DATA") }
            next[message.id] = message
        }
        byId = next; after += page.changes.count
        return page.hasMore && !page.changes.isEmpty
    }
}

/// The commands a caller sends in a service's scope. The service refuses every other one.
enum HostedServiceCommands {
    static let taskActions: Set<String> = ["createConversation", "submit", "answerWait", "decideWait", "cancelTask", "getTask", "listTasks"]
    /// A session or request command: the same shape as its Team counterpart, checked the same way, with the scope named `serviceId`.
    static func task(serviceId: String, generation: Int, action: String, payload: [String: Any], commandId: String = UUID().uuidString) throws -> [String: Any] {
        guard HostedService.isServiceId(serviceId), taskActions.contains(action) else { throw TeamMessageFailure(code: "INVALID_INPUT") }
        var body: [String: Any] = ["schemaVersion": 1, "commandId": commandId, "teamId": serviceId, "generation": generation, "action": action, "payload": payload]
        try TeamExecutionContract.validate(body)
        body["teamId"] = nil; body["serviceId"] = serviceId
        return body
    }
    static func sessions(serviceId: String, generation: Int) throws -> [String: Any] {
        guard HostedService.isServiceId(serviceId), generation >= 1 else { throw TeamMessageFailure(code: "INVALID_INPUT") }
        return ["action": "list", "serviceId": serviceId, "generation": generation]
    }
    static func sync(serviceId: String, generation: Int, sessionId: String, after: Int) throws -> [String: Any] {
        guard HostedService.isServiceId(serviceId), generation >= 1, after >= 0 else { throw TeamMessageFailure(code: "INVALID_INPUT") }
        return ["action": "sync", "serviceId": serviceId, "generation": generation, "conversationId": sessionId, "after": after]
    }
    /// What a typed message is: the answer to the Bot's question when a request of this session waits for one from the
    /// viewer, else a new request.
    static func send(serviceId: String, generation: Int, sharedBotId: String, sessionId: String, text: String, tasks: [TeamTask], uid: String,
                     messageId: String = UUID().uuidString, changeId: String = UUID().uuidString, commandId: String = UUID().uuidString) throws -> [String: Any] {
        let viewer = TeamMemberRef(uid: uid, generation: generation)
        let ids: [String: Any] = ["conversationId": sessionId, "messageId": messageId, "changeId": changeId, "text": text]
        if let waitId = tasks.lazy.filter({ $0.conversationId == sessionId }).compactMap({ TeamTaskActions.answerableWait($0, viewer: viewer, pendingWaitIds: []) }).first {
            return try task(serviceId: serviceId, generation: generation, action: "answerWait", payload: ids.merging(["waitId": waitId]) { $1 }, commandId: commandId)
        }
        return try task(serviceId: serviceId, generation: generation, action: "submit", payload: ids.merging(["targets": [sharedBotId]]) { $1 }, commandId: commandId)
    }
    static func newSession(serviceId: String, generation: Int, sharedBotId: String, sessionId: String, title: String, uid: String) throws -> [String: Any] {
        try task(serviceId: serviceId, generation: generation, action: "createConversation",
                 payload: ["conversationId": sessionId, "kind": "bot_dm", "title": title, "participantUids": [uid], "sharedBotIds": [sharedBotId]])
    }
    /// The request that waits for the viewer to approve or decline, if any.
    static func approval(in tasks: [TeamTask], uid: String, generation: Int) -> TeamTask? {
        tasks.first { $0.state == "waiting" && $0.wait?.kind == "tool_approval" && $0.wait?.recipient == TeamMemberRef(uid: uid, generation: generation) }
    }
    /// A command for `/v1/services`.
    static func service(_ action: String, serviceId: String? = nil, more: [String: Any] = [:]) throws -> [String: Any] {
        guard ["listServices", "getService", "enterService", "leaveService", "reportService"].contains(action), (action == "listServices") == (serviceId == nil),
              serviceId.map(HostedService.isServiceId) ?? true else { throw TeamMessageFailure(code: "INVALID_INPUT") }
        var body = more; body["action"] = action
        if let serviceId { body["serviceId"] = serviceId }
        return body
    }
    static func report(serviceId: String, reportId: String, reason: String, text: String, sessionId: String?) throws -> [String: Any] {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard HostedService.reportReasons.contains(reason), text.count <= 1000 else { throw TeamMessageFailure(code: "INVALID_INPUT") }
        var more: [String: Any] = ["reportId": reportId, "reason": reason, "text": text]
        if let sessionId { more["sessionId"] = sessionId }
        return try service("reportService", serviceId: serviceId, more: more)
    }
}
