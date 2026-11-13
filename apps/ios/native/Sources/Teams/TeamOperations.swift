import Foundation

/// One action a task took outside the Chat, as `listOperations` returns it. Parameters and results are never part of it.
struct TeamTaskOperation: Decodable, Equatable, Identifiable, Sendable {
    struct Tool: Decodable, Equatable, Sendable { let name: String; let version: String }
    let operationId: String
    let logicalStepId: String
    let category: String
    let tool: Tool
    let state: String
    let epoch: Int
    let dispatchedAt: Double
    var settledAt: Double? = nil
    var resolvedBy: TeamMemberRef? = nil
    var resolvedAt: Double? = nil
    var id: String { operationId }
    /// Nobody knows whether it went through: it was sent and no result came back, or the result could not be read.
    var unverified: Bool { ["queryable", "unqueryable"].contains(category) && ["dispatched", "unknown"].contains(state) }
}
struct TeamTaskOperations: Decodable, Equatable, Sendable {
    let taskId: String
    let operations: [TeamTaskOperation]
}
struct TeamOperationResolved: Decodable, Sendable { let taskId: String; let state: String; let operationId: String; let operationState: String }

enum TeamOperationRules {
    /// The requester and the Bot's maintainer may look at what a task that needs checking did outside the Chat.
    static func canReview(_ task: TeamTask, viewer: TeamMemberRef, maintainer: TeamMemberRef?) -> Bool {
        task.state == "needs_reconciliation" && (task.requester == viewer || maintainer == viewer)
    }
    /// Only the Bot's maintainer says what happened.
    static func canResolve(viewer: TeamMemberRef, maintainer: TeamMemberRef?) -> Bool { maintainer == viewer }
    static func stateKey(_ operation: TeamTaskOperation) -> String {
        if operation.unverified { return operation.state == "unknown" ? "teams_ops_state_unknown" : "teams_ops_state_unreported" }
        switch operation.state {
        case "succeeded": return "teams_ops_state_happened"
        case "failed": return "teams_ops_state_not_happened"
        default: return "teams_ops_state_repeatable"
        }
    }
    /// The text of a refusal on the review screen. The same codes mean something else for other commands.
    static func errorKey(_ code: String) -> String {
        switch code {
        case "OPERATION_CONFLICT": "teams_error_ops_conflict"
        case "OPERATION_REQUIRED", "OPERATION_READ_FORBIDDEN": "teams_error_ops_unavailable"
        case "MAINTAINER_REQUIRED": "teams_error_ops_maintainer"
        case "TASK_RUNNING": "teams_error_ops_running"
        default: TeamsErrors.key(code)
        }
    }
}
