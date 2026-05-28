//! Re-export of the `tengu_tui_*` event-name constants from
//! `lingxi-telemetry`. Internal callers in `session.rs` use these
//! constants in `tracing::info!(event = ...)` lines.
//!
//! Inventory (8 events at M6-05):
//! - M6-01: `SESSION_STARTED`, `SESSION_ENDED`, `FIRST_RENDER`, `RESIZE`.
//! - M6-03: `STREAMING_RENDER_STARTED`, `STREAMING_RENDER_ENDED`.
//! - M6-05: `PERMISSION_DIALOG_SHOWN`, `PERMISSION_DIALOG_RESOLVED`.

use lingxi_permission::gate::PermissionResponse;

pub use lingxi_telemetry::tengu::tui::{
    FIRST_RENDER, PERMISSION_DIALOG_RESOLVED, PERMISSION_DIALOG_SHOWN, RESIZE, SESSION_ENDED,
    SESSION_STARTED, STREAMING_RENDER_ENDED, STREAMING_RENDER_STARTED,
};

/// (M6-05) Fire when a permission dialog transitions from `None` to
/// `Some(_)`. `kind` is one of `"tool_use" / "exit_plan_mode" /
/// "bypass_permissions"`.
pub fn permission_dialog_shown(kind: &str) {
    tracing::info!(
        target: "lingxi.tengu",
        event = PERMISSION_DIALOG_SHOWN,
        kind = kind,
    );
}

/// (M6-05) Fire when the user resolves a permission dialog.
pub fn permission_dialog_resolved(
    kind: &str,
    response: PermissionResponse,
    persist: bool,
    elapsed_ms: u64,
) {
    let decision = match response {
        PermissionResponse::AllowOnce => "allow_once",
        PermissionResponse::AllowAlways => "allow_always",
        PermissionResponse::Deny => "deny",
    };
    tracing::info!(
        target: "lingxi.tengu",
        event = PERMISSION_DIALOG_RESOLVED,
        kind = kind,
        decision = decision,
        persist = persist,
        elapsed_ms = elapsed_ms,
    );
}
