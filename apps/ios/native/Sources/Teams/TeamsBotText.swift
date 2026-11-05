import Foundation

/// Which words describe a Bot run and the Desktop that serves it (spec §6.3; the label tables of Desktop
/// `BotRunCards.tsx`). Foundation only: the views turn these `Localizable.xcstrings` keys into text, the
/// macOS tests check the tables and that every key exists in the catalogue.
enum TeamsBotText {
    /// The label of a run state (`TeamBotState.state`). Unknown states read as unknown rather than as a raw code.
    static func stateKey(_ state: String) -> String {
        switch state {
        case "queued", "pending": "cron_status_queued"
        case "claimed": "teams_run_preparing"
        case "running": "teams_run_processing"
        case "recovery": "teams_run_recovery"
        case "recovered": "teams_run_recovered"
        case "completed": "chat_status_completed"
        case "failed": "chat_status_failed"
        case "missed": "teams_run_missed"
        case "cancelled": "chat_status_cancelled"
        case "revoked": "teams_run_revoked"
        default: "chat_status_unknown"
        }
    }

    /// What the owner's Desktop reports about a run (`TeamBotState.hostStatus`); `nil` for no or an unknown status.
    static func hostKey(_ status: String?) -> String? {
        switch status {
        case "retrying": "teams_run_host_retrying"
        case "result_pending": "teams_run_host_result_pending"
        case "recovery_required": "teams_run_host_recovery_required"
        case "model_unavailable": "teams_run_host_model_unavailable"
        case "runtime_unavailable": "teams_run_host_runtime_unavailable"
        case "storage_error": "teams_run_host_storage_error"
        default: nil
        }
    }

    /// Every key this file can return, for the catalogue check.
    static let allKeys: [String] = [
        "cron_status_queued", "teams_run_preparing", "teams_run_processing", "teams_run_recovery", "teams_run_recovered",
        "chat_status_completed", "chat_status_failed", "teams_run_missed", "chat_status_cancelled", "teams_run_revoked",
        "chat_status_unknown", "teams_run_host_retrying", "teams_run_host_result_pending", "teams_run_host_recovery_required",
        "teams_run_host_model_unavailable", "teams_run_host_runtime_unavailable", "teams_run_host_storage_error",
    ]
}
