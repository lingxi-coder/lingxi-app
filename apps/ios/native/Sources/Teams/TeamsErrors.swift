import Foundation

/// Error code (from the Teams HTTP API or a local failure) to the string key the UI shows (spec §6.7).
/// Foundation only, so the mapping is tested on macOS; Android has the same table in `teams/TeamsUi.kt`.
enum TeamsErrors {
    /// `Localizable.xcstrings` key for a failure code. Anything not listed is `teams_error`.
    static func key(_ code: String) -> String {
        switch code {
        // Files in a message: why one was not taken, could not be uploaded or cannot be opened.
        case "ATTACHMENT_TYPE", "UPLOAD_TYPE_MISMATCH": "teams_file_error_type"
        case "ATTACHMENT_EMPTY": "teams_file_error_empty"
        case "ATTACHMENT_TOO_LARGE": "teams_file_error_too_large"
        case "ATTACHMENT_COUNT": "teams_file_error_count"
        case "ATTACHMENT_TOTAL", "ATTACHMENT_LIMIT": "teams_file_error_total"
        case "UPLOAD_QUOTA_EXCEEDED": "teams_file_error_quota"
        case "UPLOAD_EXPIRED": "teams_file_error_expired"
        case "UPLOAD_DIGEST_MISMATCH", "UPLOAD_INCOMPLETE", "UPLOAD_PART_INVALID", "UPLOAD_FAILED": "teams_file_error_incomplete"
        case "UPLOAD_REQUIRED", "UPLOAD_SEALED", "UPLOAD_ID_CONFLICT", "ATTACHMENT_REQUIRED": "teams_file_error_unsendable"
        case "ATTACHMENT_REVOKED": "teams_file_error_revoked"
        case "ATTACHMENT_DAMAGED": "teams_file_error_damaged"
        case "ATTACHMENT_UNREADABLE": "teams_file_error_unreadable"
        case "INVALID_DATA", "INVALID_MESSAGE_SNAPSHOT":
            "teams_error_invalid_data"
        case "MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED":
            "teams_stale_error"
        case "BOT_USE_REQUIRED", "INVALID_BOT_TARGET", "BOT_PARTICIPANT_REQUIRED", "BOT_SELECTION_STALE", "CONVERSATION_POLICY_REQUIRED", "ARTIFACT_READ_REQUIRED", "GRANT_READ_REQUIRED", "INPUT_GRANT_REQUIRED":
            "teams_error_bot_access_changed"
        case "BOT_MAINTAINER_CHANGED":
            "teams_composer_lock_bot_handed_over"
        case "REQUEST_QUOTA_EXCEEDED":
            "teams_error_request_quota"
        case "MESSAGE_LISTENER_FAILED":
            "teams_error_sync_failed"
        case "MESSAGE_RECALLED", "MESSAGE_ID_CONFLICT", "MESSAGE_AUTHOR_REQUIRED", "PARTICIPANT_REQUIRED", "CONVERSATION_REJECTED", "INVALID_INPUT",
             "BOT_REVISION_CONFLICT", "MAINTAINER_REQUIRED", "MESSAGE_IN_FLIGHT", "REPLY_REQUIRED",
             "WAIT_REQUIRED", "WAIT_RECIPIENT_REQUIRED", "WAIT_CLOSED", "WAIT_KIND_MISMATCH", "TASK_REVOKED":
            "teams_message_failed"
        case "ARTIFACT_REVISION_CONFLICT":
            "teams_error_doc_conflict"
        case "ARTIFACT_UNCHANGED":
            "teams_error_doc_unchanged"
        case "ARTIFACT_REVISION_LIMIT":
            "teams_error_doc_limit"
        case "ARTIFACT_REVISION_REQUIRED":
            "teams_error_doc_revision"
        case "LIST_REQUIRED", "SHARED_LIST_UNSUPPORTED":
            "teams_error_list_unavailable"
        case "LIST_LIMIT":
            "teams_error_list_full"
        case "LIST_REVISION_CONFLICT", "LIST_ITEM_REQUIRED":
            "teams_list_moved"
        case "INVITE_UNAVAILABLE":
            "teams_invite_error"
        default:
            code.contains("STORAGE") ? "teams_messages_storage_error" : "teams_error"
        }
    }

    /// Codes after which the team's data is no longer shown and the viewer must refresh membership.
    static func isMembershipLoss(_ code: String) -> Bool {
        ["MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "CONVERSATION_REQUIRED"].contains(code)
    }
}
