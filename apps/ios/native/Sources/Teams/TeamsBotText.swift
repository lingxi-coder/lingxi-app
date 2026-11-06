import Foundation

enum TeamsBotText {
    static func stateKey(_ state: String) -> String {
        switch state {
        case "queued": "cron_status_queued"
        case "running": "teams_run_processing"
        case "completed": "chat_status_completed"
        case "failed": "chat_status_failed"
        case "expired": "teams_task_expired"
        case "interrupted": "teams_task_interrupted"
        case "authorization_required": "teams_task_authorization_required"
        case "revoked": "teams_run_revoked"
        default: "chat_status_unknown"
        }
    }
    static let allKeys = ["cron_status_queued", "teams_run_processing", "chat_status_completed", "chat_status_failed", "teams_task_expired", "teams_task_interrupted", "teams_task_authorization_required", "teams_run_revoked", "chat_status_unknown"]
}
